use std::collections::{BTreeSet, HashMap};

use cranelift_codegen::ir::{
    AbiParam,
    InstBuilder,
    MemFlags,
    StackSlotData,
    StackSlotKind,
    Value as ClifValue,
    condcodes::{FloatCC, IntCC},
    types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};

use crate::ast::{TypeRef, TypeRefKind};
use crate::interp::Value;
use crate::native::value::{HeapValue, NativeHeap, NativeTag, NativeValue};
use crate::ir::{Const, Function, Instr, Program as IrProgram};

type EntryFn = unsafe extern "C" fn(*const NativeValue, *mut NativeValue, *mut NativeHeap) -> u8;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum JitType {
    Int,
    Bool,
    Float,
    Heap,
    Struct,
    Enum,
    Boxed,
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
    Heap,
}

struct CompiledEntry {
    entry: EntryFn,
    arity: usize,
}

struct PendingCompiled {
    id: FuncId,
    arity: usize,
}

struct HostCalls {
    make_list: FuncId,
    make_map: FuncId,
    make_struct: FuncId,
    get_struct_field: FuncId,
    make_enum: FuncId,
    make_box: FuncId,
    interp_string: FuncId,
}

pub(crate) struct JitRuntime {
    _module: JITModule,
    functions: HashMap<String, CompiledEntry>,
    hostcalls: HostCalls,
}

const NATIVE_VALUE_SIZE: i32 = std::mem::size_of::<NativeValue>() as i32;
const NATIVE_VALUE_PAYLOAD_OFFSET: i32 = std::mem::size_of::<NativeTag>() as i32;
impl JitRuntime {
    pub(crate) fn build() -> Self {
        let mut builder =
            JITBuilder::new(default_libcall_names()).expect("JIT builder creation should not fail");
        builder.symbol(
            "fuse_native_make_list",
            fuse_native_make_list as *const u8,
        );
        builder.symbol("fuse_native_make_map", fuse_native_make_map as *const u8);
        builder.symbol(
            "fuse_native_make_struct",
            fuse_native_make_struct as *const u8,
        );
        builder.symbol(
            "fuse_native_get_struct_field",
            fuse_native_get_struct_field as *const u8,
        );
        builder.symbol("fuse_native_make_enum", fuse_native_make_enum as *const u8);
        builder.symbol("fuse_native_make_box", fuse_native_make_box as *const u8);
        builder.symbol("fuse_native_interp_string", fuse_native_interp_string as *const u8);
        let mut module = JITModule::new(builder);
        let hostcalls = HostCalls::declare(&mut module);
        Self {
            _module: module,
            functions: HashMap::new(),
            hostcalls,
        }
    }

    pub(crate) fn try_call(
        &mut self,
        program: &IrProgram,
        name: &str,
        args: &[Value],
        heap: &mut NativeHeap,
    ) -> Option<Value> {
        if !self.functions.contains_key(name) {
            let func = program.functions.get(name)?;
            let mut param_types = Vec::with_capacity(args.len());
            for arg in args {
                param_types.push(value_kind(arg)?);
            }
            if let Some(compiled) = compile_function(
                &mut self._module,
                &self.hostcalls,
                program,
                name,
                func,
                &param_types,
                heap,
            )
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
                    },
                );
            }
        }
        let compiled = self.functions.get(name)?;
        if args.len() != compiled.arity {
            return None;
        }
        let mut native_args = Vec::with_capacity(args.len());
        for arg in args {
            native_args.push(NativeValue::from_value(arg, heap)?);
        }
        let mut out = NativeValue::null();
        // SAFETY: Function pointer is JIT-compiled with matching signature and argument layout.
        let status = unsafe { (compiled.entry)(native_args.as_ptr(), &mut out, heap) };
        if status != 0 {
            return None;
        }
        out.to_value(heap)
    }

    pub(crate) fn has_function(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }

}

impl HostCalls {
    fn declare(module: &mut JITModule) -> Self {
        let pointer_ty = module.target_config().pointer_type();
        let mut list_sig = module.make_signature();
        list_sig.params.push(AbiParam::new(pointer_ty));
        list_sig.params.push(AbiParam::new(pointer_ty));
        list_sig.params.push(AbiParam::new(types::I64));
        list_sig.returns.push(AbiParam::new(types::I64));
        let make_list = module
            .declare_function("fuse_native_make_list", Linkage::Import, &list_sig)
            .expect("declare make_list hostcall");

        let mut map_sig = module.make_signature();
        map_sig.params.push(AbiParam::new(pointer_ty));
        map_sig.params.push(AbiParam::new(pointer_ty));
        map_sig.params.push(AbiParam::new(types::I64));
        map_sig.returns.push(AbiParam::new(types::I64));
        let make_map = module
            .declare_function("fuse_native_make_map", Linkage::Import, &map_sig)
            .expect("declare make_map hostcall");

        let mut struct_sig = module.make_signature();
        struct_sig.params.push(AbiParam::new(pointer_ty));
        struct_sig.params.push(AbiParam::new(types::I64));
        struct_sig.params.push(AbiParam::new(pointer_ty));
        struct_sig.params.push(AbiParam::new(types::I64));
        struct_sig.returns.push(AbiParam::new(types::I64));
        let make_struct = module
            .declare_function("fuse_native_make_struct", Linkage::Import, &struct_sig)
            .expect("declare make_struct hostcall");

        let mut get_field_sig = module.make_signature();
        get_field_sig.params.push(AbiParam::new(pointer_ty));
        get_field_sig.params.push(AbiParam::new(types::I64));
        get_field_sig.params.push(AbiParam::new(types::I64));
        get_field_sig.returns.push(AbiParam::new(types::I64));
        let get_struct_field = module
            .declare_function(
                "fuse_native_get_struct_field",
                Linkage::Import,
                &get_field_sig,
            )
            .expect("declare get_struct_field hostcall");

        let mut enum_sig = module.make_signature();
        enum_sig.params.push(AbiParam::new(pointer_ty));
        enum_sig.params.push(AbiParam::new(types::I64));
        enum_sig.params.push(AbiParam::new(types::I64));
        enum_sig.params.push(AbiParam::new(pointer_ty));
        enum_sig.params.push(AbiParam::new(types::I64));
        enum_sig.returns.push(AbiParam::new(types::I64));
        let make_enum = module
            .declare_function("fuse_native_make_enum", Linkage::Import, &enum_sig)
            .expect("declare make_enum hostcall");

        let mut box_sig = module.make_signature();
        box_sig.params.push(AbiParam::new(pointer_ty));
        box_sig.params.push(AbiParam::new(pointer_ty));
        box_sig.returns.push(AbiParam::new(types::I64));
        let make_box = module
            .declare_function("fuse_native_make_box", Linkage::Import, &box_sig)
            .expect("declare make_box hostcall");

        let mut interp_sig = module.make_signature();
        interp_sig.params.push(AbiParam::new(pointer_ty));
        interp_sig.params.push(AbiParam::new(pointer_ty));
        interp_sig.params.push(AbiParam::new(types::I64));
        interp_sig.returns.push(AbiParam::new(types::I64));
        let interp_string = module
            .declare_function("fuse_native_interp_string", Linkage::Import, &interp_sig)
            .expect("declare interp_string hostcall");

        Self {
            make_list,
            make_map,
            make_struct,
            get_struct_field,
            make_enum,
            make_box,
            interp_string,
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_make_list(
    heap: *mut NativeHeap,
    values: *const NativeValue,
    len: u64,
) -> u64 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return u64::MAX;
    };
    let count = len as usize;
    let slice = unsafe { std::slice::from_raw_parts(values, count) };
    let items = slice.to_vec();
    heap.insert(HeapValue::List(items))
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_make_map(
    heap: *mut NativeHeap,
    pairs: *const NativeValue,
    len: u64,
) -> u64 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return u64::MAX;
    };
    let count = len as usize;
    let slice = unsafe { std::slice::from_raw_parts(pairs, count.saturating_mul(2)) };
    let mut map = HashMap::new();
    for idx in 0..count {
        let key = slice[idx * 2];
        let value = slice[idx * 2 + 1];
        if key.tag != NativeTag::Heap {
            return u64::MAX;
        }
        let Some(heap_value) = heap.get(key.payload) else {
            return u64::MAX;
        };
        let HeapValue::String(text) = heap_value else {
            return u64::MAX;
        };
        map.insert(text.clone(), value);
    }
    heap.insert(HeapValue::Map(map))
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_make_struct(
    heap: *mut NativeHeap,
    name_handle: u64,
    pairs: *const NativeValue,
    len: u64,
) -> u64 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return u64::MAX;
    };
    let Some(name_value) = heap.get(name_handle) else {
        return u64::MAX;
    };
    let HeapValue::String(name) = name_value else {
        return u64::MAX;
    };
    let count = len as usize;
    let slice = unsafe { std::slice::from_raw_parts(pairs, count.saturating_mul(2)) };
    let mut fields = HashMap::new();
    for idx in 0..count {
        let key = slice[idx * 2];
        let value = slice[idx * 2 + 1];
        if key.tag != NativeTag::Heap {
            return u64::MAX;
        }
        let Some(heap_value) = heap.get(key.payload) else {
            return u64::MAX;
        };
        let HeapValue::String(text) = heap_value else {
            return u64::MAX;
        };
        fields.insert(text.clone(), value);
    }
    heap.insert(HeapValue::Struct {
        name: name.clone(),
        fields,
    })
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_get_struct_field(
    heap: *mut NativeHeap,
    struct_handle: u64,
    field_handle: u64,
) -> u64 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return u64::MAX;
    };
    let Some(field_value) = heap.get(field_handle) else {
        return u64::MAX;
    };
    let HeapValue::String(field) = field_value else {
        return u64::MAX;
    };
    let Some(struct_value) = heap.get(struct_handle) else {
        return u64::MAX;
    };
    let HeapValue::Struct { fields, .. } = struct_value else {
        return u64::MAX;
    };
    let Some(value) = fields.get(field) else {
        return u64::MAX;
    };
    if value.tag != NativeTag::Heap {
        return u64::MAX;
    }
    value.payload
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_make_enum(
    heap: *mut NativeHeap,
    name_handle: u64,
    variant_handle: u64,
    payload: *const NativeValue,
    len: u64,
) -> u64 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return u64::MAX;
    };
    let Some(name_value) = heap.get(name_handle) else {
        return u64::MAX;
    };
    let HeapValue::String(name) = name_value else {
        return u64::MAX;
    };
    let Some(variant_value) = heap.get(variant_handle) else {
        return u64::MAX;
    };
    let HeapValue::String(variant) = variant_value else {
        return u64::MAX;
    };
    let count = len as usize;
    let slice = unsafe { std::slice::from_raw_parts(payload, count) };
    let items = slice.to_vec();
    heap.insert(HeapValue::Enum {
        name: name.clone(),
        variant: variant.clone(),
        payload: items,
    })
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_make_box(heap: *mut NativeHeap, value: *const NativeValue) -> u64 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return u64::MAX;
    };
    let value = unsafe { value.as_ref() };
    let Some(value) = value else {
        return u64::MAX;
    };
    heap.insert(HeapValue::Boxed(*value))
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_interp_string(
    heap: *mut NativeHeap,
    parts: *const NativeValue,
    len: u64,
) -> u64 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return u64::MAX;
    };
    let count = len as usize;
    let slice = unsafe { std::slice::from_raw_parts(parts, count) };
    let mut out = String::new();
    for value in slice {
        let Some(part) = value.to_value(heap) else {
            return u64::MAX;
        };
        out.push_str(&part.to_string_value());
    }
    heap.insert(HeapValue::String(out))
}

fn compile_function(
    module: &mut JITModule,
    hostcalls: &HostCalls,
    program: &IrProgram,
    name: &str,
    func: &Function,
    param_types: &[JitType],
    heap: &mut NativeHeap,
) -> Option<PendingCompiled> {
    let ret = return_kind(func.ret.as_ref()?, program)?;
    if func.code.is_empty() || func.params.len() != param_types.len() {
        return None;
    }

    let starts = block_starts(&func.code)?;
    let local_types = infer_local_types(func, param_types, &starts, program)?;
    let mut ctx = module.make_context();
    let pointer_ty = module.target_config().pointer_type();
    ctx.func.signature.params.push(AbiParam::new(pointer_ty));
    ctx.func.signature.params.push(AbiParam::new(pointer_ty));
    ctx.func.signature.params.push(AbiParam::new(pointer_ty));
    ctx.func.signature.returns.push(AbiParam::new(types::I8));
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
    let out_ptr = builder.block_params(blocks[0])[1];
    let heap_ptr = builder.block_params(blocks[0])[2];

    let mut locals = Vec::with_capacity(func.locals);
    for slot in 0..func.locals {
        let var = Variable::from_u32(u32::try_from(slot).ok()?);
        let local_ty = clif_type(local_types.get(slot)?);
        builder.declare_var(var, local_ty);
        let init = if slot < func.params.len() {
            let slot = i32::try_from(slot).ok()?;
            let offset = slot
                .checked_mul(NATIVE_VALUE_SIZE)?
                .checked_add(NATIVE_VALUE_PAYLOAD_OFFSET)?;
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
                Instr::Push(Const::String(value)) => {
                    let handle = NativeValue::intern_string(value.clone(), heap).payload;
                    stack.push(StackValue {
                        value: builder.ins().iconst(types::I64, handle as i64),
                        kind: JitType::Heap,
                    });
                }
                Instr::MakeList { len } => {
                    let mut items = Vec::with_capacity(*len);
                    for _ in 0..*len {
                        items.push(stack.pop()?);
                    }
                    items.reverse();
                    let count = u32::try_from(items.len()).ok()?;
                    let base = if count == 0 {
                        builder.ins().iconst(pointer_ty, 0)
                    } else {
                        let slot_size = count.checked_mul(NATIVE_VALUE_SIZE as u32)?;
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_size,
                        ));
                        let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                        for (idx, item) in items.into_iter().enumerate() {
                            let offset = i32::try_from(idx).ok()?.checked_mul(NATIVE_VALUE_SIZE)?;
                            store_native_value(&mut builder, base, offset, item)?;
                        }
                        base
                    };
                    let len_val = builder.ins().iconst(types::I64, count as i64);
                    let func_ref = module.declare_func_in_func(hostcalls.make_list, builder.func);
                    let call = builder.ins().call(func_ref, &[heap_ptr, base, len_val]);
                    let handle = builder.inst_results(call)[0];
                    stack.push(StackValue {
                        value: handle,
                        kind: JitType::Heap,
                    });
                }
                Instr::MakeMap { len } => {
                    let mut pairs = Vec::with_capacity(*len);
                    for _ in 0..*len {
                        let value = stack.pop()?;
                        let key = stack.pop()?;
                        pairs.push((key, value));
                    }
                    pairs.reverse();
                    let count = u32::try_from(pairs.len()).ok()?;
                    let base = if count == 0 {
                        builder.ins().iconst(pointer_ty, 0)
                    } else {
                        let slot_size = count
                            .checked_mul(2)?
                            .checked_mul(NATIVE_VALUE_SIZE as u32)?;
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_size,
                        ));
                        let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                        for (idx, (key, value)) in pairs.into_iter().enumerate() {
                            let pair_idx = i32::try_from(idx).ok()?.checked_mul(2)?;
                            let key_offset = pair_idx.checked_mul(NATIVE_VALUE_SIZE)?;
                            let value_offset = key_offset.checked_add(NATIVE_VALUE_SIZE)?;
                            store_native_value(&mut builder, base, key_offset, key)?;
                            store_native_value(&mut builder, base, value_offset, value)?;
                        }
                        base
                    };
                    let len_val = builder.ins().iconst(types::I64, count as i64);
                    let func_ref = module.declare_func_in_func(hostcalls.make_map, builder.func);
                    let call = builder.ins().call(func_ref, &[heap_ptr, base, len_val]);
                    let handle = builder.inst_results(call)[0];
                    stack.push(StackValue {
                        value: handle,
                        kind: JitType::Heap,
                    });
                }
                Instr::MakeStruct { name, fields } => {
                    let mut values = Vec::with_capacity(fields.len());
                    for _ in 0..fields.len() {
                        values.push(stack.pop()?);
                    }
                    values.reverse();
                    let name_handle = NativeValue::intern_string(name.clone(), heap).payload;
                    let field_handles: Vec<u64> = fields
                        .iter()
                        .map(|field| NativeValue::intern_string(field.clone(), heap).payload)
                        .collect();
                    let count = u32::try_from(field_handles.len()).ok()?;
                    let base = if count == 0 {
                        builder.ins().iconst(pointer_ty, 0)
                    } else {
                        let slot_size = count
                            .checked_mul(2)?
                            .checked_mul(NATIVE_VALUE_SIZE as u32)?;
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_size,
                        ));
                        let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                        for (idx, (field_handle, value)) in field_handles
                            .into_iter()
                            .zip(values.into_iter())
                            .enumerate()
                        {
                            let pair_idx = i32::try_from(idx).ok()?.checked_mul(2)?;
                            let key_offset = pair_idx.checked_mul(NATIVE_VALUE_SIZE)?;
                            let value_offset = key_offset.checked_add(NATIVE_VALUE_SIZE)?;
                            let key = StackValue {
                                value: builder.ins().iconst(types::I64, field_handle as i64),
                                kind: JitType::Heap,
                            };
                            store_native_value(&mut builder, base, key_offset, key)?;
                            store_native_value(&mut builder, base, value_offset, value)?;
                        }
                        base
                    };
                    let name_val = builder.ins().iconst(types::I64, name_handle as i64);
                    let len_val = builder.ins().iconst(types::I64, count as i64);
                    let func_ref =
                        module.declare_func_in_func(hostcalls.make_struct, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, name_val, base, len_val]);
                    let handle = builder.inst_results(call)[0];
                    stack.push(StackValue {
                        value: handle,
                        kind: JitType::Struct,
                    });
                }
                Instr::MakeEnum { name, variant, argc } => {
                    let mut payload = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        payload.push(stack.pop()?);
                    }
                    payload.reverse();
                    let name_handle = NativeValue::intern_string(name.clone(), heap).payload;
                    let variant_handle = NativeValue::intern_string(variant.clone(), heap).payload;
                    let count = u32::try_from(payload.len()).ok()?;
                    let base = if count == 0 {
                        builder.ins().iconst(pointer_ty, 0)
                    } else {
                        let slot_size = count.checked_mul(NATIVE_VALUE_SIZE as u32)?;
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_size,
                        ));
                        let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                        for (idx, item) in payload.into_iter().enumerate() {
                            let offset = i32::try_from(idx).ok()?.checked_mul(NATIVE_VALUE_SIZE)?;
                            store_native_value(&mut builder, base, offset, item)?;
                        }
                        base
                    };
                    let name_val = builder.ins().iconst(types::I64, name_handle as i64);
                    let variant_val = builder.ins().iconst(types::I64, variant_handle as i64);
                    let len_val = builder.ins().iconst(types::I64, count as i64);
                    let func_ref =
                        module.declare_func_in_func(hostcalls.make_enum, builder.func);
                    let call = builder.ins().call(
                        func_ref,
                        &[heap_ptr, name_val, variant_val, base, len_val],
                    );
                    let handle = builder.inst_results(call)[0];
                    stack.push(StackValue {
                        value: handle,
                        kind: JitType::Enum,
                    });
                }
                Instr::MakeBox => {
                    let value = stack.pop()?;
                    if value.kind == JitType::Boxed {
                        stack.push(value);
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            NATIVE_VALUE_SIZE as u32,
                        ));
                        let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                        store_native_value(&mut builder, base, 0, value)?;
                        let func_ref =
                            module.declare_func_in_func(hostcalls.make_box, builder.func);
                        let call = builder.ins().call(func_ref, &[heap_ptr, base]);
                        let handle = builder.inst_results(call)[0];
                        stack.push(StackValue {
                            value: handle,
                            kind: JitType::Boxed,
                        });
                    }
                }
                Instr::InterpString { parts } => {
                    let mut items = Vec::with_capacity(*parts);
                    for _ in 0..*parts {
                        items.push(stack.pop()?);
                    }
                    items.reverse();
                    let count = u32::try_from(items.len()).ok()?;
                    let base = if count == 0 {
                        builder.ins().iconst(pointer_ty, 0)
                    } else {
                        let slot_size = count.checked_mul(NATIVE_VALUE_SIZE as u32)?;
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_size,
                        ));
                        let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                        for (idx, item) in items.into_iter().enumerate() {
                            let offset = i32::try_from(idx).ok()?.checked_mul(NATIVE_VALUE_SIZE)?;
                            store_native_value(&mut builder, base, offset, item)?;
                        }
                        base
                    };
                    let len_val = builder.ins().iconst(types::I64, count as i64);
                    let func_ref =
                        module.declare_func_in_func(hostcalls.interp_string, builder.func);
                    let call = builder.ins().call(func_ref, &[heap_ptr, base, len_val]);
                    let handle = builder.inst_results(call)[0];
                    stack.push(StackValue {
                        value: handle,
                        kind: JitType::Heap,
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
                Instr::GetField { field } => {
                    let base = stack.pop()?;
                    if base.kind != JitType::Struct {
                        return None;
                    }
                    let field_handle = NativeValue::intern_string(field.clone(), heap).payload;
                    let field_val = builder.ins().iconst(types::I64, field_handle as i64);
                    let func_ref =
                        module.declare_func_in_func(hostcalls.get_struct_field, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, base.value, field_val]);
                    let handle = builder.inst_results(call)[0];
                    stack.push(StackValue {
                        value: handle,
                        kind: JitType::Heap,
                    });
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
                    write_native_return(&mut builder, out_ptr, ret, value)?;
                    let status = builder.ins().iconst(types::I8, 0);
                    builder.ins().return_(&[status]);
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
                write_native_return(&mut builder, out_ptr, ret, value)?;
                let status = builder.ins().iconst(types::I8, 0);
                builder.ins().return_(&[status]);
            } else if stack.is_empty() {
                return None;
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

fn write_native_return(
    builder: &mut FunctionBuilder<'_>,
    out_ptr: ClifValue,
    ret: ReturnKind,
    value: StackValue,
) -> Option<()> {
    let (tag, payload) = match (ret, value.kind) {
        (ReturnKind::Int, JitType::Int) => (NativeTag::Int, value.value),
        (ReturnKind::Bool, JitType::Bool) => (NativeTag::Bool, value.value),
        (ReturnKind::Float, JitType::Float) => (
            NativeTag::Float,
            builder
                .ins()
                .bitcast(types::I64, MemFlags::new(), value.value),
        ),
        (ReturnKind::Heap, JitType::Heap | JitType::Struct | JitType::Enum | JitType::Boxed) => {
            (NativeTag::Heap, value.value)
        }
        _ => return None,
    };
    let tag_value = builder.ins().iconst(types::I64, tag as i64);
    builder.ins().store(MemFlags::new(), tag_value, out_ptr, 0);
    builder
        .ins()
        .store(MemFlags::new(), payload, out_ptr, NATIVE_VALUE_PAYLOAD_OFFSET);
    Some(())
}

fn stack_tag(kind: JitType) -> Option<NativeTag> {
    match kind {
        JitType::Int => Some(NativeTag::Int),
        JitType::Bool => Some(NativeTag::Bool),
        JitType::Float => Some(NativeTag::Float),
        JitType::Heap | JitType::Struct | JitType::Enum | JitType::Boxed => Some(NativeTag::Heap),
    }
}

fn stack_payload(builder: &mut FunctionBuilder<'_>, value: StackValue) -> Option<ClifValue> {
    match value.kind {
        JitType::Float => Some(builder.ins().bitcast(
            types::I64,
            MemFlags::new(),
            value.value,
        )),
        JitType::Int
        | JitType::Bool
        | JitType::Heap
        | JitType::Struct
        | JitType::Enum
        | JitType::Boxed => Some(value.value),
    }
}

fn store_native_value(
    builder: &mut FunctionBuilder<'_>,
    base_ptr: ClifValue,
    offset: i32,
    value: StackValue,
) -> Option<()> {
    let tag = stack_tag(value.kind)?;
    let payload = stack_payload(builder, value)?;
    let tag_value = builder.ins().iconst(types::I64, tag as i64);
    builder
        .ins()
        .store(MemFlags::new(), tag_value, base_ptr, offset);
    builder.ins().store(
        MemFlags::new(),
        payload,
        base_ptr,
        offset + NATIVE_VALUE_PAYLOAD_OFFSET,
    );
    Some(())
}

fn return_kind(ty: &TypeRef, program: &IrProgram) -> Option<ReturnKind> {
    match &ty.kind {
        TypeRefKind::Simple(name) if name.name == "Int" => Some(ReturnKind::Int),
        TypeRefKind::Simple(name) if name.name == "Bool" => Some(ReturnKind::Bool),
        TypeRefKind::Simple(name) if name.name == "Float" => Some(ReturnKind::Float),
        TypeRefKind::Simple(name) if name.name == "String" => Some(ReturnKind::Heap),
        TypeRefKind::Simple(name) if program.types.contains_key(&name.name) => {
            Some(ReturnKind::Heap)
        }
        TypeRefKind::Simple(name) if program.enums.contains_key(&name.name) => {
            Some(ReturnKind::Heap)
        }
        TypeRefKind::Refined { base, .. } if base.name == "Int" => Some(ReturnKind::Int),
        TypeRefKind::Refined { base, .. } if base.name == "Float" => Some(ReturnKind::Float),
        TypeRefKind::Refined { base, .. } if base.name == "String" => Some(ReturnKind::Heap),
        TypeRefKind::Refined { base, .. } if program.types.contains_key(&base.name) => {
            Some(ReturnKind::Heap)
        }
        TypeRefKind::Generic { base, .. }
            if base.name == "List" || base.name == "Map" || base.name == "Task" =>
        {
            Some(ReturnKind::Heap)
        }
        _ => None,
    }
}

fn value_kind(value: &Value) -> Option<JitType> {
    match value {
        Value::Int(_) => Some(JitType::Int),
        Value::Bool(_) => Some(JitType::Bool),
        Value::Float(_) => Some(JitType::Float),
        Value::String(_) | Value::List(_) | Value::Map(_) => Some(JitType::Heap),
        Value::Struct { .. } => Some(JitType::Struct),
        Value::Enum { .. } => Some(JitType::Enum),
        Value::Boxed(_) => Some(JitType::Boxed),
        Value::Task(_) => Some(JitType::Heap),
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
    _program: &IrProgram,
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
                Instr::Push(Const::String(_)) => stack.push(JitType::Heap),
                Instr::MakeList { len } => {
                    for _ in 0..*len {
                        stack.pop()?;
                    }
                    stack.push(JitType::Heap);
                }
                Instr::MakeMap { len } => {
                    for _ in 0..*len {
                        let _value = stack.pop()?;
                        let _key = stack.pop()?;
                    }
                    stack.push(JitType::Heap);
                }
                Instr::MakeStruct { fields, .. } => {
                    for _ in 0..fields.len() {
                        stack.pop()?;
                    }
                    stack.push(JitType::Struct);
                }
                Instr::MakeEnum { argc, .. } => {
                    for _ in 0..*argc {
                        stack.pop()?;
                    }
                    stack.push(JitType::Enum);
                }
                Instr::MakeBox => {
                    let value = stack.pop()?;
                    match value {
                        JitType::Boxed => stack.push(JitType::Boxed),
                        _ => stack.push(JitType::Boxed),
                    }
                }
                Instr::InterpString { parts } => {
                    for _ in 0..*parts {
                        stack.pop()?;
                    }
                    stack.push(JitType::Heap);
                }
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
                Instr::GetField { .. } => {
                    let base = stack.pop()?;
                    if base != JitType::Struct {
                        return None;
                    }
                    stack.push(JitType::Heap);
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
