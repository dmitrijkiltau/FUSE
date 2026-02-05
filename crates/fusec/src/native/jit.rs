use std::collections::{BTreeSet, HashMap};

use cranelift_codegen::ir::{
    AbiParam,
    InstBuilder,
    MemFlags,
    Value as ClifValue,
    condcodes::{FloatCC, IntCC},
    types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};

use crate::ast::{TypeRef, TypeRefKind};
use crate::interp::Value;
use crate::ir::{Const, Function, Instr, Program as IrProgram};

type EntryFn = unsafe extern "C" fn(*const i64) -> i64;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum JitType {
    Int,
    Bool,
    Float,
}

#[derive(Copy, Clone)]
struct StackValue {
    value: ClifValue,
    kind: JitType,
}

#[derive(Copy, Clone)]
enum ReturnKind {
    Int,
    Bool,
    Float,
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
    pub(crate) fn build() -> Self {
        let builder =
            JITBuilder::new(default_libcall_names()).expect("JIT builder creation should not fail");
        Self {
            _module: JITModule::new(builder),
            functions: HashMap::new(),
        }
    }

    pub(crate) fn try_call(
        &mut self,
        program: &IrProgram,
        name: &str,
        args: &[Value],
    ) -> Option<Value> {
        if !self.functions.contains_key(name) {
            let func = program.functions.get(name)?;
            let mut param_types = Vec::with_capacity(args.len());
            for arg in args {
                param_types.push(value_kind(arg)?);
            }
            if let Some(compiled) =
                compile_function(&mut self._module, name, func, &param_types)
            {
                if self._module.finalize_definitions().is_err() {
                    return None;
                }
                let raw = self._module.get_finalized_function(compiled.id);
                // SAFETY: The JIT function is declared with `fn(*const i64) -> i64`.
                let entry = unsafe { std::mem::transmute::<*const u8, EntryFn>(raw) };
                self.functions.insert(
                    name.to_string(),
                    CompiledEntry {
                        entry,
                        arity: compiled.arity,
                        ret: compiled.ret,
                    },
                );
            }
        }
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
            ReturnKind::Float => Value::Float(f64::from_bits(raw as u64)),
        })
    }

    pub(crate) fn has_function(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }

}

fn compile_function(
    module: &mut JITModule,
    name: &str,
    func: &Function,
    param_types: &[JitType],
) -> Option<PendingCompiled> {
    let ret = return_kind(func.ret.as_ref()?)?;
    if func.code.is_empty() || func.params.len() != param_types.len() {
        return None;
    }

    let starts = block_starts(&func.code)?;
    let local_types = infer_local_types(func, param_types, &starts)?;
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
        let local_ty = clif_type(local_types.get(slot)?);
        builder.declare_var(var, local_ty);
        let init = if slot < func.params.len() {
            let offset = i32::try_from(slot.checked_mul(8)?).ok()?;
            builder
                .ins()
                .load(local_ty, MemFlags::new(), args_ptr, offset)
        } else if local_ty == types::F64 {
            builder.ins().f64const(0.0)
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
        let mut stack: Vec<StackValue> = Vec::new();
        let mut terminated = false;
        for ip in *start..end {
            match &func.code[ip] {
                Instr::Push(Const::Unit) => {
                    stack.push(StackValue {
                        value: builder.ins().iconst(types::I64, 0),
                        kind: JitType::Int,
                    });
                }
                Instr::Push(Const::Int(v)) => {
                    stack.push(StackValue {
                        value: builder.ins().iconst(types::I64, *v),
                        kind: JitType::Int,
                    });
                }
                Instr::Push(Const::Bool(v)) => {
                    stack.push(StackValue {
                        value: builder.ins().iconst(types::I64, if *v { 1 } else { 0 }),
                        kind: JitType::Bool,
                    });
                }
                Instr::Push(Const::Float(v)) => {
                    stack.push(StackValue {
                        value: builder.ins().f64const(*v),
                        kind: JitType::Float,
                    });
                }
                Instr::LoadLocal(slot) => {
                    let var = *locals.get(*slot)?;
                    let kind = *local_types.get(*slot)?;
                    stack.push(StackValue {
                        value: builder.use_var(var),
                        kind,
                    });
                }
                Instr::StoreLocal(slot) => {
                    let value = stack.pop()?;
                    let var = *locals.get(*slot)?;
                    let kind = *local_types.get(*slot)?;
                    if value.kind != kind {
                        return None;
                    }
                    builder.def_var(var, value.value);
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
                    match value.kind {
                        JitType::Int => stack.push(StackValue {
                            value: builder.ins().ineg(value.value),
                            kind: JitType::Int,
                        }),
                        JitType::Float => stack.push(StackValue {
                            value: builder.ins().fneg(value.value),
                            kind: JitType::Float,
                        }),
                        _ => return None,
                    }
                }
                Instr::Not => {
                    let value = stack.pop()?;
                    if value.kind != JitType::Bool {
                        return None;
                    }
                    let is_zero = builder.ins().icmp_imm(IntCC::Equal, value.value, 0);
                    stack.push(StackValue {
                        value: bool_to_i64(&mut builder, is_zero),
                        kind: JitType::Bool,
                    });
                }
                Instr::Add | Instr::Sub | Instr::Mul | Instr::Div | Instr::Mod => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    let out = numeric_binop(&mut builder, &lhs, &rhs, &func.code[ip])?;
                    stack.push(out);
                }
                Instr::Eq | Instr::NotEq | Instr::Lt | Instr::LtEq | Instr::Gt | Instr::GtEq => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    let out = compare_op(&mut builder, &lhs, &rhs, &func.code[ip])?;
                    stack.push(out);
                }
                Instr::And => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    if lhs.kind != JitType::Bool || rhs.kind != JitType::Bool {
                        return None;
                    }
                    let lhs_b = builder.ins().icmp_imm(IntCC::NotEqual, lhs.value, 0);
                    let rhs_b = builder.ins().icmp_imm(IntCC::NotEqual, rhs.value, 0);
                    let value = builder.ins().band(lhs_b, rhs_b);
                    stack.push(StackValue {
                        value: bool_to_i64(&mut builder, value),
                        kind: JitType::Bool,
                    });
                }
                Instr::Or => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    if lhs.kind != JitType::Bool || rhs.kind != JitType::Bool {
                        return None;
                    }
                    let lhs_b = builder.ins().icmp_imm(IntCC::NotEqual, lhs.value, 0);
                    let rhs_b = builder.ins().icmp_imm(IntCC::NotEqual, rhs.value, 0);
                    let value = builder.ins().bor(lhs_b, rhs_b);
                    stack.push(StackValue {
                        value: bool_to_i64(&mut builder, value),
                        kind: JitType::Bool,
                    });
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
                    if cond.kind != JitType::Bool {
                        return None;
                    }
                    let is_false = builder.ins().icmp_imm(IntCC::Equal, cond.value, 0);
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
                    let ret_value = match (ret, value.kind) {
                        (ReturnKind::Int, JitType::Int) => value.value,
                        (ReturnKind::Bool, JitType::Bool) => value.value,
                        (ReturnKind::Float, JitType::Float) => {
                            builder
                                .ins()
                                .bitcast(types::I64, MemFlags::new(), value.value)
                        }
                        _ => return None,
                    };
                    builder.ins().return_(&[ret_value]);
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
                let ret_value = match (ret, value.kind) {
                    (ReturnKind::Int, JitType::Int) => value.value,
                    (ReturnKind::Bool, JitType::Bool) => value.value,
                    (ReturnKind::Float, JitType::Float) => {
                        builder
                            .ins()
                            .bitcast(types::I64, MemFlags::new(), value.value)
                    }
                    _ => return None,
                };
                builder.ins().return_(&[ret_value]);
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

fn bool_to_i64(builder: &mut FunctionBuilder<'_>, value: ClifValue) -> ClifValue {
    let one = builder.ins().iconst(types::I64, 1);
    let zero = builder.ins().iconst(types::I64, 0);
    builder.ins().select(value, one, zero)
}

fn return_kind(ty: &TypeRef) -> Option<ReturnKind> {
    match &ty.kind {
        TypeRefKind::Simple(name) if name.name == "Int" => Some(ReturnKind::Int),
        TypeRefKind::Simple(name) if name.name == "Bool" => Some(ReturnKind::Bool),
        TypeRefKind::Simple(name) if name.name == "Float" => Some(ReturnKind::Float),
        TypeRefKind::Refined { base, .. } if base.name == "Int" => Some(ReturnKind::Int),
        TypeRefKind::Refined { base, .. } if base.name == "Float" => Some(ReturnKind::Float),
        _ => None,
    }
}

fn value_to_raw(value: &Value) -> Option<i64> {
    match value {
        Value::Int(v) => Some(*v),
        Value::Bool(v) => Some(if *v { 1 } else { 0 }),
        Value::Float(v) => Some(v.to_bits() as i64),
        _ => None,
    }
}

fn value_kind(value: &Value) -> Option<JitType> {
    match value {
        Value::Int(_) => Some(JitType::Int),
        Value::Bool(_) => Some(JitType::Bool),
        Value::Float(_) => Some(JitType::Float),
        _ => None,
    }
}

fn clif_type(kind: &JitType) -> types::Type {
    match kind {
        JitType::Float => types::F64,
        _ => types::I64,
    }
}

fn infer_local_types(
    func: &Function,
    param_types: &[JitType],
    starts: &[usize],
) -> Option<Vec<JitType>> {
    let mut locals: Vec<Option<JitType>> = vec![None; func.locals];
    for (idx, kind) in param_types.iter().enumerate() {
        if idx < locals.len() {
            locals[idx] = Some(*kind);
        }
    }
    for (block_idx, start) in starts.iter().enumerate() {
        let end = if block_idx + 1 < starts.len() {
            starts[block_idx + 1]
        } else {
            func.code.len()
        };
        let mut stack: Vec<JitType> = Vec::new();
        for ip in *start..end {
            match &func.code[ip] {
                Instr::Push(Const::Unit) => stack.push(JitType::Int),
                Instr::Push(Const::Int(_)) => stack.push(JitType::Int),
                Instr::Push(Const::Bool(_)) => stack.push(JitType::Bool),
                Instr::Push(Const::Float(_)) => stack.push(JitType::Float),
                Instr::LoadLocal(slot) => {
                    let kind = locals.get(*slot)?.as_ref()?;
                    stack.push(*kind);
                }
                Instr::StoreLocal(slot) => {
                    let kind = stack.pop()?;
                    match locals.get_mut(*slot)? {
                        Some(existing) if *existing != kind => return None,
                        Some(_) => {}
                        slot_entry @ None => {
                            *slot_entry = Some(kind);
                        }
                    }
                }
                Instr::Pop => {
                    stack.pop()?;
                }
                Instr::Dup => {
                    let kind = *stack.last()?;
                    stack.push(kind);
                }
                Instr::Neg => {
                    let kind = stack.pop()?;
                    match kind {
                        JitType::Int | JitType::Float => stack.push(kind),
                        _ => return None,
                    }
                }
                Instr::Not => {
                    let kind = stack.pop()?;
                    if kind != JitType::Bool {
                        return None;
                    }
                    stack.push(JitType::Bool);
                }
                Instr::Add | Instr::Sub | Instr::Mul | Instr::Div | Instr::Mod => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    let out = numeric_kind(lhs, rhs, &func.code[ip])?;
                    stack.push(out);
                }
                Instr::Eq | Instr::NotEq | Instr::Lt | Instr::LtEq | Instr::Gt | Instr::GtEq => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    let out = compare_kind(lhs, rhs, &func.code[ip])?;
                    stack.push(out);
                }
                Instr::And | Instr::Or => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    if lhs != JitType::Bool || rhs != JitType::Bool {
                        return None;
                    }
                    stack.push(JitType::Bool);
                }
                Instr::Jump(_) => {
                    if !stack.is_empty() {
                        return None;
                    }
                }
                Instr::JumpIfFalse(_) => {
                    let cond = stack.pop()?;
                    if cond != JitType::Bool || !stack.is_empty() {
                        return None;
                    }
                }
                Instr::Return => {
                    let _ = stack.pop()?;
                    if !stack.is_empty() {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        if !stack.is_empty() {
            return None;
        }
    }
    Some(
        locals
            .into_iter()
            .map(|kind| kind.unwrap_or(JitType::Int))
            .collect(),
    )
}

fn numeric_kind(lhs: JitType, rhs: JitType, op: &Instr) -> Option<JitType> {
    match (lhs, rhs) {
        (JitType::Int, JitType::Int) => Some(JitType::Int),
        (JitType::Float, JitType::Float) => match op {
            Instr::Mod => None,
            _ => Some(JitType::Float),
        },
        (JitType::Float, JitType::Int) | (JitType::Int, JitType::Float) => match op {
            Instr::Mod => None,
            _ => Some(JitType::Float),
        },
        _ => None,
    }
}

fn compare_kind(lhs: JitType, rhs: JitType, op: &Instr) -> Option<JitType> {
    match (lhs, rhs) {
        (JitType::Int, JitType::Int) => Some(JitType::Bool),
        (JitType::Float, JitType::Float) => Some(JitType::Bool),
        (JitType::Int, JitType::Float) | (JitType::Float, JitType::Int) => Some(JitType::Bool),
        (JitType::Bool, JitType::Bool) => match op {
            Instr::Eq | Instr::NotEq => Some(JitType::Bool),
            _ => None,
        },
        _ => None,
    }
}

fn numeric_binop(
    builder: &mut FunctionBuilder<'_>,
    lhs: &StackValue,
    rhs: &StackValue,
    op: &Instr,
) -> Option<StackValue> {
    match (lhs.kind, rhs.kind) {
        (JitType::Int, JitType::Int) => {
            let value = match op {
                Instr::Add => builder.ins().iadd(lhs.value, rhs.value),
                Instr::Sub => builder.ins().isub(lhs.value, rhs.value),
                Instr::Mul => builder.ins().imul(lhs.value, rhs.value),
                Instr::Div => builder.ins().sdiv(lhs.value, rhs.value),
                Instr::Mod => builder.ins().srem(lhs.value, rhs.value),
                _ => return None,
            };
            Some(StackValue {
                value,
                kind: JitType::Int,
            })
        }
        (JitType::Float, JitType::Float) => {
            if matches!(op, Instr::Mod) {
                return None;
            }
            let value = match op {
                Instr::Add => builder.ins().fadd(lhs.value, rhs.value),
                Instr::Sub => builder.ins().fsub(lhs.value, rhs.value),
                Instr::Mul => builder.ins().fmul(lhs.value, rhs.value),
                Instr::Div => builder.ins().fdiv(lhs.value, rhs.value),
                _ => return None,
            };
            Some(StackValue {
                value,
                kind: JitType::Float,
            })
        }
        (JitType::Float, JitType::Int) | (JitType::Int, JitType::Float) => {
            if matches!(op, Instr::Mod) {
                return None;
            }
            let lhs_f = to_float(builder, lhs)?;
            let rhs_f = to_float(builder, rhs)?;
            let value = match op {
                Instr::Add => builder.ins().fadd(lhs_f, rhs_f),
                Instr::Sub => builder.ins().fsub(lhs_f, rhs_f),
                Instr::Mul => builder.ins().fmul(lhs_f, rhs_f),
                Instr::Div => builder.ins().fdiv(lhs_f, rhs_f),
                _ => return None,
            };
            Some(StackValue {
                value,
                kind: JitType::Float,
            })
        }
        _ => None,
    }
}

fn compare_op(
    builder: &mut FunctionBuilder<'_>,
    lhs: &StackValue,
    rhs: &StackValue,
    op: &Instr,
) -> Option<StackValue> {
    let cmp = match (lhs.kind, rhs.kind) {
        (JitType::Int, JitType::Int) => {
            let cc = match op {
                Instr::Eq => IntCC::Equal,
                Instr::NotEq => IntCC::NotEqual,
                Instr::Lt => IntCC::SignedLessThan,
                Instr::LtEq => IntCC::SignedLessThanOrEqual,
                Instr::Gt => IntCC::SignedGreaterThan,
                Instr::GtEq => IntCC::SignedGreaterThanOrEqual,
                _ => return None,
            };
            builder.ins().icmp(cc, lhs.value, rhs.value)
        }
        (JitType::Float, JitType::Float) => {
            let cc = match op {
                Instr::Eq => FloatCC::Equal,
                Instr::NotEq => FloatCC::NotEqual,
                Instr::Lt => FloatCC::LessThan,
                Instr::LtEq => FloatCC::LessThanOrEqual,
                Instr::Gt => FloatCC::GreaterThan,
                Instr::GtEq => FloatCC::GreaterThanOrEqual,
                _ => return None,
            };
            builder.ins().fcmp(cc, lhs.value, rhs.value)
        }
        (JitType::Int, JitType::Float) | (JitType::Float, JitType::Int) => {
            let cc = match op {
                Instr::Eq => FloatCC::Equal,
                Instr::NotEq => FloatCC::NotEqual,
                Instr::Lt => FloatCC::LessThan,
                Instr::LtEq => FloatCC::LessThanOrEqual,
                Instr::Gt => FloatCC::GreaterThan,
                Instr::GtEq => FloatCC::GreaterThanOrEqual,
                _ => return None,
            };
            let lhs_f = to_float(builder, lhs)?;
            let rhs_f = to_float(builder, rhs)?;
            builder.ins().fcmp(cc, lhs_f, rhs_f)
        }
        (JitType::Bool, JitType::Bool) => {
            let cc = match op {
                Instr::Eq => IntCC::Equal,
                Instr::NotEq => IntCC::NotEqual,
                _ => return None,
            };
            builder.ins().icmp(cc, lhs.value, rhs.value)
        }
        _ => return None,
    };
    Some(StackValue {
        value: bool_to_i64(builder, cmp),
        kind: JitType::Bool,
    })
}

fn to_float(builder: &mut FunctionBuilder<'_>, value: &StackValue) -> Option<ClifValue> {
    match value.kind {
        JitType::Float => Some(value.value),
        JitType::Int => Some(builder.ins().fcvt_from_sint(types::F64, value.value)),
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
