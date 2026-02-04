use std::collections::{BTreeSet, HashMap};

use cranelift_codegen::ir::{
    AbiParam, InstBuilder, MemFlags, Value as ClifValue, condcodes::IntCC, types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};

use crate::ast::{TypeRef, TypeRefKind};
use crate::interp::Value;
use crate::ir::{Const, Function, Instr, Program as IrProgram};

type EntryFn = unsafe extern "C" fn(*const i64) -> i64;

#[derive(Copy, Clone)]
enum ReturnKind {
    Int,
    Bool,
}

struct CompiledEntry {
    entry: EntryFn,
    arity: usize,
    ret: ReturnKind,
}

struct PendingCompiled {
    id: FuncId,
    arity: usize,
    ret: ReturnKind,
}

pub(crate) struct JitRuntime {
    _module: JITModule,
    functions: HashMap<String, CompiledEntry>,
}

impl JitRuntime {
    pub(crate) fn build(program: &IrProgram) -> Self {
        let builder = JITBuilder::new(default_libcall_names());
        let Ok(builder) = builder else {
            return Self::empty();
        };
        let mut module = JITModule::new(builder);
        let mut pending = Vec::new();
        for (name, func) in &program.functions {
            if let Some(compiled) = compile_function(&mut module, name, func) {
                pending.push((name.clone(), compiled));
            }
        }
        if module.finalize_definitions().is_err() {
            return Self::empty();
        }
        let mut functions = HashMap::new();
        for (name, compiled) in pending {
            let raw = module.get_finalized_function(compiled.id);
            // SAFETY: The JIT function is declared with `fn(*const i64) -> i64`.
            let entry = unsafe { std::mem::transmute::<*const u8, EntryFn>(raw) };
            functions.insert(
                name,
                CompiledEntry {
                    entry,
                    arity: compiled.arity,
                    ret: compiled.ret,
                },
            );
        }
        Self {
            _module: module,
            functions,
        }
    }

    pub(crate) fn try_call(&self, name: &str, args: &[Value]) -> Option<Value> {
        let compiled = self.functions.get(name)?;
        if args.len() != compiled.arity {
            return None;
        }
        let mut raw_args = Vec::with_capacity(args.len());
        for arg in args {
            raw_args.push(value_to_raw(arg)?);
        }
        // SAFETY: Function pointer is JIT-compiled with matching signature and argument layout.
        let raw = unsafe { (compiled.entry)(raw_args.as_ptr()) };
        Some(match compiled.ret {
            ReturnKind::Int => Value::Int(raw),
            ReturnKind::Bool => Value::Bool(raw != 0),
        })
    }

    pub(crate) fn has_function(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }

    fn empty() -> Self {
        let builder =
            JITBuilder::new(default_libcall_names()).expect("JIT builder creation should not fail");
        Self {
            _module: JITModule::new(builder),
            functions: HashMap::new(),
        }
    }
}

fn compile_function(
    module: &mut JITModule,
    name: &str,
    func: &Function,
) -> Option<PendingCompiled> {
    let ret = return_kind(func.ret.as_ref()?)?;
    if func.code.is_empty() {
        return None;
    }

    let starts = block_starts(&func.code)?;
    let mut ctx = module.make_context();
    let pointer_ty = module.target_config().pointer_type();
    ctx.func.signature.params.push(AbiParam::new(pointer_ty));
    ctx.func.signature.returns.push(AbiParam::new(types::I64));
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

    let blocks: Vec<_> = starts.iter().map(|_| builder.create_block()).collect();
    let mut block_for_start = HashMap::new();
    for (idx, start) in starts.iter().enumerate() {
        block_for_start.insert(*start, idx);
    }

    builder.switch_to_block(blocks[0]);
    builder.append_block_params_for_function_params(blocks[0]);
    let args_ptr = builder.block_params(blocks[0])[0];

    let mut locals = Vec::with_capacity(func.locals);
    for slot in 0..func.locals {
        let var = Variable::from_u32(u32::try_from(slot).ok()?);
        builder.declare_var(var, types::I64);
        let init = if slot < func.params.len() {
            let offset = i32::try_from(slot.checked_mul(8)?).ok()?;
            builder
                .ins()
                .load(types::I64, MemFlags::new(), args_ptr, offset)
        } else {
            builder.ins().iconst(types::I64, 0)
        };
        builder.def_var(var, init);
        locals.push(var);
    }

    for (block_idx, start) in starts.iter().enumerate() {
        let block = blocks[block_idx];
        if block_idx != 0 {
            builder.switch_to_block(block);
        }
        let end = if block_idx + 1 < starts.len() {
            starts[block_idx + 1]
        } else {
            func.code.len()
        };
        let mut stack: Vec<ClifValue> = Vec::new();
        let mut terminated = false;
        for ip in *start..end {
            match &func.code[ip] {
                Instr::Push(Const::Unit) => {
                    stack.push(builder.ins().iconst(types::I64, 0));
                }
                Instr::Push(Const::Int(v)) => {
                    stack.push(builder.ins().iconst(types::I64, *v));
                }
                Instr::Push(Const::Bool(v)) => {
                    stack.push(builder.ins().iconst(types::I64, if *v { 1 } else { 0 }));
                }
                Instr::LoadLocal(slot) => {
                    let var = *locals.get(*slot)?;
                    stack.push(builder.use_var(var));
                }
                Instr::StoreLocal(slot) => {
                    let value = stack.pop()?;
                    let var = *locals.get(*slot)?;
                    builder.def_var(var, value);
                }
                Instr::Pop => {
                    stack.pop()?;
                }
                Instr::Dup => {
                    let value = *stack.last()?;
                    stack.push(value);
                }
                Instr::Neg => {
                    let value = stack.pop()?;
                    stack.push(builder.ins().ineg(value));
                }
                Instr::Not => {
                    let value = stack.pop()?;
                    let is_zero = builder.ins().icmp_imm(IntCC::Equal, value, 0);
                    stack.push(bool_to_i64(&mut builder, is_zero));
                }
                Instr::Add => binop(&mut builder, &mut stack, |b, l, r| b.ins().iadd(l, r))?,
                Instr::Sub => binop(&mut builder, &mut stack, |b, l, r| b.ins().isub(l, r))?,
                Instr::Mul => binop(&mut builder, &mut stack, |b, l, r| b.ins().imul(l, r))?,
                Instr::Eq => cmpop(&mut builder, &mut stack, IntCC::Equal)?,
                Instr::NotEq => cmpop(&mut builder, &mut stack, IntCC::NotEqual)?,
                Instr::Lt => cmpop(&mut builder, &mut stack, IntCC::SignedLessThan)?,
                Instr::LtEq => cmpop(&mut builder, &mut stack, IntCC::SignedLessThanOrEqual)?,
                Instr::Gt => cmpop(&mut builder, &mut stack, IntCC::SignedGreaterThan)?,
                Instr::GtEq => cmpop(&mut builder, &mut stack, IntCC::SignedGreaterThanOrEqual)?,
                Instr::And => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    let lhs_b = builder.ins().icmp_imm(IntCC::NotEqual, lhs, 0);
                    let rhs_b = builder.ins().icmp_imm(IntCC::NotEqual, rhs, 0);
                    let value = builder.ins().band(lhs_b, rhs_b);
                    stack.push(bool_to_i64(&mut builder, value));
                }
                Instr::Or => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    let lhs_b = builder.ins().icmp_imm(IntCC::NotEqual, lhs, 0);
                    let rhs_b = builder.ins().icmp_imm(IntCC::NotEqual, rhs, 0);
                    let value = builder.ins().bor(lhs_b, rhs_b);
                    stack.push(bool_to_i64(&mut builder, value));
                }
                Instr::Jump(target) => {
                    if !stack.is_empty() {
                        return None;
                    }
                    let idx = *block_for_start.get(target)?;
                    builder.ins().jump(blocks[idx], &[]);
                    terminated = true;
                    break;
                }
                Instr::JumpIfFalse(target) => {
                    let cond = stack.pop()?;
                    if !stack.is_empty() {
                        return None;
                    }
                    let is_false = builder.ins().icmp_imm(IntCC::Equal, cond, 0);
                    let then_idx = *block_for_start.get(target)?;
                    let else_ip = ip + 1;
                    let else_idx = *block_for_start.get(&else_ip)?;
                    builder
                        .ins()
                        .brif(is_false, blocks[then_idx], &[], blocks[else_idx], &[]);
                    terminated = true;
                    break;
                }
                Instr::Return => {
                    let value = stack.pop()?;
                    if !stack.is_empty() {
                        return None;
                    }
                    builder.ins().return_(&[value]);
                    terminated = true;
                    break;
                }
                _ => return None,
            }
        }

        if !terminated {
            if block_idx + 1 < blocks.len() {
                if !stack.is_empty() {
                    return None;
                }
                builder.ins().jump(blocks[block_idx + 1], &[]);
            } else if stack.len() == 1 {
                let value = stack.pop()?;
                builder.ins().return_(&[value]);
            } else if stack.is_empty() {
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().return_(&[zero]);
            } else {
                return None;
            }
        }
    }

    builder.seal_all_blocks();
    builder.finalize();

    let symbol = jit_symbol(name);
    let id = module
        .declare_function(&symbol, Linkage::Local, &ctx.func.signature)
        .ok()?;
    module.define_function(id, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    Some(PendingCompiled {
        id,
        arity: func.params.len(),
        ret,
    })
}

fn block_starts(code: &[Instr]) -> Option<Vec<usize>> {
    let mut starts = BTreeSet::new();
    starts.insert(0usize);
    for (ip, instr) in code.iter().enumerate() {
        match instr {
            Instr::Jump(target) | Instr::JumpIfFalse(target) => {
                if *target >= code.len() {
                    return None;
                }
                starts.insert(*target);
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            _ => {}
        }
    }
    Some(starts.into_iter().collect())
}

fn binop(
    builder: &mut FunctionBuilder<'_>,
    stack: &mut Vec<ClifValue>,
    f: impl FnOnce(&mut FunctionBuilder<'_>, ClifValue, ClifValue) -> ClifValue,
) -> Option<()> {
    let rhs = stack.pop()?;
    let lhs = stack.pop()?;
    stack.push(f(builder, lhs, rhs));
    Some(())
}

fn cmpop(builder: &mut FunctionBuilder<'_>, stack: &mut Vec<ClifValue>, cc: IntCC) -> Option<()> {
    let rhs = stack.pop()?;
    let lhs = stack.pop()?;
    let cmp = builder.ins().icmp(cc, lhs, rhs);
    stack.push(bool_to_i64(builder, cmp));
    Some(())
}

fn bool_to_i64(builder: &mut FunctionBuilder<'_>, value: ClifValue) -> ClifValue {
    let one = builder.ins().iconst(types::I64, 1);
    let zero = builder.ins().iconst(types::I64, 0);
    builder.ins().select(value, one, zero)
}

fn return_kind(ty: &TypeRef) -> Option<ReturnKind> {
    match &ty.kind {
        TypeRefKind::Simple(name) if name.name == "Int" => Some(ReturnKind::Int),
        TypeRefKind::Simple(name) if name.name == "Bool" => Some(ReturnKind::Bool),
        TypeRefKind::Refined { base, .. } if base.name == "Int" => Some(ReturnKind::Int),
        _ => None,
    }
}

fn value_to_raw(value: &Value) -> Option<i64> {
    match value {
        Value::Int(v) => Some(*v),
        Value::Bool(v) => Some(if *v { 1 } else { 0 }),
        _ => None,
    }
}

fn jit_symbol(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 16);
    out.push_str("__fuse_jit_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}
