use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use cranelift_codegen::ir::{
    AbiParam,
    BlockArg,
    InstBuilder,
    MemFlags,
    StackSlotData,
    StackSlotKind,
    Value as ClifValue,
    condcodes::{FloatCC, IntCC},
    types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use cranelift_native::builder as native_builder;
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::ast::{BinaryOp, Expr, ExprKind, Literal, PatternKind, TypeRef, TypeRefKind, UnaryOp};
use crate::interp::Value;
use crate::native::value::{HeapValue, NativeHeap, NativeIterator, NativeTag, NativeValue, TaskValue};
use crate::ir::{CallKind, Const, Function, Instr, Program as IrProgram};

use fuse_rt::{config as rt_config, json as rt_json, validate as rt_validate};

type EntryFn = unsafe extern "C" fn(*const NativeValue, *mut NativeValue, *mut NativeHeap) -> u8;

macro_rules! jit_fail {
    ($func:expr, $ip:expr, $instr:expr, $reason:expr) => {{
        if std::env::var("FUSE_NATIVE_DEBUG").is_ok() {
            match ($ip, $instr) {
                (Some(ip), Some(instr)) => eprintln!(
                    "native compile failed in {} at {ip}: {instr:?} ({})",
                    $func.name, $reason
                ),
                _ => eprintln!("native compile failed in {} ({})", $func.name, $reason),
            }
        }
        return None;
    }};
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum JitType {
    Int,
    Bool,
    Float,
    Null,
    Heap,
    Struct,
    Enum,
    Boxed,
    Unit,
    Value,
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
    Value,
}

struct CompiledEntry {
    id: FuncId,
    entry: EntryFn,
    arity: usize,
}

struct PendingCompiled {
    id: FuncId,
    arity: usize,
    key: FuncKey,
}

type FuncKey = (String, Vec<JitType>);

struct CompileState {
    func_ids: HashMap<FuncKey, FuncId>,
    name_param_types: HashMap<String, Vec<JitType>>,
    in_progress: HashSet<FuncKey>,
    compiled: HashSet<FuncKey>,
}

impl CompileState {
    fn new() -> Self {
        Self {
            func_ids: HashMap::new(),
            name_param_types: HashMap::new(),
            in_progress: HashSet::new(),
            compiled: HashSet::new(),
        }
    }

    fn from_compiled(functions: &HashMap<FuncKey, CompiledEntry>) -> Self {
        let mut state = Self::new();
        for (key, entry) in functions {
            state.func_ids.insert(key.clone(), entry.id);
            state
                .name_param_types
                .insert(key.0.clone(), key.1.clone());
            state.compiled.insert(key.clone());
        }
        state
    }

    fn ensure_declared<M: Module>(
        &mut self,
        module: &mut M,
        name: &str,
        param_types: &[JitType],
        linkage: Linkage,
    ) -> Option<(FuncKey, FuncId)> {
        if let Some(existing) = self.name_param_types.get(name) {
            if existing.as_slice() != param_types {
                return None;
            }
        }
        let key = (name.to_string(), param_types.to_vec());
        if let Some(id) = self.func_ids.get(&key) {
            return Some((key, *id));
        }
        let signature = entry_signature(module);
        let symbol = jit_symbol(name);
        let id = module
            .declare_function(&symbol, linkage, &signature)
            .ok()?;
        self.func_ids.insert(key.clone(), id);
        self.name_param_types
            .insert(name.to_string(), param_types.to_vec());
        Some((key, id))
    }
}

pub(crate) struct HostCalls {
    make_list: FuncId,
    make_map: FuncId,
    make_struct: FuncId,
    get_struct_field: FuncId,
    get_index: FuncId,
    set_index: FuncId,
    set_field: FuncId,
    make_enum: FuncId,
    match_enum: FuncId,
    make_box: FuncId,
    interp_string: FuncId,
    bang: FuncId,
    iter_init: FuncId,
    iter_next: FuncId,
    add: FuncId,
    eq: FuncId,
    not_eq: FuncId,
    range: FuncId,
    builtin_log: FuncId,
    builtin_print: FuncId,
    builtin_env: FuncId,
    builtin_assert: FuncId,
    config_get: FuncId,
    task_id: FuncId,
    task_done: FuncId,
    task_cancel: FuncId,
    db_exec: FuncId,
    db_query: FuncId,
    db_one: FuncId,
    db_from: FuncId,
    query_select: FuncId,
    query_where: FuncId,
    query_order_by: FuncId,
    query_limit: FuncId,
    query_one: FuncId,
    query_all: FuncId,
    query_exec: FuncId,
    query_sql: FuncId,
    query_params: FuncId,
    json_encode: FuncId,
    json_decode: FuncId,
    validate_struct: FuncId,
}

pub(crate) struct JitRuntime {
    _module: JITModule,
    functions: HashMap<(String, Vec<JitType>), CompiledEntry>,
    hostcalls: HostCalls,
}

#[derive(Debug)]
pub(crate) enum JitCallError {
    Error(Value),
    Runtime(String),
}

pub(crate) struct ObjectArtifact {
    pub object: Vec<u8>,
    pub interned_strings: Vec<String>,
    pub entry_symbol: String,
}

pub(crate) struct ObjectArtifactSet {
    pub object: Vec<u8>,
    pub interned_strings: Vec<String>,
}

const NATIVE_VALUE_SIZE: i32 = std::mem::size_of::<NativeValue>() as i32;
const NATIVE_VALUE_PAYLOAD_OFFSET: i32 = std::mem::size_of::<NativeTag>() as i32;
const NATIVE_VALUE_ALIGN_SHIFT: u8 = 3;
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
        builder.symbol("fuse_native_get_index", fuse_native_get_index as *const u8);
        builder.symbol("fuse_native_set_index", fuse_native_set_index as *const u8);
        builder.symbol("fuse_native_set_field", fuse_native_set_field as *const u8);
        builder.symbol("fuse_native_make_enum", fuse_native_make_enum as *const u8);
        builder.symbol("fuse_native_match_enum", fuse_native_match_enum as *const u8);
        builder.symbol("fuse_native_make_box", fuse_native_make_box as *const u8);
        builder.symbol("fuse_native_interp_string", fuse_native_interp_string as *const u8);
        builder.symbol("fuse_native_bang", fuse_native_bang as *const u8);
        builder.symbol("fuse_native_iter_init", fuse_native_iter_init as *const u8);
        builder.symbol("fuse_native_iter_next", fuse_native_iter_next as *const u8);
        builder.symbol("fuse_native_add", fuse_native_add as *const u8);
        builder.symbol("fuse_native_eq", fuse_native_eq as *const u8);
        builder.symbol("fuse_native_not_eq", fuse_native_not_eq as *const u8);
        builder.symbol("fuse_native_range", fuse_native_range as *const u8);
        builder.symbol("fuse_native_builtin_log", fuse_native_builtin_log as *const u8);
        builder.symbol("fuse_native_builtin_print", fuse_native_builtin_print as *const u8);
        builder.symbol("fuse_native_builtin_env", fuse_native_builtin_env as *const u8);
        builder.symbol(
            "fuse_native_builtin_assert",
            fuse_native_builtin_assert as *const u8,
        );
        builder.symbol("fuse_native_config_get", fuse_native_config_get as *const u8);
        builder.symbol("fuse_native_task_id", fuse_native_task_id as *const u8);
        builder.symbol("fuse_native_task_done", fuse_native_task_done as *const u8);
        builder.symbol("fuse_native_task_cancel", fuse_native_task_cancel as *const u8);
        builder.symbol("fuse_native_json_encode", fuse_native_json_encode as *const u8);
        builder.symbol("fuse_native_json_decode", fuse_native_json_decode as *const u8);
        builder.symbol(
            "fuse_native_validate_struct",
            fuse_native_validate_struct as *const u8,
        );
        builder.symbol("fuse_native_db_exec", fuse_native_db_exec as *const u8);
        builder.symbol("fuse_native_db_query", fuse_native_db_query as *const u8);
        builder.symbol("fuse_native_db_one", fuse_native_db_one as *const u8);
        builder.symbol("fuse_native_db_from", fuse_native_db_from as *const u8);
        builder.symbol("fuse_native_query_select", fuse_native_query_select as *const u8);
        builder.symbol("fuse_native_query_where", fuse_native_query_where as *const u8);
        builder.symbol("fuse_native_query_order_by", fuse_native_query_order_by as *const u8);
        builder.symbol("fuse_native_query_limit", fuse_native_query_limit as *const u8);
        builder.symbol("fuse_native_query_one", fuse_native_query_one as *const u8);
        builder.symbol("fuse_native_query_all", fuse_native_query_all as *const u8);
        builder.symbol("fuse_native_query_exec", fuse_native_query_exec as *const u8);
        builder.symbol("fuse_native_query_sql", fuse_native_query_sql as *const u8);
        builder.symbol("fuse_native_query_params", fuse_native_query_params as *const u8);
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
    ) -> Option<Result<Value, JitCallError>> {
        let mut param_types = Vec::with_capacity(args.len());
        for arg in args {
            param_types.push(value_kind(arg)?);
        }
        let key = (name.to_string(), param_types.clone());
        if !self.functions.contains_key(&key) {
            let func = program
                .functions
                .get(name)
                .or_else(|| program.apps.get(name))
                .or_else(|| program.apps.values().find(|func| func.name == name))?;
            let mut state = CompileState::from_compiled(&self.functions);
            let mut pending = Vec::new();
            if compile_function(
                &mut self._module,
                &self.hostcalls,
                program,
                name,
                func,
                &param_types,
                Linkage::Local,
                heap,
                &mut state,
                &mut pending,
            )
            .is_some()
            {
                if !pending.is_empty() {
                    if self._module.finalize_definitions().is_err() {
                        return None;
                    }
                    for compiled in pending {
                        if self.functions.contains_key(&compiled.key) {
                            continue;
                        }
                        let raw = self._module.get_finalized_function(compiled.id);
                        // SAFETY: The JIT function is declared with matching signature.
                        let entry = unsafe { std::mem::transmute::<*const u8, EntryFn>(raw) };
                        self.functions.insert(
                            compiled.key,
                            CompiledEntry {
                                id: compiled.id,
                                entry,
                                arity: compiled.arity,
                            },
                        );
                    }
                }
            }
        }
        let compiled = self.functions.get(&key)?;
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
        match status {
            0 => out.to_value(heap).map(Ok),
            1 => out.to_value(heap).map(|value| Err(JitCallError::Error(value))),
            2 => {
                let value = out.to_value(heap)?;
                let message = value.to_string_value();
                Some(Err(JitCallError::Runtime(message)))
            }
            _ => None,
        }
    }

    pub(crate) fn has_function(&self, name: &str) -> bool {
        self.functions.keys().any(|(func_name, _)| func_name == name)
    }

}

pub(crate) fn emit_object_for_function(
    program: &IrProgram,
    func: &Function,
) -> Result<ObjectArtifact, String> {
    if !func.params.is_empty() {
        return Err("object emission requires zero-arg functions".to_string());
    }
    let mut flag_builder = settings::builder();
    flag_builder
        .set("is_pic", "true")
        .map_err(|err| format!("cranelift settings error: {err}"))?;
    let isa_builder = native_builder().map_err(|err| format!("native isa error: {err}"))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|err| format!("native isa error: {err}"))?;
    let builder = ObjectBuilder::new(isa, "fuse_native", default_libcall_names())
        .map_err(|err| format!("object builder error: {err}"))?;
    let mut module = ObjectModule::new(builder);
    let hostcalls = HostCalls::declare(&mut module);
    let mut heap = NativeHeap::new();
    let param_types: Vec<JitType> = Vec::new();
    let mut state = CompileState::new();
    let mut pending = Vec::new();
    compile_function(
        &mut module,
        &hostcalls,
        program,
        &func.name,
        func,
        &param_types,
        Linkage::Export,
        &mut heap,
        &mut state,
        &mut pending,
    )
    .ok_or_else(|| "native backend could not compile function".to_string())?;
    let product = module.finish();
    let object = product
        .emit()
        .map_err(|err| format!("object emit error: {err}"))?;
    let interned_strings = heap.interned_strings_in_order();
    Ok(ObjectArtifact {
        object,
        interned_strings,
        entry_symbol: jit_symbol(&func.name),
    })
}

pub(crate) fn emit_object_for_functions(
    program: &IrProgram,
    funcs: &[&Function],
) -> Result<ObjectArtifactSet, String> {
    if funcs.is_empty() {
        return Err("object emission requires at least one function".to_string());
    }
    for func in funcs {
        if !func.params.is_empty() {
            return Err("object emission requires zero-arg functions".to_string());
        }
    }
    let mut flag_builder = settings::builder();
    flag_builder
        .set("is_pic", "true")
        .map_err(|err| format!("cranelift settings error: {err}"))?;
    let isa_builder = native_builder().map_err(|err| format!("native isa error: {err}"))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|err| format!("native isa error: {err}"))?;
    let builder = ObjectBuilder::new(isa, "fuse_native", default_libcall_names())
        .map_err(|err| format!("object builder error: {err}"))?;
    let mut module = ObjectModule::new(builder);
    let hostcalls = HostCalls::declare(&mut module);
    let mut heap = NativeHeap::new();
    let param_types: Vec<JitType> = Vec::new();
    let mut state = CompileState::new();
    let mut pending = Vec::new();
    let mut seen = HashSet::new();
    for func in funcs {
        if !seen.insert(func.name.clone()) {
            continue;
        }
        let compiled = compile_function(
            &mut module,
            &hostcalls,
            program,
            &func.name,
            func,
            &param_types,
            Linkage::Export,
            &mut heap,
            &mut state,
            &mut pending,
        );
        if compiled.is_none() {
            return Err(format!(
                "native backend could not compile function {}",
                func.name
            ));
        }
    }
    let product = module.finish();
    let object = product
        .emit()
        .map_err(|err| format!("object emit error: {err}"))?;
    let interned_strings = heap.interned_strings_in_order();
    Ok(ObjectArtifactSet {
        object,
        interned_strings,
    })
}

impl HostCalls {
    fn declare<M: Module>(module: &mut M) -> Self {
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

        let mut match_sig = module.make_signature();
        match_sig.params.push(AbiParam::new(pointer_ty));
        match_sig.params.push(AbiParam::new(types::I64));
        match_sig.params.push(AbiParam::new(types::I64));
        match_sig.params.push(AbiParam::new(pointer_ty));
        match_sig.params.push(AbiParam::new(types::I64));
        match_sig.returns.push(AbiParam::new(types::I64));
        let match_enum = module
            .declare_function("fuse_native_match_enum", Linkage::Import, &match_sig)
            .expect("declare match_enum hostcall");

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

        let mut bang_sig = module.make_signature();
        bang_sig.params.push(AbiParam::new(pointer_ty));
        bang_sig.params.push(AbiParam::new(pointer_ty));
        bang_sig.params.push(AbiParam::new(pointer_ty));
        bang_sig.params.push(AbiParam::new(types::I64));
        bang_sig.params.push(AbiParam::new(pointer_ty));
        bang_sig.returns.push(AbiParam::new(types::I8));
        let bang = module
            .declare_function("fuse_native_bang", Linkage::Import, &bang_sig)
            .expect("declare bang hostcall");

        let mut iter_sig = module.make_signature();
        iter_sig.params.push(AbiParam::new(pointer_ty));
        iter_sig.params.push(AbiParam::new(pointer_ty));
        iter_sig.params.push(AbiParam::new(pointer_ty));
        iter_sig.returns.push(AbiParam::new(types::I8));
        let iter_init = module
            .declare_function("fuse_native_iter_init", Linkage::Import, &iter_sig)
            .expect("declare iter init hostcall");
        let iter_next = module
            .declare_function("fuse_native_iter_next", Linkage::Import, &iter_sig)
            .expect("declare iter next hostcall");

        let mut builtin_sig = module.make_signature();
        builtin_sig.params.push(AbiParam::new(pointer_ty));
        builtin_sig.params.push(AbiParam::new(pointer_ty));
        builtin_sig.params.push(AbiParam::new(types::I64));
        builtin_sig.params.push(AbiParam::new(pointer_ty));
        builtin_sig.returns.push(AbiParam::new(types::I8));
        let add = module
            .declare_function("fuse_native_add", Linkage::Import, &builtin_sig)
            .expect("declare add hostcall");
        let eq = module
            .declare_function("fuse_native_eq", Linkage::Import, &builtin_sig)
            .expect("declare eq hostcall");
        let not_eq = module
            .declare_function("fuse_native_not_eq", Linkage::Import, &builtin_sig)
            .expect("declare not_eq hostcall");
        let get_index = module
            .declare_function("fuse_native_get_index", Linkage::Import, &builtin_sig)
            .expect("declare get_index hostcall");
        let set_index = module
            .declare_function("fuse_native_set_index", Linkage::Import, &builtin_sig)
            .expect("declare set_index hostcall");
        let set_field = module
            .declare_function("fuse_native_set_field", Linkage::Import, &builtin_sig)
            .expect("declare set_field hostcall");
        let range = module
            .declare_function("fuse_native_range", Linkage::Import, &builtin_sig)
            .expect("declare range hostcall");
        let builtin_log = module
            .declare_function("fuse_native_builtin_log", Linkage::Import, &builtin_sig)
            .expect("declare builtin log hostcall");
        let builtin_print = module
            .declare_function("fuse_native_builtin_print", Linkage::Import, &builtin_sig)
            .expect("declare builtin print hostcall");
        let builtin_env = module
            .declare_function("fuse_native_builtin_env", Linkage::Import, &builtin_sig)
            .expect("declare builtin env hostcall");
        let builtin_assert = module
            .declare_function("fuse_native_builtin_assert", Linkage::Import, &builtin_sig)
            .expect("declare builtin assert hostcall");
        let config_get = module
            .declare_function("fuse_native_config_get", Linkage::Import, &builtin_sig)
            .expect("declare config get hostcall");
        let task_id = module
            .declare_function("fuse_native_task_id", Linkage::Import, &builtin_sig)
            .expect("declare task id hostcall");
        let task_done = module
            .declare_function("fuse_native_task_done", Linkage::Import, &builtin_sig)
            .expect("declare task done hostcall");
        let task_cancel = module
            .declare_function("fuse_native_task_cancel", Linkage::Import, &builtin_sig)
            .expect("declare task cancel hostcall");
        let db_exec = module
            .declare_function("fuse_native_db_exec", Linkage::Import, &builtin_sig)
            .expect("declare db exec hostcall");
        let db_query = module
            .declare_function("fuse_native_db_query", Linkage::Import, &builtin_sig)
            .expect("declare db query hostcall");
        let db_one = module
            .declare_function("fuse_native_db_one", Linkage::Import, &builtin_sig)
            .expect("declare db one hostcall");
        let db_from = module
            .declare_function("fuse_native_db_from", Linkage::Import, &builtin_sig)
            .expect("declare db from hostcall");
        let query_select = module
            .declare_function("fuse_native_query_select", Linkage::Import, &builtin_sig)
            .expect("declare query select hostcall");
        let query_where = module
            .declare_function("fuse_native_query_where", Linkage::Import, &builtin_sig)
            .expect("declare query where hostcall");
        let query_order_by = module
            .declare_function("fuse_native_query_order_by", Linkage::Import, &builtin_sig)
            .expect("declare query order_by hostcall");
        let query_limit = module
            .declare_function("fuse_native_query_limit", Linkage::Import, &builtin_sig)
            .expect("declare query limit hostcall");
        let query_one = module
            .declare_function("fuse_native_query_one", Linkage::Import, &builtin_sig)
            .expect("declare query one hostcall");
        let query_all = module
            .declare_function("fuse_native_query_all", Linkage::Import, &builtin_sig)
            .expect("declare query all hostcall");
        let query_exec = module
            .declare_function("fuse_native_query_exec", Linkage::Import, &builtin_sig)
            .expect("declare query exec hostcall");
        let query_sql = module
            .declare_function("fuse_native_query_sql", Linkage::Import, &builtin_sig)
            .expect("declare query sql hostcall");
        let query_params = module
            .declare_function("fuse_native_query_params", Linkage::Import, &builtin_sig)
            .expect("declare query params hostcall");
        let json_encode = module
            .declare_function("fuse_native_json_encode", Linkage::Import, &builtin_sig)
            .expect("declare json encode hostcall");
        let json_decode = module
            .declare_function("fuse_native_json_decode", Linkage::Import, &builtin_sig)
            .expect("declare json decode hostcall");
        let mut validate_sig = module.make_signature();
        validate_sig.params.push(AbiParam::new(pointer_ty));
        validate_sig.params.push(AbiParam::new(types::I64));
        validate_sig.params.push(AbiParam::new(pointer_ty));
        validate_sig.params.push(AbiParam::new(types::I64));
        validate_sig.params.push(AbiParam::new(pointer_ty));
        validate_sig.returns.push(AbiParam::new(types::I8));
        let validate_struct = module
            .declare_function("fuse_native_validate_struct", Linkage::Import, &validate_sig)
            .expect("declare validate struct hostcall");

        Self {
            make_list,
            make_map,
            make_struct,
            get_struct_field,
            get_index,
            set_index,
            set_field,
            make_enum,
            match_enum,
            make_box,
            interp_string,
            bang,
            iter_init,
            iter_next,
            add,
            eq,
            not_eq,
            range,
            builtin_log,
            builtin_print,
            builtin_env,
            builtin_assert,
            config_get,
            task_id,
            task_done,
            task_cancel,
            db_exec,
            db_query,
            db_one,
            db_from,
            query_select,
            query_where,
            query_order_by,
            query_limit,
            query_one,
            query_all,
            query_exec,
            query_sql,
            query_params,
            json_encode,
            json_decode,
            validate_struct,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn label(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    fn json_label(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

fn parse_log_level(raw: &str) -> Option<LogLevel> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(LogLevel::Trace),
        "debug" => Some(LogLevel::Debug),
        "info" => Some(LogLevel::Info),
        "warn" | "warning" => Some(LogLevel::Warn),
        "error" => Some(LogLevel::Error),
        _ => None,
    }
}

fn min_log_level() -> LogLevel {
    std::env::var("FUSE_LOG")
        .ok()
        .and_then(|raw| parse_log_level(&raw))
        .unwrap_or(LogLevel::Info)
}

fn value_to_json(value: &Value) -> rt_json::JsonValue {
    match value.unboxed() {
        Value::Unit => rt_json::JsonValue::Null,
        Value::Int(v) => rt_json::JsonValue::Number(v as f64),
        Value::Float(v) => rt_json::JsonValue::Number(v),
        Value::Bool(v) => rt_json::JsonValue::Bool(v),
        Value::String(v) => rt_json::JsonValue::String(v.clone()),
        Value::Null => rt_json::JsonValue::Null,
        Value::List(items) => {
            rt_json::JsonValue::Array(items.iter().map(|v| value_to_json(v)).collect())
        }
        Value::Map(items) => {
            let mut out = BTreeMap::new();
            for (key, value) in items {
                out.insert(key.clone(), value_to_json(&value));
            }
            rt_json::JsonValue::Object(out)
        }
        Value::Boxed(_) => rt_json::JsonValue::String("<box>".to_string()),
        Value::Query(_) => rt_json::JsonValue::String("<query>".to_string()),
        Value::Task(_) => rt_json::JsonValue::String("<task>".to_string()),
        Value::Iterator(_) => rt_json::JsonValue::String("<iterator>".to_string()),
        Value::Struct { fields, .. } => {
            let mut out = BTreeMap::new();
            for (key, value) in fields {
                out.insert(key.clone(), value_to_json(&value));
            }
            rt_json::JsonValue::Object(out)
        }
        Value::Enum {
            variant, payload, ..
        } => {
            let mut out = BTreeMap::new();
            out.insert(
                "type".to_string(),
                rt_json::JsonValue::String(variant.clone()),
            );
            match payload.len() {
                0 => {}
                1 => {
                    out.insert("data".to_string(), value_to_json(&payload[0]));
                }
                _ => {
                    let items = payload.iter().map(|v| value_to_json(v)).collect();
                    out.insert("data".to_string(), rt_json::JsonValue::Array(items));
                }
            }
            rt_json::JsonValue::Object(out)
        }
        Value::ResultOk(value) => value_to_json(value.as_ref()),
        Value::ResultErr(value) => value_to_json(value.as_ref()),
        Value::Config(name) => rt_json::JsonValue::String(name.clone()),
        Value::Function(name) => rt_json::JsonValue::String(name.clone()),
        Value::Builtin(name) => rt_json::JsonValue::String(name.clone()),
        Value::EnumCtor { name, variant } => {
            rt_json::JsonValue::String(format!("{name}.{variant}"))
        }
    }
}

fn json_to_value(json: &rt_json::JsonValue) -> Value {
    match json {
        rt_json::JsonValue::Null => Value::Null,
        rt_json::JsonValue::Bool(v) => Value::Bool(*v),
        rt_json::JsonValue::Number(n) => {
            if n.fract() == 0.0 {
                Value::Int(*n as i64)
            } else {
                Value::Float(*n)
            }
        }
        rt_json::JsonValue::String(v) => Value::String(v.clone()),
        rt_json::JsonValue::Array(items) => {
            Value::List(items.iter().map(|item| json_to_value(item)).collect())
        }
        rt_json::JsonValue::Object(items) => {
            let mut out = HashMap::new();
            for (key, value) in items {
                out.insert(key.clone(), json_to_value(value));
            }
            Value::Map(out)
        }
    }
}

fn split_type_name(name: &str) -> (Option<&str>, &str) {
    if name.starts_with("std.") {
        return (None, name);
    }
    match name.split_once('.') {
        Some((module, rest)) if !module.is_empty() && !rest.is_empty() => (Some(module), rest),
        _ => (None, name),
    }
}

fn value_type_name(value: &Value) -> String {
    match value.unboxed() {
        Value::Unit => "Unit".to_string(),
        Value::Int(_) => "Int".to_string(),
        Value::Float(_) => "Float".to_string(),
        Value::Bool(_) => "Bool".to_string(),
        Value::String(_) => "String".to_string(),
        Value::Null => "Null".to_string(),
        Value::List(_) => "List".to_string(),
        Value::Map(_) => "Map".to_string(),
        Value::Task(_) => "Task".to_string(),
        Value::Iterator(_) => "Iterator".to_string(),
        Value::Struct { name, .. } => name.clone(),
        Value::Enum { name, .. } => name.clone(),
        Value::EnumCtor { name, .. } => name.clone(),
        Value::ResultOk(_) | Value::ResultErr(_) => "Result".to_string(),
        Value::Config(_) => "Config".to_string(),
        Value::Function(_) => "Function".to_string(),
        Value::Builtin(_) => "Builtin".to_string(),
        Value::Boxed(_) => "Box".to_string(),
        Value::Query(_) => "Query".to_string(),
    }
}

fn validation_field_value(path: &str, code: &str, message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::String(path.to_string()));
    fields.insert("code".to_string(), Value::String(code.to_string()));
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Struct {
        name: "ValidationField".to_string(),
        fields,
    }
}

fn validation_error_value(path: &str, code: &str, message: impl Into<String>) -> Value {
    let field = validation_field_value(path, code, message);
    let mut fields = HashMap::new();
    fields.insert(
        "message".to_string(),
        Value::String("validation failed".to_string()),
    );
    fields.insert("fields".to_string(), Value::List(vec![field]));
    Value::Struct {
        name: "std.Error.Validation".to_string(),
        fields,
    }
}

enum ValidateResult {
    Ok,
    Error(Value),
    Runtime(String),
}

fn validate_value(value: &Value, ty: &TypeRef, path: &str) -> ValidateResult {
    let value = value.unboxed();
    match &ty.kind {
        TypeRefKind::Optional(inner) => {
            if matches!(value, Value::Null) {
                ValidateResult::Ok
            } else {
                validate_value(&value, inner, path)
            }
        }
        TypeRefKind::Result { ok, err } => match value {
            Value::ResultOk(inner) => validate_value(&inner, ok, path),
            Value::ResultErr(inner) => {
                if let Some(err_ty) = err {
                    validate_value(&inner, err_ty, path)
                } else {
                    ValidateResult::Ok
                }
            }
            _ => ValidateResult::Error(validation_error_value(
                path,
                "type_mismatch",
                format!("expected Result, got {}", value_type_name(&value)),
            )),
        },
        TypeRefKind::Refined { base, args } => {
            match validate_simple(&value, &base.name, path) {
                ValidateResult::Ok => check_refined(&value, &base.name, args, path),
                other => other,
            }
        }
        TypeRefKind::Simple(ident) => validate_simple(&value, &ident.name, path),
        TypeRefKind::Generic { base, args } => match base.name.as_str() {
            "Option" => {
                if args.len() != 1 {
                    return ValidateResult::Runtime("Option expects 1 type argument".to_string());
                }
                if matches!(value, Value::Null) {
                    ValidateResult::Ok
                } else {
                    validate_value(&value, &args[0], path)
                }
            }
            "Result" => {
                if args.len() != 2 {
                    return ValidateResult::Runtime("Result expects 2 type arguments".to_string());
                }
                match value {
                    Value::ResultOk(inner) => validate_value(&inner, &args[0], path),
                    Value::ResultErr(inner) => validate_value(&inner, &args[1], path),
                    _ => ValidateResult::Error(validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Result, got {}", value_type_name(&value)),
                    )),
                }
            }
            "List" => {
                if args.len() != 1 {
                    return ValidateResult::Runtime("List expects 1 type argument".to_string());
                }
                match value {
                    Value::List(items) => {
                        for (idx, item) in items.iter().enumerate() {
                            let item_path = format!("{path}[{idx}]");
                            match validate_value(item, &args[0], &item_path) {
                                ValidateResult::Ok => {}
                                other => return other,
                            }
                        }
                        ValidateResult::Ok
                    }
                    _ => ValidateResult::Error(validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected List, got {}", value_type_name(&value)),
                    )),
                }
            }
            "Map" => {
                if args.len() != 2 {
                    return ValidateResult::Runtime("Map expects 2 type arguments".to_string());
                }
                match value {
                    Value::Map(items) => {
                        for (key, val) in items.iter() {
                            let key_value = Value::String(key.clone());
                            let key_path = format!("{path}.{key}");
                            match validate_value(&key_value, &args[0], &key_path) {
                                ValidateResult::Ok => {}
                                other => return other,
                            }
                            match validate_value(val, &args[1], &key_path) {
                                ValidateResult::Ok => {}
                                other => return other,
                            }
                        }
                        ValidateResult::Ok
                    }
                    _ => ValidateResult::Error(validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Map, got {}", value_type_name(&value)),
                    )),
                }
            }
            _ => ValidateResult::Runtime(format!(
                "validation not supported for {}",
                base.name
            )),
        },
    }
}

fn validate_simple(value: &Value, name: &str, path: &str) -> ValidateResult {
    let value = value.unboxed();
    let type_name = value_type_name(&value);
    let (module, simple_name) = split_type_name(name);
    if module.is_none() {
        match simple_name {
            "Int" => {
                if matches!(value, Value::Int(_)) {
                    return ValidateResult::Ok;
                }
                return ValidateResult::Error(validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Int, got {type_name}"),
                ));
            }
            "Float" => {
                if matches!(value, Value::Float(_)) {
                    return ValidateResult::Ok;
                }
                return ValidateResult::Error(validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Float, got {type_name}"),
                ));
            }
            "Bool" => {
                if matches!(value, Value::Bool(_)) {
                    return ValidateResult::Ok;
                }
                return ValidateResult::Error(validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Bool, got {type_name}"),
                ));
            }
            "String" => {
                if matches!(value, Value::String(_)) {
                    return ValidateResult::Ok;
                }
                return ValidateResult::Error(validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected String, got {type_name}"),
                ));
            }
            "Id" => match value {
                Value::String(s) if !s.is_empty() => return ValidateResult::Ok,
                Value::String(_) => {
                    return ValidateResult::Error(validation_error_value(
                        path,
                        "invalid_value",
                        "expected non-empty Id".to_string(),
                    ))
                }
                _ => {
                    return ValidateResult::Error(validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Id, got {type_name}"),
                    ))
                }
            },
            "Email" => match value {
                Value::String(s) if rt_validate::is_email(&s) => return ValidateResult::Ok,
                Value::String(_) => {
                    return ValidateResult::Error(validation_error_value(
                        path,
                        "invalid_value",
                        "invalid email address".to_string(),
                    ))
                }
                _ => {
                    return ValidateResult::Error(validation_error_value(
                        path,
                        "type_mismatch",
                        format!("expected Email, got {type_name}"),
                    ))
                }
            },
            "Bytes" => {
                if matches!(value, Value::String(_)) {
                    return ValidateResult::Ok;
                }
                return ValidateResult::Error(validation_error_value(
                    path,
                    "type_mismatch",
                    format!("expected Bytes, got {type_name}"),
                ));
            }
            _ => {}
        }
    }
    match value {
        Value::Struct { name: struct_name, .. } if struct_name == simple_name => {
            ValidateResult::Ok
        }
        Value::Enum { name: enum_name, .. } if enum_name == simple_name => ValidateResult::Ok,
        _ => ValidateResult::Error(validation_error_value(
            path,
            "type_mismatch",
            format!("expected {name}, got {type_name}"),
        )),
    }
}

fn check_refined(value: &Value, base: &str, args: &[Expr], path: &str) -> ValidateResult {
    let value = value.unboxed();
    match base {
        "String" => {
            let (min, max) = match parse_length_range(args) {
                Ok(range) => range,
                Err(msg) => return ValidateResult::Runtime(msg),
            };
            let len = match value {
                Value::String(s) => s.chars().count() as i64,
                _ => {
                    return ValidateResult::Runtime(
                        "refined String expects a String".to_string(),
                    )
                }
            };
            if rt_validate::check_len(len, min, max) {
                ValidateResult::Ok
            } else {
                ValidateResult::Error(validation_error_value(
                    path,
                    "invalid_value",
                    format!("length {len} out of range {min}..{max}"),
                ))
            }
        }
        "Int" => {
            let (min, max) = match parse_int_range(args) {
                Ok(range) => range,
                Err(msg) => return ValidateResult::Runtime(msg),
            };
            let val = match value {
                Value::Int(v) => v,
                _ => {
                    return ValidateResult::Runtime("refined Int expects an Int".to_string())
                }
            };
            if rt_validate::check_int_range(val, min, max) {
                ValidateResult::Ok
            } else {
                ValidateResult::Error(validation_error_value(
                    path,
                    "invalid_value",
                    format!("value {val} out of range {min}..{max}"),
                ))
            }
        }
        "Float" => {
            let (min, max) = match parse_float_range(args) {
                Ok(range) => range,
                Err(msg) => return ValidateResult::Runtime(msg),
            };
            let val = match value {
                Value::Float(v) => v,
                _ => {
                    return ValidateResult::Runtime("refined Float expects a Float".to_string())
                }
            };
            if rt_validate::check_float_range(val, min, max) {
                ValidateResult::Ok
            } else {
                ValidateResult::Error(validation_error_value(
                    path,
                    "invalid_value",
                    format!("value {val} out of range {min}..{max}"),
                ))
            }
        }
        _ => ValidateResult::Ok,
    }
}

fn parse_length_range(args: &[Expr]) -> Result<(i64, i64), String> {
    let (left, right) = extract_range_args(args)?;
    let min = literal_to_i64(left).ok_or_else(|| "invalid refined range".to_string())?;
    let max = literal_to_i64(right).ok_or_else(|| "invalid refined range".to_string())?;
    Ok((min, max))
}

fn parse_int_range(args: &[Expr]) -> Result<(i64, i64), String> {
    let (left, right) = extract_range_args(args)?;
    let min = literal_to_i64(left).ok_or_else(|| "invalid refined range".to_string())?;
    let max = literal_to_i64(right).ok_or_else(|| "invalid refined range".to_string())?;
    Ok((min, max))
}

fn parse_float_range(args: &[Expr]) -> Result<(f64, f64), String> {
    let (left, right) = extract_range_args(args)?;
    let min = literal_to_f64(left).ok_or_else(|| "invalid refined range".to_string())?;
    let max = literal_to_f64(right).ok_or_else(|| "invalid refined range".to_string())?;
    Ok((min, max))
}

fn extract_range_args<'a>(args: &'a [Expr]) -> Result<(&'a Expr, &'a Expr), String> {
    if args.len() == 1 {
        if let ExprKind::Binary {
            op: BinaryOp::Range,
            left,
            right,
        } = &args[0].kind
        {
            return Ok((left, right));
        }
    }
    if args.len() == 2 {
        return Ok((&args[0], &args[1]));
    }
    Err("refined types expect a range like 1..10".to_string())
}

fn literal_to_i64(expr: &Expr) -> Option<i64> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(v)) => Some(*v),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => match &expr.kind {
            ExprKind::Literal(Literal::Int(v)) => Some(-v),
            _ => None,
        },
        _ => None,
    }
}

fn literal_to_f64(expr: &Expr) -> Option<f64> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(v)) => Some(*v as f64),
        ExprKind::Literal(Literal::Float(v)) => Some(*v),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => match &expr.kind {
            ExprKind::Literal(Literal::Int(v)) => Some(-(*v as f64)),
            ExprKind::Literal(Literal::Float(v)) => Some(-*v),
            _ => None,
        },
        _ => None,
    }
}

fn builtin_runtime_error(
    out: &mut NativeValue,
    heap: &mut NativeHeap,
    message: impl Into<String>,
) -> u8 {
    let handle = heap.intern_string(message.into());
    *out = NativeValue {
        tag: NativeTag::Heap,
        payload: handle,
    };
    2
}

fn db_url() -> Result<String, String> {
    if let Ok(url) = std::env::var("FUSE_DB_URL") {
        return Ok(url);
    }
    if let Ok(url) = std::env::var("DATABASE_URL") {
        return Ok(url);
    }
    let key = rt_config::env_key("App", "dbUrl");
    if let Ok(url) = std::env::var(key) {
        return Ok(url);
    }
    let config_path =
        std::env::var("FUSE_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
    let file_values = rt_config::load_config_file(&config_path)?;
    if let Some(section) = file_values.get("App") {
        if let Some(raw) = section.get("dbUrl") {
            return Ok(raw.clone());
        }
    }
    Err("db url not configured (set FUSE_DB_URL or App.dbUrl)".to_string())
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
extern "C" fn fuse_native_get_index(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 2 {
        return builtin_runtime_error(out, heap, "index access expects 2 values");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(base) = args.get(0) else {
        return builtin_runtime_error(out, heap, "index access expects 2 values");
    };
    let Some(index) = args.get(1) else {
        return builtin_runtime_error(out, heap, "index access expects 2 values");
    };
    let Some(base) = base.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "index access not supported on this value");
    };
    let Some(index) = index.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "index access not supported on this value");
    };
    let base = base.unboxed();
    let index = index.unboxed();
    let value = match base {
        Value::List(items) => {
            let idx = match index {
                Value::Int(v) if v >= 0 => v as usize,
                Value::Int(_) => {
                    return builtin_runtime_error(out, heap, "index out of bounds");
                }
                _ => {
                    return builtin_runtime_error(out, heap, "list index must be Int");
                }
            };
            items
                .get(idx)
                .cloned()
                .ok_or_else(|| "index out of bounds".to_string())
        }
        Value::Map(items) => match index {
            Value::String(key) => Ok(items.get(&key).cloned().unwrap_or(Value::Null)),
            _ => Err("map keys must be strings".to_string()),
        },
        Value::Null => Err("null access".to_string()),
        _ => Err("index access not supported on this value".to_string()),
    };
    let value = match value {
        Ok(value) => value,
        Err(message) => return builtin_runtime_error(out, heap, message),
    };
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "index result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_set_index(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 3 {
        return builtin_runtime_error(out, heap, "index assignment expects 3 values");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let Some(base) = args.get(0) else {
        return builtin_runtime_error(out, heap, "index assignment expects 3 values");
    };
    let Some(index) = args.get(1) else {
        return builtin_runtime_error(out, heap, "index assignment expects 3 values");
    };
    let Some(value) = args.get(2) else {
        return builtin_runtime_error(out, heap, "index assignment expects 3 values");
    };
    let (index_value, value_value, boxed_inner) = {
        let heap_ref: &NativeHeap = heap;
        let Some(index_value) = index.to_value(heap_ref) else {
            return builtin_runtime_error(out, heap, "assignment target must be an indexable value");
        };
        let Some(value_value) = value.to_value(heap_ref) else {
            return builtin_runtime_error(out, heap, "assignment target must be an indexable value");
        };
        let boxed_inner = if base.tag == NativeTag::Heap {
            match heap_ref.get(base.payload) {
                Some(HeapValue::Boxed(inner)) => Some(*inner),
                _ => None,
            }
        } else {
            None
        };
        (index_value, value_value, boxed_inner)
    };

    if let Some(inner_native) = boxed_inner {
        let inner_value = {
            let heap_ref: &NativeHeap = heap;
            match inner_native.to_value(heap_ref) {
                Some(value) => value,
                None => {
                    return builtin_runtime_error(
                        out,
                        heap,
                        "assignment target must be an indexable value",
                    );
                }
            }
        };
        let updated = match inner_value.unboxed() {
            Value::List(mut items) => {
                let idx = match index_value.unboxed() {
                    Value::Int(v) if v >= 0 => v as usize,
                    Value::Int(_) => {
                        return builtin_runtime_error(out, heap, "index out of bounds");
                    }
                    _ => {
                        return builtin_runtime_error(out, heap, "list index must be Int");
                    }
                };
                if idx >= items.len() {
                    return builtin_runtime_error(out, heap, "index out of bounds");
                }
                items[idx] = value_value;
                Value::List(items)
            }
            Value::Map(mut items) => {
                let key = match index_value.unboxed() {
                    Value::String(key) => key,
                    _ => {
                        return builtin_runtime_error(out, heap, "map keys must be strings");
                    }
                };
                items.insert(key, value_value);
                Value::Map(items)
            }
            Value::Null => return builtin_runtime_error(out, heap, "null access"),
            _ => {
                return builtin_runtime_error(
                    out,
                    heap,
                    "assignment target must be an indexable value",
                );
            }
        };
        let Some(native) = NativeValue::from_value(&updated, heap) else {
            return builtin_runtime_error(out, heap, "index assignment unsupported");
        };
        if let Some(slot) = heap.get_mut(base.payload) {
            if let HeapValue::Boxed(inner) = slot {
                *inner = native;
            }
        }
        *out = *base;
        return 0;
    }

    let base_value = {
        let heap_ref: &NativeHeap = heap;
        let Some(value) = base.to_value(heap_ref) else {
            return builtin_runtime_error(out, heap, "assignment target must be an indexable value");
        };
        value
    };
    let updated = match base_value.unboxed() {
        Value::List(mut items) => {
            let idx = match index_value.unboxed() {
                Value::Int(v) if v >= 0 => v as usize,
                Value::Int(_) => {
                    return builtin_runtime_error(out, heap, "index out of bounds");
                }
                _ => {
                    return builtin_runtime_error(out, heap, "list index must be Int");
                }
            };
            if idx >= items.len() {
                return builtin_runtime_error(out, heap, "index out of bounds");
            }
            items[idx] = value_value;
            Value::List(items)
        }
        Value::Map(mut items) => {
            let key = match index_value.unboxed() {
                Value::String(key) => key,
                _ => {
                    return builtin_runtime_error(out, heap, "map keys must be strings");
                }
            };
            items.insert(key, value_value);
            Value::Map(items)
        }
        Value::Null => return builtin_runtime_error(out, heap, "null access"),
        _ => {
            return builtin_runtime_error(
                out,
                heap,
                "assignment target must be an indexable value",
            );
        }
    };
    let Some(native) = NativeValue::from_value(&updated, heap) else {
        return builtin_runtime_error(out, heap, "index assignment unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_set_field(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 3 {
        return builtin_runtime_error(out, heap, "field assignment expects 3 values");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(base) = args.get(0) else {
        return builtin_runtime_error(out, heap, "field assignment expects 3 values");
    };
    let Some(field) = args.get(1) else {
        return builtin_runtime_error(out, heap, "field assignment expects 3 values");
    };
    let Some(value) = args.get(2) else {
        return builtin_runtime_error(out, heap, "field assignment expects 3 values");
    };
    let Some(field_value) = field.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "assignment target must be a struct field");
    };
    let field_name = match field_value.unboxed() {
        Value::String(name) => name,
        _ => {
            return builtin_runtime_error(out, heap, "assignment target must be a struct field");
        }
    };
    let Some(value_value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "assignment target must be a struct field");
    };

    let boxed_inner = if base.tag == NativeTag::Heap {
        match heap_ref.get(base.payload) {
            Some(HeapValue::Boxed(inner)) => Some(*inner),
            _ => None,
        }
    } else {
        None
    };

    if let Some(inner_native) = boxed_inner {
        let inner_value = match inner_native.to_value(heap_ref) {
            Some(value) => value,
            None => {
                return builtin_runtime_error(out, heap, "assignment target must be a struct field");
            }
        };
        let updated = match inner_value.unboxed() {
            Value::Struct { name, mut fields } => {
                fields.insert(field_name, value_value);
                Value::Struct { name, fields }
            }
            _ => {
                return builtin_runtime_error(out, heap, "assignment target must be a struct field");
            }
        };
        let Some(native) = NativeValue::from_value(&updated, heap) else {
            return builtin_runtime_error(out, heap, "field assignment unsupported");
        };
        if let Some(slot) = heap.get_mut(base.payload) {
            if let HeapValue::Boxed(inner) = slot {
                *inner = native;
            }
        }
        *out = *base;
        return 0;
    }

    let base_value = match base.to_value(heap_ref) {
        Some(value) => value,
        None => {
            return builtin_runtime_error(out, heap, "assignment target must be a struct field");
        }
    };
    let updated = match base_value.unboxed() {
        Value::Struct { name, mut fields } => {
            fields.insert(field_name, value_value);
            Value::Struct { name, fields }
        }
        _ => {
            return builtin_runtime_error(out, heap, "assignment target must be a struct field");
        }
    };
    let Some(native) = NativeValue::from_value(&updated, heap) else {
        return builtin_runtime_error(out, heap, "field assignment unsupported");
    };
    *out = native;
    0
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
extern "C" fn fuse_native_match_enum(
    heap: *mut NativeHeap,
    handle: u64,
    variant_handle: u64,
    out: *mut NativeValue,
    len: u64,
) -> u64 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 0;
    };
    let Some(value) = heap.get(handle) else {
        return 0;
    };
    let HeapValue::Enum { variant, payload, .. } = value else {
        return 0;
    };
    let Some(variant_value) = heap.get(variant_handle) else {
        return 0;
    };
    let HeapValue::String(expected_variant) = variant_value else {
        return 0;
    };
    if variant != expected_variant {
        return 0;
    }
    if payload.len() != len as usize {
        return 0;
    }
    if !out.is_null() {
        let slice = unsafe { std::slice::from_raw_parts_mut(out, len as usize) };
        for (dst, src) in slice.iter_mut().zip(payload.iter()) {
            *dst = src.clone();
        }
    }
    1
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

#[unsafe(no_mangle)]
extern "C" fn fuse_native_bang(
    heap: *mut NativeHeap,
    value: *const NativeValue,
    err_value: *const NativeValue,
    has_error: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(value) = (unsafe { value.as_ref() }) else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    let err_override = if has_error != 0 {
        unsafe { err_value.as_ref() }.copied()
    } else {
        None
    };

    let default_error = || {
        let mut fields = HashMap::new();
        fields.insert("message".to_string(), Value::String("missing value".to_string()));
        let value = Value::Struct {
            name: "std.Error".to_string(),
            fields,
        };
        NativeValue::from_value(&value, heap).unwrap_or_else(NativeValue::null)
    };

    match value.tag {
        NativeTag::Null => {
            *out = err_override.unwrap_or_else(default_error);
            1
        }
        NativeTag::Heap => {
            let Some(heap_value) = heap.get(value.payload) else {
                return 2;
            };
            match heap_value {
                HeapValue::ResultOk(inner) => {
                    *out = *inner;
                    0
                }
                HeapValue::ResultErr(inner) => {
                    *out = err_override.unwrap_or(*inner);
                    1
                }
                _ => {
                    *out = *value;
                    0
                }
            }
        }
        _ => {
            *out = *value;
            0
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_iter_init(
    heap: *mut NativeHeap,
    value: *const NativeValue,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(value) = (unsafe { value.as_ref() }) else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "cannot iterate over value");
    };
    let value = value.unboxed();
    let iter_values = match value {
        Value::List(items) => items,
        Value::Map(items) => items.into_values().collect(),
        other => {
            return builtin_runtime_error(
                out,
                heap,
                format!("cannot iterate over {}", value_type_name(&other)),
            );
        }
    };
    let mut native_values = Vec::with_capacity(iter_values.len());
    for item in iter_values {
        let Some(native) = NativeValue::from_value(&item, heap) else {
            return builtin_runtime_error(out, heap, "iterator value unsupported");
        };
        native_values.push(native);
    }
    let handle = heap.insert(HeapValue::Iterator(NativeIterator::new(native_values)));
    *out = NativeValue {
        tag: NativeTag::Heap,
        payload: handle,
    };
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_iter_next(
    heap: *mut NativeHeap,
    iter: *const NativeValue,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(iter) = (unsafe { iter.as_ref() }) else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    let heap_ref: &NativeHeap = heap;
    let handle = match iter.tag {
        NativeTag::Heap => iter.payload,
        _ => {
            let value = iter.to_value(heap_ref).unwrap_or(Value::Null);
            return builtin_runtime_error(
                out,
                heap,
                format!("expected iterator, got {}", value_type_name(&value)),
            );
        }
    };
    let type_name = iter
        .to_value(heap_ref)
        .map(|value| value_type_name(&value))
        .unwrap_or_else(|| "Unknown".to_string());
    let Some(slot) = heap.get_mut(handle) else {
        return builtin_runtime_error(out, heap, format!("expected iterator, got {type_name}"));
    };
    match slot {
        HeapValue::Iterator(iter) => {
            if iter.index >= iter.values.len() {
                return 1;
            }
            let item = iter.values[iter.index];
            iter.index += 1;
            *out = item;
            0
        }
        _ => builtin_runtime_error(out, heap, format!("expected iterator, got {type_name}")),
    }
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_add(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 2 {
        return builtin_runtime_error(out, heap, "add expects 2 values");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(left) = args.get(0) else {
        return builtin_runtime_error(out, heap, "add expects 2 values");
    };
    let Some(right) = args.get(1) else {
        return builtin_runtime_error(out, heap, "add expects 2 values");
    };
    let Some(left) = left.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "add expects 2 values");
    };
    let Some(right) = right.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "add expects 2 values");
    };
    let left = left.unboxed();
    let right = right.unboxed();
    let value = match (left, right) {
        (Value::String(a), Value::String(b)) => Value::String(format!("{a}{b}")),
        (Value::String(a), b) => Value::String(format!("{a}{}", b.to_string_value())),
        (a, Value::String(b)) => Value::String(format!("{}{}", a.to_string_value(), b)),
        (Value::Int(a), Value::Int(b)) => Value::Int(a + b),
        (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
        (Value::Int(a), Value::Float(b)) => Value::Float(a as f64 + b),
        (Value::Float(a), Value::Int(b)) => Value::Float(a + b as f64),
        _ => return builtin_runtime_error(out, heap, "unsupported + operands"),
    };
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "add result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_range(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 2 {
        return builtin_runtime_error(out, heap, "range expects 2 arguments");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(left) = args.get(0) else {
        return builtin_runtime_error(out, heap, "range expects 2 arguments");
    };
    let Some(right) = args.get(1) else {
        return builtin_runtime_error(out, heap, "range expects 2 arguments");
    };
    let Some(left) = left.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "range expects numeric bounds");
    };
    let Some(right) = right.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "range expects numeric bounds");
    };
    let left = left.unboxed();
    let right = right.unboxed();
    let value = match (left, right) {
        (Value::Int(start), Value::Int(end)) => {
            if start > end {
                return builtin_runtime_error(out, heap, "range start must be <= end");
            }
            let items = (start..=end).map(Value::Int).collect();
            Value::List(items)
        }
        (Value::Float(start), Value::Float(end)) => {
            if !start.is_finite() || !end.is_finite() {
                return builtin_runtime_error(out, heap, "invalid range bounds");
            }
            if start > end {
                return builtin_runtime_error(out, heap, "range start must be <= end");
            }
            let mut items = Vec::new();
            let mut current = start;
            let epsilon = 1e-9;
            while current <= end + epsilon {
                items.push(Value::Float(current));
                current += 1.0;
            }
            Value::List(items)
        }
        (Value::Int(start), Value::Float(end)) => {
            if !end.is_finite() {
                return builtin_runtime_error(out, heap, "invalid range bounds");
            }
            let start = start as f64;
            if start > end {
                return builtin_runtime_error(out, heap, "range start must be <= end");
            }
            let mut items = Vec::new();
            let mut current = start;
            let epsilon = 1e-9;
            while current <= end + epsilon {
                items.push(Value::Float(current));
                current += 1.0;
            }
            Value::List(items)
        }
        (Value::Float(start), Value::Int(end)) => {
            if !start.is_finite() {
                return builtin_runtime_error(out, heap, "invalid range bounds");
            }
            let end = end as f64;
            if start > end {
                return builtin_runtime_error(out, heap, "range start must be <= end");
            }
            let mut items = Vec::new();
            let mut current = start;
            let epsilon = 1e-9;
            while current <= end + epsilon {
                items.push(Value::Float(current));
                current += 1.0;
            }
            Value::List(items)
        }
        _ => return builtin_runtime_error(out, heap, "range expects numeric bounds"),
    };
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "range result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_eq(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    native_compare(BinaryOp::Eq, heap, args, len, out)
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_not_eq(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    native_compare(BinaryOp::NotEq, heap, args, len, out)
}

fn native_compare(
    op: BinaryOp,
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 2 {
        return builtin_runtime_error(out, heap, "compare expects 2 values");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(left) = args.get(0) else {
        return builtin_runtime_error(out, heap, "compare expects 2 values");
    };
    let Some(right) = args.get(1) else {
        return builtin_runtime_error(out, heap, "compare expects 2 values");
    };
    let Some(left) = left.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "compare expects 2 values");
    };
    let Some(right) = right.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "compare expects 2 values");
    };
    let left = left.unboxed();
    let right = right.unboxed();
    let value = match (left, right) {
        (Value::Int(a), Value::Int(b)) => match op {
            BinaryOp::Eq => Value::Bool(a == b),
            BinaryOp::NotEq => Value::Bool(a != b),
            _ => return builtin_runtime_error(out, heap, "unsupported comparison"),
        },
        (Value::Float(a), Value::Float(b)) => match op {
            BinaryOp::Eq => Value::Bool(a == b),
            BinaryOp::NotEq => Value::Bool(a != b),
            _ => return builtin_runtime_error(out, heap, "unsupported comparison"),
        },
        (Value::Bool(a), Value::Bool(b)) => match op {
            BinaryOp::Eq => Value::Bool(a == b),
            BinaryOp::NotEq => Value::Bool(a != b),
            _ => return builtin_runtime_error(out, heap, "unsupported comparison"),
        },
        (Value::String(a), Value::String(b)) => match op {
            BinaryOp::Eq => Value::Bool(a == b),
            BinaryOp::NotEq => Value::Bool(a != b),
            _ => return builtin_runtime_error(out, heap, "unsupported comparison"),
        },
        _ => return builtin_runtime_error(out, heap, "unsupported comparison operands"),
    };
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "compare result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_builtin_log(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    let mut values = Vec::new();
    if len > 0 {
        let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
        let heap_ref: &NativeHeap = heap;
        for arg in args {
            let value = arg.to_value(heap_ref).unwrap_or(Value::Null);
            values.push(value);
        }
    }

    let mut level = LogLevel::Info;
    let mut start_idx = 0usize;
    if values.len() >= 2 {
        if let Value::String(raw_level) = &values[0] {
            if let Some(parsed) = parse_log_level(raw_level) {
                level = parsed;
                start_idx = 1;
            }
        }
    }

    if level >= min_log_level() {
        let message = values
            .get(start_idx)
            .map(|val| val.to_string_value())
            .unwrap_or_default();
        let data_args = values
            .get(start_idx.saturating_add(1)..)
            .unwrap_or(&[]);
        if !data_args.is_empty() {
            let data_json = if data_args.len() == 1 {
                value_to_json(&data_args[0])
            } else {
                rt_json::JsonValue::Array(data_args.iter().map(value_to_json).collect())
            };
            let mut obj = BTreeMap::new();
            obj.insert(
                "level".to_string(),
                rt_json::JsonValue::String(level.json_label().to_string()),
            );
            obj.insert(
                "message".to_string(),
                rt_json::JsonValue::String(message),
            );
            obj.insert("data".to_string(), data_json);
            eprintln!("{}", rt_json::encode(&rt_json::JsonValue::Object(obj)));
        } else {
            let message = values[start_idx..]
                .iter()
                .map(|val| val.to_string_value())
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!("[{}] {}", level.label(), message);
        }
    }

    *out = NativeValue::int(0);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_builtin_print(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    let text = if len == 0 {
        String::new()
    } else {
        let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
        let heap_ref: &NativeHeap = heap;
        let Some(value) = args.get(0) else {
            return builtin_runtime_error(out, heap, "print expects a value");
        };
        let Some(value) = value.to_value(heap_ref) else {
            return builtin_runtime_error(out, heap, "print expects a value");
        };
        value.to_string_value()
    };
    println!("{text}");
    *out = NativeValue::int(0);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_builtin_env(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    if len == 0 {
        return builtin_runtime_error(out, heap, "env expects a string argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "env expects a string argument");
    };
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "env expects a string argument");
    };
    let key = match value {
        Value::String(text) => text,
        _ => return builtin_runtime_error(out, heap, "env expects a string argument"),
    };
    match std::env::var(key) {
        Ok(value) => {
            *out = NativeValue::string(value, heap);
            0
        }
        Err(_) => {
            *out = NativeValue::null();
            0
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_builtin_assert(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    if len == 0 {
        return builtin_runtime_error(out, heap, "assert expects a Bool as the first argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "assert expects a Bool as the first argument");
    };
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "assert expects a Bool as the first argument");
    };
    let cond = match value {
        Value::Bool(value) => value,
        _ => return builtin_runtime_error(out, heap, "assert expects a Bool as the first argument"),
    };
    if cond {
        *out = NativeValue::int(0);
        return 0;
    }
    let message = args
        .get(1)
        .and_then(|value| value.to_value(heap_ref))
        .map(|val| val.to_string_value())
        .unwrap_or_else(|| "assertion failed".to_string());
    builtin_runtime_error(out, heap, format!("assert failed: {message}"))
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_config_get(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    if len < 2 {
        return builtin_runtime_error(out, heap, "config.get expects config and field");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(config_val) = args.get(0) else {
        return builtin_runtime_error(out, heap, "config.get expects config and field");
    };
    let Some(field_val) = args.get(1) else {
        return builtin_runtime_error(out, heap, "config.get expects config and field");
    };
    let Some(config_val) = config_val.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "config.get expects config and field");
    };
    let Some(field_val) = field_val.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "config.get expects config and field");
    };
    let config = match config_val {
        Value::String(text) => text,
        _ => return builtin_runtime_error(out, heap, "config.get expects config and field"),
    };
    let field = match field_val {
        Value::String(text) => text,
        _ => return builtin_runtime_error(out, heap, "config.get expects config and field"),
    };
    if !heap.has_config(&config) {
        return builtin_runtime_error(out, heap, format!("unknown config {config}"));
    }
    let Some(value) = heap.config_field(&config, &field) else {
        return builtin_runtime_error(
            out,
            heap,
            format!("unknown config field {config}.{field}"),
        );
    };
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "config value unsupported");
    };
    *out = native;
    0
}

fn task_arg_from_native<'a>(
    heap: &'a NativeHeap,
    value: &NativeValue,
) -> Option<(&'a TaskValue, u64)> {
    if value.tag != NativeTag::Heap {
        return None;
    }
    let handle = value.payload;
    match heap.get(handle)? {
        HeapValue::Task(task) => Some((task, handle)),
        _ => None,
    }
}

fn task_arg_from_native_mut<'a>(
    heap: &'a mut NativeHeap,
    value: &NativeValue,
) -> Option<(&'a mut TaskValue, u64)> {
    if value.tag != NativeTag::Heap {
        return None;
    }
    let handle = value.payload;
    match heap.get_mut(handle)? {
        HeapValue::Task(task) => Some((task, handle)),
        _ => None,
    }
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_task_id(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len == 0 {
        return builtin_runtime_error(out, heap, "task.id expects a Task argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "task.id expects a Task argument");
    };
    let Some((task, handle)) = task_arg_from_native(heap, value) else {
        return builtin_runtime_error(out, heap, "task.id expects a Task argument");
    };
    let id = if task.id == 0 { handle } else { task.id };
    *out = NativeValue::string(format!("task-{id}"), heap);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_task_done(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len == 0 {
        return builtin_runtime_error(out, heap, "task.done expects a Task argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "task.done expects a Task argument");
    };
    let Some((task, _handle)) = task_arg_from_native(heap, value) else {
        return builtin_runtime_error(out, heap, "task.done expects a Task argument");
    };
    *out = NativeValue::bool(task.done);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_task_cancel(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len == 0 {
        return builtin_runtime_error(out, heap, "task.cancel expects a Task argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "task.cancel expects a Task argument");
    };
    let Some((task, _handle)) = task_arg_from_native_mut(heap, value) else {
        return builtin_runtime_error(out, heap, "task.cancel expects a Task argument");
    };
    let cancelled = if task.done || task.cancelled {
        false
    } else {
        task.cancelled = true;
        true
    };
    *out = NativeValue::bool(cancelled);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_json_encode(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    if len == 0 {
        return builtin_runtime_error(out, heap, "json.encode expects a value");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "json.encode expects a value");
    };
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "json.encode expects a value");
    };
    let json = value_to_json(&value);
    let encoded = rt_json::encode(&json);
    *out = NativeValue::string(encoded, heap);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_json_decode(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    if len == 0 {
        return builtin_runtime_error(out, heap, "json.decode expects a string argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "json.decode expects a string argument");
    };
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "json.decode expects a string argument");
    };
    let text = match value {
        Value::String(text) => text,
        _ => return builtin_runtime_error(out, heap, "json.decode expects a string argument"),
    };
    let json = match rt_json::decode(&text) {
        Ok(json) => json,
        Err(msg) => return builtin_runtime_error(out, heap, format!("invalid json: {msg}")),
    };
    let value = json_to_value(&json);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "json.decode result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_validate_struct(
    heap: *mut NativeHeap,
    name_handle: u64,
    pairs: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    let Some(name_value) = heap.get(name_handle) else {
        return builtin_runtime_error(out, heap, "invalid struct name");
    };
    let HeapValue::String(type_name) = name_value else {
        return builtin_runtime_error(out, heap, "invalid struct name");
    };
    let Some(type_info) = heap.type_info(type_name) else {
        return builtin_runtime_error(out, heap, "unknown struct type");
    };

    if len == 0 {
        *out = NativeValue::int(0);
        return 0;
    }
    let pairs = unsafe { std::slice::from_raw_parts(pairs, len as usize * 2) };
    let heap_ref: &NativeHeap = heap;
    for idx in 0..len as usize {
        let key_val = pairs[idx * 2];
        let value_val = pairs[idx * 2 + 1];
        let key_value = match key_val.to_value(heap_ref) {
            Some(Value::String(text)) => text,
            _ => {
                return builtin_runtime_error(out, heap, "struct field names must be strings");
            }
        };
        let Some(field_info) = type_info
            .fields
            .iter()
            .find(|field| field.name == key_value)
        else {
            return builtin_runtime_error(
                out,
                heap,
                format!("unknown field {}.{}", type_info.name, key_value),
            );
        };
        let Some(value) = value_val.to_value(heap_ref) else {
            return builtin_runtime_error(out, heap, "invalid field value");
        };
        let path = format!("{}.{}", type_info.name, key_value);
        match validate_value(&value, &field_info.ty, &path) {
            ValidateResult::Ok => {}
            ValidateResult::Error(err_value) => {
                let Some(native) = NativeValue::from_value(&err_value, heap) else {
                    return builtin_runtime_error(out, heap, "validation error");
                };
                *out = native;
                return 1;
            }
            ValidateResult::Runtime(message) => {
                return builtin_runtime_error(out, heap, message);
            }
        }
    }

    *out = NativeValue::int(0);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_db_exec(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    if len == 0 || len > 2 {
        return builtin_runtime_error(out, heap, "db.exec expects 1 or 2 arguments");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "db.exec expects a SQL string");
    };
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "db.exec expects a SQL string");
    };
    let sql = match value {
        Value::String(text) => text,
        _ => return builtin_runtime_error(out, heap, "db.exec expects a SQL string"),
    };
    let params = if len > 1 {
        let Some(params_value) = args.get(1).and_then(|v| v.to_value(heap_ref)) else {
            return builtin_runtime_error(out, heap, "db.exec params must be a list");
        };
        match params_value {
            Value::List(items) => items,
            _ => return builtin_runtime_error(out, heap, "db.exec params must be a list"),
        }
    } else {
        Vec::new()
    };
    let url = match db_url() {
        Ok(url) => url,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let db = match heap.db_mut(url) {
        Ok(db) => db,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    if let Err(err) = db.exec_params(&sql, &params) {
        return builtin_runtime_error(out, heap, err);
    }
    *out = NativeValue::int(0);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_db_query(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    if len == 0 || len > 2 {
        return builtin_runtime_error(out, heap, "db.query expects 1 or 2 arguments");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "db.query expects a SQL string");
    };
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "db.query expects a SQL string");
    };
    let sql = match value {
        Value::String(text) => text,
        _ => return builtin_runtime_error(out, heap, "db.query expects a SQL string"),
    };
    let params = if len > 1 {
        let Some(params_value) = args.get(1).and_then(|v| v.to_value(heap_ref)) else {
            return builtin_runtime_error(out, heap, "db.query params must be a list");
        };
        match params_value {
            Value::List(items) => items,
            _ => return builtin_runtime_error(out, heap, "db.query params must be a list"),
        }
    } else {
        Vec::new()
    };
    let url = match db_url() {
        Ok(url) => url,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let db = match heap.db_mut(url) {
        Ok(db) => db,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let rows = match db.query_params(&sql, &params) {
        Ok(rows) => rows,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let list: Vec<Value> = rows.into_iter().map(Value::Map).collect();
    let value = Value::List(list);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "db.query result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_db_one(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };

    if len == 0 || len > 2 {
        return builtin_runtime_error(out, heap, "db.one expects 1 or 2 arguments");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "db.one expects a SQL string");
    };
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "db.one expects a SQL string");
    };
    let sql = match value {
        Value::String(text) => text,
        _ => return builtin_runtime_error(out, heap, "db.one expects a SQL string"),
    };
    let params = if len > 1 {
        let Some(params_value) = args.get(1).and_then(|v| v.to_value(heap_ref)) else {
            return builtin_runtime_error(out, heap, "db.one params must be a list");
        };
        match params_value {
            Value::List(items) => items,
            _ => return builtin_runtime_error(out, heap, "db.one params must be a list"),
        }
    } else {
        Vec::new()
    };
    let url = match db_url() {
        Ok(url) => url,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let db = match heap.db_mut(url) {
        Ok(db) => db,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let rows = match db.query_params(&sql, &params) {
        Ok(rows) => rows,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    if let Some(row) = rows.into_iter().next() {
        let value = Value::Map(row);
        let Some(native) = NativeValue::from_value(&value, heap) else {
            return builtin_runtime_error(out, heap, "db.one result unsupported");
        };
        *out = native;
        0
    } else {
        *out = NativeValue::null();
        0
    }
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_db_from(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 1 {
        return builtin_runtime_error(out, heap, "db.from expects a table name");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(value) = args.get(0) else {
        return builtin_runtime_error(out, heap, "db.from expects a table name");
    };
    let Some(value) = value.to_value(heap_ref) else {
        return builtin_runtime_error(out, heap, "db.from expects a table name");
    };
    let table = match value {
        Value::String(text) => text,
        _ => return builtin_runtime_error(out, heap, "db.from expects a table name"),
    };
    let query = match crate::db::Query::new(table) {
        Ok(query) => query,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let value = Value::Query(query);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "db.from result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_select(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 2 {
        return builtin_runtime_error(out, heap, "query.select expects 2 arguments");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.select expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.select expects a Query");
    };
    let Some(cols_val) = args.get(1).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.select expects a list of strings");
    };
    let Value::List(items) = cols_val else {
        return builtin_runtime_error(out, heap, "query.select expects a list of strings");
    };
    let mut columns = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::String(text) => columns.push(text),
            _ => {
                return builtin_runtime_error(out, heap, "query.select expects a list of strings");
            }
        }
    }
    let next = match query.select(columns) {
        Ok(next) => next,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let value = Value::Query(next);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "query.select result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_where(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 4 {
        return builtin_runtime_error(out, heap, "query.where expects 4 arguments");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.where expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.where expects a Query");
    };
    let Some(column_val) = args.get(1).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.where expects a column string");
    };
    let Value::String(column) = column_val else {
        return builtin_runtime_error(out, heap, "query.where expects a column string");
    };
    let Some(op_val) = args.get(2).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.where expects an operator string");
    };
    let Value::String(op) = op_val else {
        return builtin_runtime_error(out, heap, "query.where expects an operator string");
    };
    let Some(value) = args.get(3).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.where expects a value");
    };
    let next = match query.where_clause(column, op, value) {
        Ok(next) => next,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let value = Value::Query(next);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "query.where result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_order_by(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 3 {
        return builtin_runtime_error(out, heap, "query.order_by expects 3 arguments");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.order_by expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.order_by expects a Query");
    };
    let Some(column_val) = args.get(1).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.order_by expects a column string");
    };
    let Value::String(column) = column_val else {
        return builtin_runtime_error(out, heap, "query.order_by expects a column string");
    };
    let Some(dir_val) = args.get(2).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.order_by expects a direction string");
    };
    let Value::String(dir) = dir_val else {
        return builtin_runtime_error(out, heap, "query.order_by expects a direction string");
    };
    let next = match query.order_by(column, dir) {
        Ok(next) => next,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let value = Value::Query(next);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "query.order_by result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_limit(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 2 {
        return builtin_runtime_error(out, heap, "query.limit expects 2 arguments");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.limit expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.limit expects a Query");
    };
    let Some(limit_val) = args.get(1).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.limit expects an Int");
    };
    let limit = match limit_val {
        Value::Int(v) => v,
        Value::Float(v) => v as i64,
        _ => return builtin_runtime_error(out, heap, "query.limit expects an Int"),
    };
    let next = match query.limit(limit) {
        Ok(next) => next,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let value = Value::Query(next);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "query.limit result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_one(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 1 {
        return builtin_runtime_error(out, heap, "query.one expects 1 argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.one expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.one expects a Query");
    };
    let (sql, params) = match query.build_sql(Some(1)) {
        Ok(result) => result,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let url = match db_url() {
        Ok(url) => url,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let db = match heap.db_mut(url) {
        Ok(db) => db,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let rows = match db.query_params(&sql, &params) {
        Ok(rows) => rows,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    if let Some(row) = rows.into_iter().next() {
        let value = Value::Map(row);
        let Some(native) = NativeValue::from_value(&value, heap) else {
            return builtin_runtime_error(out, heap, "query.one result unsupported");
        };
        *out = native;
    } else {
        *out = NativeValue::null();
    }
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_all(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 1 {
        return builtin_runtime_error(out, heap, "query.all expects 1 argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.all expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.all expects a Query");
    };
    let (sql, params) = match query.build_sql(None) {
        Ok(result) => result,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let url = match db_url() {
        Ok(url) => url,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let db = match heap.db_mut(url) {
        Ok(db) => db,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let rows = match db.query_params(&sql, &params) {
        Ok(rows) => rows,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let list: Vec<Value> = rows.into_iter().map(Value::Map).collect();
    let value = Value::List(list);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "query.all result unsupported");
    };
    *out = native;
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_exec(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 1 {
        return builtin_runtime_error(out, heap, "query.exec expects 1 argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.exec expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.exec expects a Query");
    };
    let (sql, params) = match query.build_sql(None) {
        Ok(result) => result,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let url = match db_url() {
        Ok(url) => url,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let db = match heap.db_mut(url) {
        Ok(db) => db,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    if let Err(err) = db.exec_params(&sql, &params) {
        return builtin_runtime_error(out, heap, err);
    }
    *out = NativeValue::int(0);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_sql(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 1 {
        return builtin_runtime_error(out, heap, "query.sql expects 1 argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.sql expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.sql expects a Query");
    };
    let sql = match query.sql() {
        Ok(sql) => sql,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    *out = NativeValue::string(sql, heap);
    0
}

#[unsafe(no_mangle)]
extern "C" fn fuse_native_query_params(
    heap: *mut NativeHeap,
    args: *const NativeValue,
    len: u64,
    out: *mut NativeValue,
) -> u8 {
    let heap = unsafe { heap.as_mut() };
    let Some(heap) = heap else {
        return 2;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return 2;
    };
    if len != 1 {
        return builtin_runtime_error(out, heap, "query.params expects 1 argument");
    }
    let args = unsafe { std::slice::from_raw_parts(args, len as usize) };
    let heap_ref: &NativeHeap = heap;
    let Some(query_val) = args.get(0).and_then(|v| v.to_value(heap_ref)) else {
        return builtin_runtime_error(out, heap, "query.params expects a Query");
    };
    let Value::Query(query) = query_val else {
        return builtin_runtime_error(out, heap, "query.params expects a Query");
    };
    let params = match query.params() {
        Ok(params) => params,
        Err(err) => return builtin_runtime_error(out, heap, err),
    };
    let value = Value::List(params);
    let Some(native) = NativeValue::from_value(&value, heap) else {
        return builtin_runtime_error(out, heap, "query.params result unsupported");
    };
    *out = native;
    0
}

fn entry_signature<M: Module>(module: &mut M) -> cranelift_codegen::ir::Signature {
    let pointer_ty = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_ty));
    signature.params.push(AbiParam::new(pointer_ty));
    signature.params.push(AbiParam::new(pointer_ty));
    signature.returns.push(AbiParam::new(types::I8));
    signature
}

fn compile_function<M: Module>(
    module: &mut M,
    hostcalls: &HostCalls,
    program: &IrProgram,
    name: &str,
    func: &Function,
    param_types: &[JitType],
    linkage: Linkage,
    heap: &mut NativeHeap,
    state: &mut CompileState,
    pending: &mut Vec<PendingCompiled>,
) -> Option<FuncId> {
    let ret = if let Some(ret) = func.ret.as_ref() {
        match return_kind(ret, program) {
            Some(kind) => kind,
            None => jit_fail!(func, None::<usize>, None::<&Instr>, "unsupported return type"),
        }
    } else {
        ReturnKind::Value
    };
    if func.code.is_empty() {
        jit_fail!(func, None::<usize>, None::<&Instr>, "empty function body");
    }
    if func.params.len() != param_types.len() {
        jit_fail!(
            func,
            None::<usize>,
            None::<&Instr>,
            "parameter arity mismatch"
        );
    }
    if std::env::var("FUSE_NATIVE_DEBUG").is_ok() {
        let debug_func = std::env::var("FUSE_NATIVE_DEBUG_FUNC").ok();
        if debug_func.as_deref().map_or(true, |name| name == func.name) {
            eprintln!("native compile {} code:", func.name);
            for (idx, instr) in func.code.iter().enumerate() {
                eprintln!("  {idx}: {instr:?}");
            }
        }
    }

    let (key, id) = match state.ensure_declared(module, name, param_types, linkage) {
        Some(result) => result,
        None => jit_fail!(
            func,
            None::<usize>,
            None::<&Instr>,
            "signature mismatch"
        ),
    };
    if state.compiled.contains(&key) {
        return Some(id);
    }
    if state.in_progress.contains(&key) {
        return Some(id);
    }
    state.in_progress.insert(key.clone());

    let starts = match block_starts(&func.code) {
        Some(starts) => starts,
        None => jit_fail!(
            func,
            None::<usize>,
            None::<&Instr>,
            "invalid control flow"
        ),
    };
    let (local_types, entry_stacks) = match analyze_types(func, param_types, &starts, program) {
        Some(result) => result,
        None => jit_fail!(
            func,
            None::<usize>,
            None::<&Instr>,
            "type analysis failed"
        ),
    };
    let mut ctx = module.make_context();
    let pointer_ty = module.target_config().pointer_type();
    ctx.func.signature = entry_signature(module);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

    let blocks: Vec<_> = starts.iter().map(|_| builder.create_block()).collect();
    let mut block_for_start = HashMap::new();
    for (idx, start) in starts.iter().enumerate() {
        block_for_start.insert(*start, idx);
    }

    for (idx, block) in blocks.iter().enumerate() {
        if idx == 0 {
            continue;
        }
        if let Some(stack_types) = entry_stacks.get(idx) {
            for kind in stack_types {
                builder.append_block_param(*block, clif_type(kind, pointer_ty));
            }
        }
    }

    builder.switch_to_block(blocks[0]);
    builder.append_block_params_for_function_params(blocks[0]);
    let args_ptr = builder.block_params(blocks[0])[0];
    let out_ptr = builder.block_params(blocks[0])[1];
    let heap_ptr = builder.block_params(blocks[0])[2];

    let mut locals = Vec::with_capacity(func.locals);
    let mut local_value_slots: Vec<Option<cranelift_codegen::ir::StackSlot>> =
        vec![None; func.locals];
    for slot in 0..func.locals {
        let kind = *local_types.get(slot)?;
        let local_ty = clif_type(&kind, pointer_ty);
        let var = builder.declare_var(local_ty);
        if kind == JitType::Value {
            let slot_id = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                NATIVE_VALUE_SIZE as u32,
                NATIVE_VALUE_ALIGN_SHIFT,
            ));
            local_value_slots[slot] = Some(slot_id);
            let ptr = builder.ins().stack_addr(pointer_ty, slot_id, 0);
            if slot < func.params.len() {
                let slot_idx = i32::try_from(slot).ok()?;
                let base = slot_idx.checked_mul(NATIVE_VALUE_SIZE)?;
                let tag = builder.ins().load(types::I64, MemFlags::new(), args_ptr, base);
                let payload = builder.ins().load(
                    types::I64,
                    MemFlags::new(),
                    args_ptr,
                    base + NATIVE_VALUE_PAYLOAD_OFFSET,
                );
                builder.ins().store(MemFlags::new(), tag, ptr, 0);
                builder
                    .ins()
                    .store(MemFlags::new(), payload, ptr, NATIVE_VALUE_PAYLOAD_OFFSET);
            } else {
                let tag = builder.ins().iconst(types::I64, NativeTag::Null as i64);
                let payload = builder.ins().iconst(types::I64, 0);
                builder.ins().store(MemFlags::new(), tag, ptr, 0);
                builder
                    .ins()
                    .store(MemFlags::new(), payload, ptr, NATIVE_VALUE_PAYLOAD_OFFSET);
            }
            builder.def_var(var, ptr);
        } else {
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
        }
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
        if let Some(stack_types) = entry_stacks.get(block_idx) {
            if !stack_types.is_empty() {
                let params = builder.block_params(block);
                let offset = if block_idx == 0 { 3 } else { 0 };
                for (idx, kind) in stack_types.iter().enumerate() {
                    let value = params.get(offset + idx).copied()?;
                    stack.push(StackValue { value, kind: *kind });
                }
            }
        }
        let mut terminated = false;
        for ip in *start..end {
            match &func.code[ip] {
                Instr::Push(Const::Unit) => {
                    stack.push(StackValue {
                        value: builder.ins().iconst(types::I64, 0),
                        kind: JitType::Unit,
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
                Instr::Push(Const::Null) => {
                    stack.push(StackValue {
                        value: builder.ins().iconst(types::I64, 0),
                        kind: JitType::Null,
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
                            NATIVE_VALUE_ALIGN_SHIFT,
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
                            NATIVE_VALUE_ALIGN_SHIFT,
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
                            NATIVE_VALUE_ALIGN_SHIFT,
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
                    let validate_out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let validate_out_ptr =
                        builder.ins().stack_addr(pointer_ty, validate_out_slot, 0);
                    let validate_func =
                        module.declare_func_in_func(hostcalls.validate_struct, builder.func);
                    let len_val = builder.ins().iconst(types::I64, count as i64);
                    let name_val = builder.ins().iconst(types::I64, name_handle as i64);
                    let validate_call = builder.ins().call(
                        validate_func,
                        &[heap_ptr, name_val, base, len_val, validate_out_ptr],
                    );
                    let status = builder.inst_results(validate_call)[0];
                    let ok_block = builder.create_block();
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let ok_args: [BlockArg; 0] = [];
                    let err_args = [
                        BlockArg::Value(status),
                        BlockArg::Value(validate_out_ptr),
                    ];
                    builder.ins().brif(
                        is_ok,
                        ok_block,
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    builder.switch_to_block(ok_block);
                    let func_ref =
                        module.declare_func_in_func(hostcalls.make_struct, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, name_val, base, len_val]);
                    let handle = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(StackValue {
                        value: handle,
                        kind: JitType::Struct,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    builder.ins().jump(blocks[ok_idx], &ok_args);
                    terminated = true;
                    break;
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
                            NATIVE_VALUE_ALIGN_SHIFT,
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
                            NATIVE_VALUE_ALIGN_SHIFT,
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
                            NATIVE_VALUE_ALIGN_SHIFT,
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
                Instr::MatchLocal {
                    slot,
                    pat,
                    bindings,
                    jump,
                } => {
                    let local_kind = *local_types.get(*slot)?;
                    let local_var = *locals.get(*slot)?;
                    let binding_map: HashMap<&str, usize> = bindings
                        .iter()
                        .map(|(name, slot)| (name.as_str(), *slot))
                        .collect();
                    match &pat.kind {
                        PatternKind::Wildcard => {
                            // Always matches, no bindings to apply.
                        }
                        PatternKind::Ident(ident) => {
                            if let Some(bind_slot) = binding_map.get(ident.name.as_str()) {
                                let target_var = *locals.get(*bind_slot)?;
                                if *local_types.get(*bind_slot)? != JitType::Value {
                                    jit_fail!(
                                        func,
                                        Some(ip),
                                        Some(&func.code[ip]),
                                        "match binding type mismatch"
                                    );
                                }
                                let value = StackValue {
                                    value: builder.use_var(local_var),
                                    kind: local_kind,
                                };
                                let src_ptr = ensure_value_ptr(&mut builder, pointer_ty, value)?;
                                let dst_ptr = builder.use_var(target_var);
                                copy_native_value(&mut builder, src_ptr, dst_ptr);
                            } else {
                                let ok_idx = *block_for_start.get(&(ip + 1))?;
                                let fail_idx = *block_for_start.get(jump)?;
                                let fail_args = coerce_stack_args(
                                    &mut builder,
                                    pointer_ty,
                                    &stack,
                                    entry_stacks.get(fail_idx)?,
                                )?;
                                let enum_handle = match local_kind {
                                    JitType::Enum => builder.use_var(local_var),
                                    JitType::Value => {
                                        let value_ptr = builder.use_var(local_var);
                                        let tag = builder.ins().load(
                                            types::I64,
                                            MemFlags::new(),
                                            value_ptr,
                                            0,
                                        );
                                        let payload = builder.ins().load(
                                            types::I64,
                                            MemFlags::new(),
                                            value_ptr,
                                            NATIVE_VALUE_PAYLOAD_OFFSET,
                                        );
                                        let is_heap = builder.ins().icmp_imm(
                                            IntCC::Equal,
                                            tag,
                                            NativeTag::Heap as i64,
                                        );
                                        let match_block = builder.create_block();
                                        builder.ins().brif(
                                            is_heap,
                                            match_block,
                                            &[],
                                            blocks[fail_idx],
                                            &fail_args,
                                        );
                                        builder.switch_to_block(match_block);
                                        payload
                                    }
                                    _ => {
                                        jit_fail!(
                                            func,
                                            Some(ip),
                                            Some(&func.code[ip]),
                                            "match on non-enum"
                                        );
                                    }
                                };
                                let variant_handle =
                                    NativeValue::intern_string(ident.name.clone(), heap).payload;
                                let func_ref =
                                    module.declare_func_in_func(hostcalls.match_enum, builder.func);
                                let variant_val =
                                    builder.ins().iconst(types::I64, variant_handle as i64);
                                let len_val = builder.ins().iconst(types::I64, 0);
                                let payload_ptr = builder.ins().iconst(pointer_ty, 0);
                                let call = builder.ins().call(
                                    func_ref,
                                    &[heap_ptr, enum_handle, variant_val, payload_ptr, len_val],
                                );
                                let matched = builder.inst_results(call)[0];
                                let is_match =
                                    builder.ins().icmp_imm(IntCC::NotEqual, matched, 0);
                                let bind_block = builder.create_block();
                                builder.ins().brif(
                                    is_match,
                                    bind_block,
                                    &[],
                                    blocks[fail_idx],
                                    &fail_args,
                                );
                                builder.switch_to_block(bind_block);
                                let ok_args = coerce_stack_args(
                                    &mut builder,
                                    pointer_ty,
                                    &stack,
                                    entry_stacks.get(ok_idx)?,
                                )?;
                                builder.ins().jump(blocks[ok_idx], &ok_args);
                                terminated = true;
                                break;
                            }
                        }
                        PatternKind::EnumVariant { name, args } => {
                            let ok_idx = *block_for_start.get(&(ip + 1))?;
                            let fail_idx = *block_for_start.get(jump)?;
                            let fail_args = coerce_stack_args(
                                &mut builder,
                                pointer_ty,
                                &stack,
                                entry_stacks.get(fail_idx)?,
                            )?;
                            let enum_handle = match local_kind {
                                JitType::Enum => builder.use_var(local_var),
                                JitType::Value => {
                                    let value_ptr = builder.use_var(local_var);
                                    let tag = builder.ins().load(
                                        types::I64,
                                        MemFlags::new(),
                                        value_ptr,
                                        0,
                                    );
                                    let payload = builder.ins().load(
                                        types::I64,
                                        MemFlags::new(),
                                        value_ptr,
                                        NATIVE_VALUE_PAYLOAD_OFFSET,
                                    );
                                    let is_heap = builder.ins().icmp_imm(
                                        IntCC::Equal,
                                        tag,
                                        NativeTag::Heap as i64,
                                    );
                                    let match_block = builder.create_block();
                                    builder.ins().brif(
                                        is_heap,
                                        match_block,
                                        &[],
                                        blocks[fail_idx],
                                        &fail_args,
                                    );
                                    builder.switch_to_block(match_block);
                                    payload
                                }
                                _ => {
                                    jit_fail!(
                                        func,
                                        Some(ip),
                                        Some(&func.code[ip]),
                                        "match on non-enum"
                                    );
                                }
                            };
                            let variant_handle =
                                NativeValue::intern_string(name.name.clone(), heap).payload;
                            let count = u32::try_from(args.len()).ok()?;
                            let payload_ptr = if count == 0 {
                                builder.ins().iconst(pointer_ty, 0)
                            } else {
                                let slot_size = count.checked_mul(NATIVE_VALUE_SIZE as u32)?;
                                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                                    StackSlotKind::ExplicitSlot,
                                    slot_size,
                                    NATIVE_VALUE_ALIGN_SHIFT,
                                ));
                                builder.ins().stack_addr(pointer_ty, slot, 0)
                            };
                            let func_ref =
                                module.declare_func_in_func(hostcalls.match_enum, builder.func);
                            let variant_val =
                                builder.ins().iconst(types::I64, variant_handle as i64);
                            let len_val = builder.ins().iconst(types::I64, count as i64);
                            let call = builder.ins().call(
                                func_ref,
                                &[heap_ptr, enum_handle, variant_val, payload_ptr, len_val],
                            );
                            let matched = builder.inst_results(call)[0];
                            let is_match =
                                builder.ins().icmp_imm(IntCC::NotEqual, matched, 0);
                            let bind_block = builder.create_block();
                            builder.ins().brif(
                                is_match,
                                bind_block,
                                &[],
                                blocks[fail_idx],
                                &fail_args,
                            );
                            builder.switch_to_block(bind_block);
                            for (idx, arg) in args.iter().enumerate() {
                                match &arg.kind {
                                    PatternKind::Wildcard => {}
                                    PatternKind::Ident(ident) => {
                                        let Some(bind_slot) =
                                            binding_map.get(ident.name.as_str())
                                        else {
                                            continue;
                                        };
                                        if *local_types.get(*bind_slot)? != JitType::Value {
                                            jit_fail!(
                                                func,
                                                Some(ip),
                                                Some(&func.code[ip]),
                                                "match binding type mismatch"
                                            );
                                        }
                                        let target_var = *locals.get(*bind_slot)?;
                                        let offset =
                                            i32::try_from(idx).ok()?.checked_mul(NATIVE_VALUE_SIZE)?;
                                        let src_ptr = builder
                                            .ins()
                                            .iadd_imm(payload_ptr, i64::from(offset));
                                        let dst_ptr = builder.use_var(target_var);
                                        copy_native_value(&mut builder, src_ptr, dst_ptr);
                                    }
                                    _ => jit_fail!(
                                        func,
                                        Some(ip),
                                        Some(&func.code[ip]),
                                        "unsupported match arg"
                                    ),
                                }
                            }
                            let ok_args = coerce_stack_args(
                                &mut builder,
                                pointer_ty,
                                &stack,
                                entry_stacks.get(ok_idx)?,
                            )?;
                            builder.ins().jump(blocks[ok_idx], &ok_args);
                            terminated = true;
                            break;
                        }
                        _ => jit_fail!(
                            func,
                            Some(ip),
                            Some(&func.code[ip]),
                            "unsupported match pattern"
                        ),
                    }
                }
                Instr::LoadConfigField { config, field } => {
                    let config_handle = NativeValue::intern_string(config.clone(), heap).payload;
                    let field_handle = NativeValue::intern_string(field.clone(), heap).payload;
                    let result_kind =
                        config_field_jit_kind(program, config, field).unwrap_or(JitType::Value);
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (NATIVE_VALUE_SIZE * 2) as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                    let config_val = StackValue {
                        value: builder.ins().iconst(types::I64, config_handle as i64),
                        kind: JitType::Heap,
                    };
                    store_native_value(&mut builder, base, 0, config_val)?;
                    let field_val = StackValue {
                        value: builder.ins().iconst(types::I64, field_handle as i64),
                        kind: JitType::Heap,
                    };
                    store_native_value(&mut builder, base, NATIVE_VALUE_SIZE, field_val)?;
                    let len_val = builder.ins().iconst(types::I64, 2);
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let builtin_out_ptr = builder.ins().stack_addr(pointer_ty, out_slot, 0);
                    let func_ref = module.declare_func_in_func(hostcalls.config_get, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, base, len_val, builtin_out_ptr]);
                    let status = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(load_call_result(&mut builder, builtin_out_ptr, result_kind)?);
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [
                        BlockArg::Value(status),
                        BlockArg::Value(builtin_out_ptr),
                    ];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
                }
                Instr::IterInit => {
                    let value = stack.pop()?;
                    let value_ptr = if value.kind == JitType::Value {
                        value.value
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            NATIVE_VALUE_SIZE as u32,
                            NATIVE_VALUE_ALIGN_SHIFT,
                        ));
                        let ptr = builder.ins().stack_addr(pointer_ty, slot, 0);
                        store_native_value(&mut builder, ptr, 0, value)?;
                        ptr
                    };
                    let iter_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let iter_out_ptr = builder.ins().stack_addr(pointer_ty, iter_slot, 0);
                    let func_ref = module.declare_func_in_func(hostcalls.iter_init, builder.func);
                    let call = builder.ins().call(func_ref, &[heap_ptr, value_ptr, iter_out_ptr]);
                    let status = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(StackValue {
                        value: iter_out_ptr,
                        kind: JitType::Value,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [BlockArg::Value(status), BlockArg::Value(iter_out_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
                }
                Instr::IterNext { jump } => {
                    let iter_value = stack.pop()?;
                    let iter_ptr = if iter_value.kind == JitType::Value {
                        iter_value.value
                    } else {
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            NATIVE_VALUE_SIZE as u32,
                            NATIVE_VALUE_ALIGN_SHIFT,
                        ));
                        let ptr = builder.ins().stack_addr(pointer_ty, slot, 0);
                        store_native_value(&mut builder, ptr, 0, iter_value)?;
                        ptr
                    };
                    let item_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let item_out_ptr = builder.ins().stack_addr(pointer_ty, item_slot, 0);
                    let func_ref = module.declare_func_in_func(hostcalls.iter_next, builder.func);
                    let call = builder.ins().call(func_ref, &[heap_ptr, iter_ptr, item_out_ptr]);
                    let status = builder.inst_results(call)[0];

                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let done_idx = *block_for_start.get(jump)?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(StackValue {
                        value: item_out_ptr,
                        kind: JitType::Value,
                    });
                    ok_stack.push(StackValue {
                        value: iter_ptr,
                        kind: JitType::Value,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let done_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &stack,
                        entry_stacks.get(done_idx)?,
                    )?;

                    let not_ok_block = builder.create_block();
                    builder.append_block_param(not_ok_block, types::I8);
                    builder.append_block_param(not_ok_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let not_ok_args = [BlockArg::Value(status), BlockArg::Value(item_out_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        not_ok_block,
                        &not_ok_args,
                    );
                    builder.switch_to_block(not_ok_block);
                    let status_val = builder.block_params(not_ok_block)[0];
                    let err_out_ptr = builder.block_params(not_ok_block)[1];
                    let is_done = builder.ins().icmp_imm(IntCC::Equal, status_val, 1);
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let err_args = [BlockArg::Value(status_val), BlockArg::Value(err_out_ptr)];
                    builder.ins().brif(
                        is_done,
                        blocks[done_idx],
                        &done_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
                }
                Instr::Call { name, argc, kind } => {
                    let mut args = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        args.push(stack.pop()?);
                    }
                    args.reverse();
                    let count = u32::try_from(args.len()).ok()?;
                    let base = if count == 0 {
                        builder.ins().iconst(pointer_ty, 0)
                    } else {
                        let slot_size = count.checked_mul(NATIVE_VALUE_SIZE as u32)?;
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_size,
                            NATIVE_VALUE_ALIGN_SHIFT,
                        ));
                        let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                        for (idx, arg) in args.iter().cloned().enumerate() {
                            let offset = i32::try_from(idx).ok()?.checked_mul(NATIVE_VALUE_SIZE)?;
                            store_native_value(&mut builder, base, offset, arg)?;
                        }
                        base
                    };
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let call_out_ptr = builder.ins().stack_addr(pointer_ty, out_slot, 0);
                    let (status, result_kind) = match kind {
                        CallKind::Builtin => {
                            let builtin = match name.as_str() {
                                "print" => hostcalls.builtin_print,
                                "log" => hostcalls.builtin_log,
                                "env" => hostcalls.builtin_env,
                                "assert" => hostcalls.builtin_assert,
                                "range" => hostcalls.range,
                                "task.id" => hostcalls.task_id,
                                "task.done" => hostcalls.task_done,
                                "task.cancel" => hostcalls.task_cancel,
                                "db.exec" => hostcalls.db_exec,
                                "db.query" => hostcalls.db_query,
                                "db.one" => hostcalls.db_one,
                                "db.from" => hostcalls.db_from,
                                "query.select" => hostcalls.query_select,
                                "query.where" => hostcalls.query_where,
                                "query.order_by" => hostcalls.query_order_by,
                                "query.limit" => hostcalls.query_limit,
                                "query.one" => hostcalls.query_one,
                                "query.all" => hostcalls.query_all,
                                "query.exec" => hostcalls.query_exec,
                                "query.sql" => hostcalls.query_sql,
                                "query.params" => hostcalls.query_params,
                                "json.encode" => hostcalls.json_encode,
                                "json.decode" => hostcalls.json_decode,
                                _ => jit_fail!(func, Some(ip), Some(&func.code[ip]), "unknown builtin"),
                            };
                            let len_val = builder.ins().iconst(types::I64, count as i64);
                            let func_ref = module.declare_func_in_func(builtin, builder.func);
                            let call = builder
                                .ins()
                                .call(func_ref, &[heap_ptr, base, len_val, call_out_ptr]);
                            (builder.inst_results(call)[0], JitType::Value)
                        }
                        CallKind::Function => {
                            let mut param_kinds = Vec::with_capacity(args.len());
                            for arg in &args {
                                param_kinds.push(arg.kind);
                            }
                            let callee = match program
                                .functions
                                .get(name)
                                .or_else(|| program.apps.get(name))
                                .or_else(|| {
                                    program
                                        .apps
                                        .values()
                                        .find(|func| func.name == name.as_str())
                                }) {
                                Some(callee) => callee,
                                None => jit_fail!(func, Some(ip), Some(&func.code[ip]), "unknown callee"),
                            };
                            if callee.params.len() != args.len() {
                                jit_fail!(func, Some(ip), Some(&func.code[ip]), "callee arity mismatch");
                            }
                            let result_kind = match return_jit_kind(callee.ret.as_ref(), program) {
                                Some(kind) => kind,
                                None => jit_fail!(
                                    func,
                                    Some(ip),
                                    Some(&func.code[ip]),
                                    "callee return type unsupported"
                                ),
                            };
                            let callee_id = match compile_function(
                                module,
                                hostcalls,
                                program,
                                name,
                                callee,
                                &param_kinds,
                                Linkage::Local,
                                heap,
                                state,
                                pending,
                            ) {
                                Some(id) => id,
                                None => jit_fail!(
                                    func,
                                    Some(ip),
                                    Some(&func.code[ip]),
                                    "callee compile failed"
                                ),
                            };
                            let func_ref = module.declare_func_in_func(callee_id, builder.func);
                            let call = builder
                                .ins()
                                .call(func_ref, &[base, call_out_ptr, heap_ptr]);
                            (builder.inst_results(call)[0], result_kind)
                        }
                    };
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(load_call_result(&mut builder, call_out_ptr, result_kind)?);
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [BlockArg::Value(status), BlockArg::Value(call_out_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
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
                    if kind == JitType::Value {
                        let slot_id = *local_value_slots.get(*slot)?.as_ref()?;
                        let ptr = builder.ins().stack_addr(pointer_ty, slot_id, 0);
                        store_native_value(&mut builder, ptr, 0, value)?;
                        builder.def_var(var, ptr);
                    } else {
                        if value.kind != kind {
                            jit_fail!(
                                func,
                                Some(ip),
                                Some(&func.code[ip]),
                                "store kind mismatch"
                            );
                        }
                        builder.def_var(var, value.value);
                    }
                }
                Instr::GetField { field } => {
                    let base = stack.pop()?;
                    let base_handle = match base.kind {
                        JitType::Struct => base.value,
                        JitType::Value => builder.ins().load(
                            types::I64,
                            MemFlags::new(),
                            base.value,
                            NATIVE_VALUE_PAYLOAD_OFFSET,
                        ),
                        _ => return None,
                    };
                    let field_handle = NativeValue::intern_string(field.clone(), heap).payload;
                    let field_val = builder.ins().iconst(types::I64, field_handle as i64);
                    let func_ref =
                        module.declare_func_in_func(hostcalls.get_struct_field, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, base_handle, field_val]);
                    let handle = builder.inst_results(call)[0];
                    stack.push(StackValue {
                        value: handle,
                        kind: JitType::Heap,
                    });
                }
                Instr::GetIndex => {
                    let index = stack.pop()?;
                    let base = stack.pop()?;
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (NATIVE_VALUE_SIZE * 2) as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let base_ptr = builder.ins().stack_addr(pointer_ty, slot, 0);
                    store_native_value(&mut builder, base_ptr, 0, base)?;
                    store_native_value(&mut builder, base_ptr, NATIVE_VALUE_SIZE, index)?;
                    let len_val = builder.ins().iconst(types::I64, 2);
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let idx_out_ptr = builder.ins().stack_addr(pointer_ty, out_slot, 0);
                    let func_ref = module.declare_func_in_func(hostcalls.get_index, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, base_ptr, len_val, idx_out_ptr]);
                    let status = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(StackValue {
                        value: idx_out_ptr,
                        kind: JitType::Value,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [BlockArg::Value(status), BlockArg::Value(idx_out_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
                }
                Instr::SetIndex => {
                    let value = stack.pop()?;
                    let index = stack.pop()?;
                    let base = stack.pop()?;
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (NATIVE_VALUE_SIZE * 3) as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let base_ptr = builder.ins().stack_addr(pointer_ty, slot, 0);
                    store_native_value(&mut builder, base_ptr, 0, base)?;
                    store_native_value(&mut builder, base_ptr, NATIVE_VALUE_SIZE, index)?;
                    store_native_value(&mut builder, base_ptr, NATIVE_VALUE_SIZE * 2, value)?;
                    let len_val = builder.ins().iconst(types::I64, 3);
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let set_out_ptr = builder.ins().stack_addr(pointer_ty, out_slot, 0);
                    let func_ref = module.declare_func_in_func(hostcalls.set_index, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, base_ptr, len_val, set_out_ptr]);
                    let status = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(StackValue {
                        value: set_out_ptr,
                        kind: JitType::Value,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [BlockArg::Value(status), BlockArg::Value(set_out_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
                }
                Instr::SetField { field } => {
                    let value = stack.pop()?;
                    let base = stack.pop()?;
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (NATIVE_VALUE_SIZE * 3) as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let base_ptr = builder.ins().stack_addr(pointer_ty, slot, 0);
                    store_native_value(&mut builder, base_ptr, 0, base)?;
                    let field_handle = NativeValue::intern_string(field.clone(), heap).payload;
                    let field_val = StackValue {
                        value: builder.ins().iconst(types::I64, field_handle as i64),
                        kind: JitType::Heap,
                    };
                    store_native_value(&mut builder, base_ptr, NATIVE_VALUE_SIZE, field_val)?;
                    store_native_value(&mut builder, base_ptr, NATIVE_VALUE_SIZE * 2, value)?;
                    let len_val = builder.ins().iconst(types::I64, 3);
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let set_out_ptr = builder.ins().stack_addr(pointer_ty, out_slot, 0);
                    let func_ref = module.declare_func_in_func(hostcalls.set_field, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, base_ptr, len_val, set_out_ptr]);
                    let status = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(StackValue {
                        value: set_out_ptr,
                        kind: JitType::Value,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [BlockArg::Value(status), BlockArg::Value(set_out_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
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
                Instr::Add => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    if let Some(out) = numeric_binop(&mut builder, &lhs, &rhs, &func.code[ip]) {
                        stack.push(out);
                        continue;
                    }
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (NATIVE_VALUE_SIZE * 2) as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                    store_native_value(&mut builder, base, 0, lhs)?;
                    store_native_value(&mut builder, base, NATIVE_VALUE_SIZE, rhs)?;
                    let len_val = builder.ins().iconst(types::I64, 2);
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let add_out_ptr = builder.ins().stack_addr(pointer_ty, out_slot, 0);
                    let func_ref = module.declare_func_in_func(hostcalls.add, builder.func);
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, base, len_val, add_out_ptr]);
                    let status = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(StackValue {
                        value: add_out_ptr,
                        kind: JitType::Value,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [BlockArg::Value(status), BlockArg::Value(add_out_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
                }
                Instr::Sub | Instr::Mul | Instr::Div | Instr::Mod => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    let out = numeric_binop(&mut builder, &lhs, &rhs, &func.code[ip])?;
                    stack.push(out);
                }
                Instr::Eq | Instr::NotEq | Instr::Lt | Instr::LtEq | Instr::Gt | Instr::GtEq => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    if let Some(out) = compare_op(&mut builder, &lhs, &rhs, &func.code[ip]) {
                        stack.push(out);
                        continue;
                    }
                    if !matches!(func.code[ip], Instr::Eq | Instr::NotEq) {
                        return None;
                    }
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (NATIVE_VALUE_SIZE * 2) as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let base = builder.ins().stack_addr(pointer_ty, slot, 0);
                    store_native_value(&mut builder, base, 0, lhs)?;
                    store_native_value(&mut builder, base, NATIVE_VALUE_SIZE, rhs)?;
                    let len_val = builder.ins().iconst(types::I64, 2);
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let cmp_out_ptr = builder.ins().stack_addr(pointer_ty, out_slot, 0);
                    let func_ref = match func.code[ip] {
                        Instr::Eq => module.declare_func_in_func(hostcalls.eq, builder.func),
                        _ => module.declare_func_in_func(hostcalls.not_eq, builder.func),
                    };
                    let call =
                        builder.ins().call(func_ref, &[heap_ptr, base, len_val, cmp_out_ptr]);
                    let status = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    let payload = builder.ins().load(
                        types::I64,
                        MemFlags::new(),
                        cmp_out_ptr,
                        NATIVE_VALUE_PAYLOAD_OFFSET,
                    );
                    ok_stack.push(StackValue {
                        value: payload,
                        kind: JitType::Bool,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [BlockArg::Value(status), BlockArg::Value(cmp_out_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
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
                    let idx = *block_for_start.get(target)?;
                    let args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &stack,
                        entry_stacks.get(idx)?,
                    )?;
                    builder.ins().jump(blocks[idx], &args);
                    terminated = true;
                    break;
                }
                Instr::JumpIfFalse(target) => {
                    let cond = stack.pop()?;
                    if cond.kind != JitType::Bool {
                        return None;
                    }
                    let is_false = builder.ins().icmp_imm(IntCC::Equal, cond.value, 0);
                    let then_idx = *block_for_start.get(target)?;
                    let else_ip = ip + 1;
                    let else_idx = *block_for_start.get(&else_ip)?;
                    let then_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &stack,
                        entry_stacks.get(then_idx)?,
                    )?;
                    let else_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &stack,
                        entry_stacks.get(else_idx)?,
                    )?;
                    builder
                        .ins()
                        .brif(is_false, blocks[then_idx], &then_args, blocks[else_idx], &else_args);
                    terminated = true;
                    break;
                }
                Instr::JumpIfNull(target) => {
                    let value = stack.pop()?;
                    let is_null = match value.kind {
                        JitType::Null => {
                            let one = builder.ins().iconst(types::I64, 1);
                            builder.ins().icmp_imm(IntCC::Equal, one, 1)
                        }
                        JitType::Value => {
                            let tag =
                                builder.ins().load(types::I64, MemFlags::new(), value.value, 0);
                            builder
                                .ins()
                                .icmp_imm(IntCC::Equal, tag, NativeTag::Null as i64)
                        }
                        _ => {
                            let zero = builder.ins().iconst(types::I64, 0);
                            builder.ins().icmp_imm(IntCC::Equal, zero, 1)
                        }
                    };
                    let then_idx = *block_for_start.get(target)?;
                    let else_ip = ip + 1;
                    let else_idx = *block_for_start.get(&else_ip)?;
                    let cond = is_null;
                    let then_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &stack,
                        entry_stacks.get(then_idx)?,
                    )?;
                    let else_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &stack,
                        entry_stacks.get(else_idx)?,
                    )?;
                    builder
                        .ins()
                        .brif(cond, blocks[then_idx], &then_args, blocks[else_idx], &else_args);
                    terminated = true;
                    break;
                }
                Instr::Return => {
                    let value = stack.pop()?;
                    stack.clear();
                    write_native_return(&mut builder, out_ptr, ret, value)?;
                    let status = builder.ins().iconst(types::I8, 0);
                    builder.ins().return_(&[status]);
                    terminated = true;
                    break;
                }
                Instr::Bang { has_error } => {
                    let err_value = if *has_error {
                        Some(stack.pop()?)
                    } else {
                        None
                    };
                    let value = stack.pop()?;
                    let value_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let value_ptr = builder.ins().stack_addr(pointer_ty, value_slot, 0);
                    store_native_value(&mut builder, value_ptr, 0, value)?;

                    let err_ptr = if let Some(err_value) = err_value {
                        let err_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            NATIVE_VALUE_SIZE as u32,
                            NATIVE_VALUE_ALIGN_SHIFT,
                        ));
                        let err_ptr = builder.ins().stack_addr(pointer_ty, err_slot, 0);
                        store_native_value(&mut builder, err_ptr, 0, err_value)?;
                        err_ptr
                    } else {
                        builder.ins().iconst(pointer_ty, 0)
                    };
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        NATIVE_VALUE_SIZE as u32,
                        NATIVE_VALUE_ALIGN_SHIFT,
                    ));
                    let out_slot_ptr = builder.ins().stack_addr(pointer_ty, out_slot, 0);
                    let has_err_flag =
                        builder
                            .ins()
                            .iconst(types::I64, if *has_error { 1 } else { 0 });
                    let func_ref = module.declare_func_in_func(hostcalls.bang, builder.func);
                    let call = builder.ins().call(
                        func_ref,
                        &[heap_ptr, value_ptr, err_ptr, has_err_flag, out_slot_ptr],
                    );
                    let status = builder.inst_results(call)[0];
                    let ok_idx = *block_for_start.get(&(ip + 1))?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(StackValue {
                        value: out_slot_ptr,
                        kind: JitType::Value,
                    });
                    let ok_args = coerce_stack_args(
                        &mut builder,
                        pointer_ty,
                        &ok_stack,
                        entry_stacks.get(ok_idx)?,
                    )?;
                    let err_block = builder.create_block();
                    builder.append_block_param(err_block, types::I8);
                    builder.append_block_param(err_block, pointer_ty);
                    let is_ok = builder.ins().icmp_imm(IntCC::Equal, status, 0);
                    let err_args = [BlockArg::Value(status), BlockArg::Value(out_slot_ptr)];
                    builder.ins().brif(
                        is_ok,
                        blocks[ok_idx],
                        &ok_args,
                        err_block,
                        &err_args,
                    );
                    builder.switch_to_block(err_block);
                    let status_val = builder.block_params(err_block)[0];
                    let err_out_ptr = builder.block_params(err_block)[1];
                    copy_native_value(&mut builder, err_out_ptr, out_ptr);
                    builder.ins().return_(&[status_val]);
                    terminated = true;
                    break;
                }
                Instr::RuntimeError(message) => {
                    let handle = NativeValue::intern_string(message.clone(), heap).payload;
                    let tag = builder.ins().iconst(types::I64, NativeTag::Heap as i64);
                    let payload = builder.ins().iconst(types::I64, handle as i64);
                    builder.ins().store(MemFlags::new(), tag, out_ptr, 0);
                    builder
                        .ins()
                        .store(MemFlags::new(), payload, out_ptr, NATIVE_VALUE_PAYLOAD_OFFSET);
                    let status = builder.ins().iconst(types::I8, 2);
                    builder.ins().return_(&[status]);
                    terminated = true;
                    break;
                }
                _ => jit_fail!(
                    func,
                    Some(ip),
                    Some(&func.code[ip]),
                    "unsupported instruction"
                ),
            }
        }

        if !terminated {
            if block_idx + 1 < blocks.len() {
                let next_idx = block_idx + 1;
                let args = coerce_stack_args(
                    &mut builder,
                    pointer_ty,
                    &stack,
                    entry_stacks.get(next_idx)?,
                )?;
                builder.ins().jump(blocks[next_idx], &args);
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

    module.define_function(id, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    state.in_progress.remove(&key);
    state.compiled.insert(key.clone());
    pending.push(PendingCompiled {
        id,
        arity: func.params.len(),
        key,
    });
    Some(id)
}

fn block_starts(code: &[Instr]) -> Option<Vec<usize>> {
    let mut starts = BTreeSet::new();
    starts.insert(0usize);
    for (ip, instr) in code.iter().enumerate() {
        match instr {
            Instr::Jump(target)
            | Instr::JumpIfFalse(target)
            | Instr::JumpIfNull(target)
            | Instr::MatchLocal { jump: target, .. } => {
                if *target >= code.len() {
                    return None;
                }
                starts.insert(*target);
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            Instr::Bang { .. } => {
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            Instr::IterInit => {
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            Instr::IterNext { jump } => {
                if *jump >= code.len() {
                    return None;
                }
                starts.insert(*jump);
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            Instr::MakeStruct { .. } => {
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            Instr::Add
            | Instr::Eq
            | Instr::NotEq
            | Instr::GetIndex
            | Instr::SetIndex
            | Instr::SetField { .. } => {
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            Instr::Call {
                kind: CallKind::Function,
                ..
            } => {
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            Instr::Call {
                kind: CallKind::Builtin,
                name,
                ..
            } if matches!(
                name.as_str(),
                "print"
                    | "log"
                    | "env"
                    | "assert"
                    | "range"
                    | "task.id"
                    | "task.done"
                    | "task.cancel"
                    | "db.exec"
                    | "db.query"
                    | "db.one"
                    | "db.from"
                    | "query.select"
                    | "query.where"
                    | "query.order_by"
                    | "query.limit"
                    | "query.one"
                    | "query.all"
                    | "query.exec"
                    | "query.sql"
                    | "query.params"
                    | "json.encode"
                    | "json.decode"
            ) => {
                if ip + 1 < code.len() {
                    starts.insert(ip + 1);
                }
            }
            Instr::LoadConfigField { .. } => {
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
    let (tag_value, payload) = match ret {
        _ if value.kind == JitType::Unit => stack_tag_payload(builder, value)?,
        _ if value.kind == JitType::Value => stack_tag_payload(builder, value)?,
        ReturnKind::Int if value.kind == JitType::Int => stack_tag_payload(builder, value)?,
        ReturnKind::Bool if value.kind == JitType::Bool => stack_tag_payload(builder, value)?,
        ReturnKind::Float if value.kind == JitType::Float => stack_tag_payload(builder, value)?,
        ReturnKind::Heap
            if matches!(
                value.kind,
                JitType::Heap | JitType::Struct | JitType::Enum | JitType::Boxed | JitType::Null
            ) =>
        {
            stack_tag_payload(builder, value)?
        }
        ReturnKind::Value => stack_tag_payload(builder, value)?,
        _ => return None,
    };
    builder
        .ins()
        .store(MemFlags::new(), tag_value, out_ptr, 0);
    builder.ins().store(
        MemFlags::new(),
        payload,
        out_ptr,
        NATIVE_VALUE_PAYLOAD_OFFSET,
    );
    Some(())
}

fn load_call_result(
    builder: &mut FunctionBuilder<'_>,
    result_ptr: ClifValue,
    kind: JitType,
) -> Option<StackValue> {
    match kind {
        JitType::Value => Some(StackValue {
            value: result_ptr,
            kind,
        }),
        JitType::Unit => Some(StackValue {
            value: builder.ins().iconst(types::I64, 0),
            kind,
        }),
        JitType::Float => {
            let payload = builder.ins().load(
                types::I64,
                MemFlags::new(),
                result_ptr,
                NATIVE_VALUE_PAYLOAD_OFFSET,
            );
            let value = builder.ins().bitcast(types::F64, MemFlags::new(), payload);
            Some(StackValue { value, kind })
        }
        JitType::Int
        | JitType::Bool
        | JitType::Null
        | JitType::Heap
        | JitType::Struct
        | JitType::Enum
        | JitType::Boxed => {
            let payload = builder.ins().load(
                types::I64,
                MemFlags::new(),
                result_ptr,
                NATIVE_VALUE_PAYLOAD_OFFSET,
            );
            Some(StackValue { value: payload, kind })
        }
    }
}

fn stack_tag_payload(
    builder: &mut FunctionBuilder<'_>,
    value: StackValue,
) -> Option<(ClifValue, ClifValue)> {
    match value.kind {
        JitType::Int => Some((
            builder.ins().iconst(types::I64, NativeTag::Int as i64),
            value.value,
        )),
        JitType::Bool => Some((
            builder.ins().iconst(types::I64, NativeTag::Bool as i64),
            value.value,
        )),
        JitType::Float => Some((
            builder.ins().iconst(types::I64, NativeTag::Float as i64),
            builder
                .ins()
                .bitcast(types::I64, MemFlags::new(), value.value),
        )),
        JitType::Null => Some((
            builder.ins().iconst(types::I64, NativeTag::Null as i64),
            builder.ins().iconst(types::I64, 0),
        )),
        JitType::Unit => Some((
            builder.ins().iconst(types::I64, NativeTag::Unit as i64),
            builder.ins().iconst(types::I64, 0),
        )),
        JitType::Heap | JitType::Struct | JitType::Enum | JitType::Boxed => Some((
            builder.ins().iconst(types::I64, NativeTag::Heap as i64),
            value.value,
        )),
        JitType::Value => {
            let tag = builder.ins().load(types::I64, MemFlags::new(), value.value, 0);
            let payload = builder.ins().load(
                types::I64,
                MemFlags::new(),
                value.value,
                NATIVE_VALUE_PAYLOAD_OFFSET,
            );
            Some((tag, payload))
        }
    }
}

fn store_native_value(
    builder: &mut FunctionBuilder<'_>,
    base_ptr: ClifValue,
    offset: i32,
    value: StackValue,
) -> Option<()> {
    let (tag_value, payload) = stack_tag_payload(builder, value)?;
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
        TypeRefKind::Simple(name) if name.name == "Unit" => Some(ReturnKind::Value),
        TypeRefKind::Simple(name) if name.name == "String" => Some(ReturnKind::Heap),
        TypeRefKind::Simple(name) if program.types.contains_key(&name.name) => {
            Some(ReturnKind::Heap)
        }
        TypeRefKind::Simple(name) if program.enums.contains_key(&name.name) => {
            Some(ReturnKind::Heap)
        }
        TypeRefKind::Optional(_) => Some(ReturnKind::Value),
        TypeRefKind::Result { ok, .. } => return_kind(ok, program),
            TypeRefKind::Refined { base, .. } if base.name == "Int" => Some(ReturnKind::Int),
            TypeRefKind::Refined { base, .. } if base.name == "Float" => Some(ReturnKind::Float),
            TypeRefKind::Refined { base, .. } if base.name == "Unit" => Some(ReturnKind::Value),
            TypeRefKind::Refined { base, .. } if base.name == "String" => Some(ReturnKind::Heap),
        TypeRefKind::Refined { base, .. } if program.types.contains_key(&base.name) => {
            Some(ReturnKind::Heap)
        }
        TypeRefKind::Generic { base, .. }
            if base.name == "List" || base.name == "Map" || base.name == "Task" =>
        {
            Some(ReturnKind::Heap)
        }
        TypeRefKind::Generic { base, args } if base.name == "Option" => {
            let _ = args;
            Some(ReturnKind::Value)
        }
        TypeRefKind::Generic { base, args } if base.name == "Result" => {
            let ok = args.get(0)?;
            return_kind(ok, program)
        }
        _ => None,
    }
}

fn return_jit_kind(ret: Option<&TypeRef>, program: &IrProgram) -> Option<JitType> {
    let ty = match ret {
        Some(ty) => ty,
        None => return Some(JitType::Value),
    };
    Some(jit_kind_for_type_ref(ty, program))
}

fn jit_kind_for_type_ref(ty: &TypeRef, program: &IrProgram) -> JitType {
    match &ty.kind {
        TypeRefKind::Simple(name) => jit_kind_for_name(&name.name, program),
        TypeRefKind::Optional(_) => JitType::Value,
        TypeRefKind::Result { .. } => JitType::Value,
        TypeRefKind::Refined { base, .. } => jit_kind_for_name(&base.name, program),
        TypeRefKind::Generic { base, .. }
            if base.name == "List" || base.name == "Map" || base.name == "Task" =>
        {
            JitType::Heap
        }
        TypeRefKind::Generic { base, .. } if base.name == "Option" => JitType::Value,
        TypeRefKind::Generic { base, .. } if base.name == "Result" => JitType::Value,
        _ => JitType::Value,
    }
}

fn jit_kind_for_name(name: &str, program: &IrProgram) -> JitType {
    match name {
        "Int" => JitType::Int,
        "Bool" => JitType::Bool,
        "Float" => JitType::Float,
        "Unit" => JitType::Unit,
        "String" => JitType::Heap,
        _ if program.types.contains_key(name) => JitType::Struct,
        _ if program.enums.contains_key(name) => JitType::Enum,
        _ => JitType::Value,
    }
}

fn config_field_jit_kind(program: &IrProgram, config: &str, field: &str) -> Option<JitType> {
    let config = program.configs.get(config)?;
    let field_info = config.fields.iter().find(|info| info.name == field)?;
    Some(jit_kind_for_type_ref(&field_info.ty, program))
}

fn value_kind(value: &Value) -> Option<JitType> {
        match value {
            Value::Unit => Some(JitType::Unit),
            Value::Int(_) => Some(JitType::Int),
            Value::Bool(_) => Some(JitType::Bool),
        Value::Float(_) => Some(JitType::Float),
        Value::Null
        | Value::String(_)
        | Value::List(_)
        | Value::Map(_)
        | Value::Struct { .. }
        | Value::Enum { .. }
        | Value::ResultOk(_)
        | Value::ResultErr(_)
        | Value::Boxed(_)
        | Value::Task(_)
        | Value::Query(_)
        | Value::Iterator(_) => Some(JitType::Value),
        _ => None,
    }
}

fn clif_type(kind: &JitType, pointer_ty: types::Type) -> types::Type {
    match kind {
        JitType::Float => types::F64,
        JitType::Value => pointer_ty,
        JitType::Unit => types::I64,
        _ => types::I64,
    }
}

fn analyze_types(
    func: &Function,
    param_types: &[JitType],
    starts: &[usize],
    program: &IrProgram,
) -> Option<(Vec<JitType>, Vec<Vec<JitType>>)> {
    let mut locals: Vec<Option<JitType>> = vec![None; func.locals];
    for (idx, kind) in param_types.iter().enumerate() {
        if idx < locals.len() {
            locals[idx] = Some(*kind);
        }
    }
    let mut block_for_start = HashMap::new();
    for (idx, start) in starts.iter().enumerate() {
        block_for_start.insert(*start, idx);
    }
    let mut entry_stacks: Vec<Option<Vec<JitType>>> = vec![None; starts.len()];
    entry_stacks[0] = Some(Vec::new());
    let mut worklist: Vec<usize> = vec![0];
    while let Some(block_idx) = worklist.pop() {
        let start = *starts.get(block_idx)?;
        let end = if block_idx + 1 < starts.len() {
            starts[block_idx + 1]
        } else {
            func.code.len()
        };
        let mut stack = entry_stacks.get(block_idx)?.as_ref()?.clone();
        let mut terminated = false;
        let mut ip = start;
        while ip < end {
            match &func.code[ip] {
                Instr::Push(Const::Unit) => stack.push(JitType::Unit),
                Instr::Push(Const::Int(_)) => stack.push(JitType::Int),
                Instr::Push(Const::Bool(_)) => stack.push(JitType::Bool),
                Instr::Push(Const::Float(_)) => stack.push(JitType::Float),
                Instr::Push(Const::String(_)) => stack.push(JitType::Heap),
                Instr::Push(Const::Null) => stack.push(JitType::Null),
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
                        let _ = stack.pop()?;
                    }
                    stack.push(JitType::Struct);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::MakeEnum { argc, .. } => {
                    for _ in 0..*argc {
                        stack.pop()?;
                    }
                    stack.push(JitType::Enum);
                }
                Instr::MakeBox => {
                    let _value = stack.pop()?;
                    stack.push(JitType::Boxed);
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
                Instr::LoadConfigField { config, field } => {
                    let result_kind =
                        config_field_jit_kind(program, config, field).unwrap_or(JitType::Value);
                    stack.push(result_kind);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::IterInit => {
                    let _ = stack.pop()?;
                    stack.push(JitType::Value);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::IterNext { jump } => {
                    let _ = stack.pop()?;
                    let done_idx = *block_for_start.get(jump)?;
                    merge_block_stack(
                        &mut entry_stacks[done_idx],
                        &stack,
                        &mut worklist,
                        done_idx,
                    )?;
                    let mut ok_stack = stack.clone();
                    ok_stack.push(JitType::Value);
                    ok_stack.push(JitType::Value);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &ok_stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::StoreLocal(slot) => {
                    let kind = stack.pop()?;
                    match locals.get_mut(*slot)? {
                        Some(existing) => {
                            let merged = merge_kind(*existing, kind)?;
                            if merged != *existing {
                                *existing = merged;
                                if !worklist.contains(&block_idx) {
                                    worklist.push(block_idx);
                                }
                            }
                        }
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
                Instr::Add => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    if let Some(out) = numeric_kind(lhs, rhs, &func.code[ip]) {
                        stack.push(out);
                    } else {
                        stack.push(JitType::Value);
                        let ok_ip = ip + 1;
                        let ok_idx = *block_for_start.get(&ok_ip)?;
                        merge_block_stack(
                            &mut entry_stacks[ok_idx],
                            &stack,
                            &mut worklist,
                            ok_idx,
                        )?;
                        terminated = true;
                        break;
                    }
                }
                Instr::Sub | Instr::Mul | Instr::Div | Instr::Mod => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    let out = numeric_kind(lhs, rhs, &func.code[ip])?;
                    stack.push(out);
                }
                Instr::Eq | Instr::NotEq | Instr::Lt | Instr::LtEq | Instr::Gt | Instr::GtEq => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    if let Some(out) = compare_kind(lhs, rhs, &func.code[ip]) {
                        stack.push(out);
                    } else if matches!(func.code[ip], Instr::Eq | Instr::NotEq) {
                        stack.push(JitType::Bool);
                    } else {
                        return None;
                    }
                }
                Instr::And | Instr::Or => {
                    let rhs = stack.pop()?;
                    let lhs = stack.pop()?;
                    if lhs != JitType::Bool || rhs != JitType::Bool {
                        return None;
                    }
                    stack.push(JitType::Bool);
                }
                Instr::Jump(target) => {
                    let target_idx = *block_for_start.get(target)?;
                    merge_block_stack(
                        &mut entry_stacks[target_idx],
                        &stack,
                        &mut worklist,
                        target_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::Call { name, argc, kind } => {
                    let result_kind = match kind {
                        CallKind::Builtin => {
                            match name.as_str() {
                            "print"
                            | "log"
                            | "env"
                            | "assert"
                            | "range"
                            | "task.id"
                            | "task.done"
                            | "task.cancel"
                            | "db.exec"
                            | "db.query"
                            | "db.one"
                            | "db.from"
                            | "query.select"
                            | "query.where"
                            | "query.order_by"
                            | "query.limit"
                            | "query.one"
                            | "query.all"
                            | "query.exec"
                            | "query.sql"
                            | "query.params"
                            | "json.encode"
                            | "json.decode" => {}
                            _ => return None,
                            }
                            JitType::Value
                        }
                        CallKind::Function => {
                            let callee = program
                                .functions
                                .get(name)
                                .or_else(|| program.apps.get(name))
                                .or_else(|| {
                                    program
                                        .apps
                                        .values()
                                        .find(|func| func.name == name.as_str())
                                })?;
                            return_jit_kind(callee.ret.as_ref(), program)?
                        }
                    };
                    for _ in 0..*argc {
                        let _ = stack.pop()?;
                    }
                    stack.push(result_kind);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::JumpIfFalse(target) => {
                    let cond = stack.pop()?;
                    if cond != JitType::Bool {
                        return None;
                    }
                    let target_idx = *block_for_start.get(target)?;
                    merge_block_stack(
                        &mut entry_stacks[target_idx],
                        &stack,
                        &mut worklist,
                        target_idx,
                    )?;
                    let else_ip = ip + 1;
                    let else_idx = *block_for_start.get(&else_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[else_idx],
                        &stack,
                        &mut worklist,
                        else_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::MatchLocal { bindings, jump, .. } => {
                    for (_, slot) in bindings {
                        let entry = locals.get_mut(*slot)?;
                        if entry.is_none() {
                            *entry = Some(JitType::Value);
                        }
                    }
                    let target_idx = *block_for_start.get(jump)?;
                    merge_block_stack(
                        &mut entry_stacks[target_idx],
                        &stack,
                        &mut worklist,
                        target_idx,
                    )?;
                    let else_ip = ip + 1;
                    let else_idx = *block_for_start.get(&else_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[else_idx],
                        &stack,
                        &mut worklist,
                        else_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::JumpIfNull(target) => {
                    let _value = stack.pop()?;
                    let target_idx = *block_for_start.get(target)?;
                    merge_block_stack(
                        &mut entry_stacks[target_idx],
                        &stack,
                        &mut worklist,
                        target_idx,
                    )?;
                    let else_ip = ip + 1;
                    let else_idx = *block_for_start.get(&else_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[else_idx],
                        &stack,
                        &mut worklist,
                        else_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::Bang { has_error } => {
                    if *has_error {
                        let _ = stack.pop()?;
                    }
                    let _ = stack.pop()?;
                    stack.push(JitType::Value);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::Return => {
                    let _ = stack.pop()?;
                    terminated = true;
                    break;
                }
                Instr::RuntimeError(_) => {
                    terminated = true;
                    break;
                }
                Instr::GetField { .. } => {
                    let base = stack.pop()?;
                    match base {
                        JitType::Struct | JitType::Value => stack.push(JitType::Heap),
                        _ => return None,
                    }
                }
                Instr::GetIndex => {
                    let _ = stack.pop()?;
                    let _ = stack.pop()?;
                    stack.push(JitType::Value);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::SetIndex => {
                    let _ = stack.pop()?;
                    let _ = stack.pop()?;
                    let _ = stack.pop()?;
                    stack.push(JitType::Value);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                Instr::SetField { .. } => {
                    let _ = stack.pop()?;
                    let _ = stack.pop()?;
                    stack.push(JitType::Value);
                    let ok_ip = ip + 1;
                    let ok_idx = *block_for_start.get(&ok_ip)?;
                    merge_block_stack(
                        &mut entry_stacks[ok_idx],
                        &stack,
                        &mut worklist,
                        ok_idx,
                    )?;
                    terminated = true;
                    break;
                }
                _ => return None,
            }
            ip += 1;
        }
        if !terminated && block_idx + 1 < starts.len() {
            let next_idx = block_idx + 1;
            merge_block_stack(
                &mut entry_stacks[next_idx],
                &stack,
                &mut worklist,
                next_idx,
            )?;
        }
    }

    let locals = locals
        .into_iter()
        .map(|kind| kind.unwrap_or(JitType::Int))
        .collect();
    let entry_stacks = entry_stacks
        .into_iter()
        .map(|stack| stack.unwrap_or_default())
        .collect();
    Some((locals, entry_stacks))
}

fn merge_block_stack(
    existing: &mut Option<Vec<JitType>>,
    incoming: &[JitType],
    worklist: &mut Vec<usize>,
    block_idx: usize,
) -> Option<()> {
    match existing {
        Some(stack) => {
            if stack.len() != incoming.len() {
                return None;
            }
            let mut changed = false;
            for (slot, next) in stack.iter_mut().zip(incoming.iter()) {
                let merged = merge_kind(*slot, *next)?;
                if merged != *slot {
                    *slot = merged;
                    changed = true;
                }
            }
            if changed && !worklist.contains(&block_idx) {
                worklist.push(block_idx);
            }
        }
        slot @ None => {
            *slot = Some(incoming.to_vec());
            if !worklist.contains(&block_idx) {
                worklist.push(block_idx);
            }
        }
    }
    Some(())
}

fn merge_kind(lhs: JitType, rhs: JitType) -> Option<JitType> {
    if lhs == rhs {
        return Some(lhs);
    }
    if lhs == JitType::Value || rhs == JitType::Value {
        return Some(JitType::Value);
    }
    if lhs == JitType::Unit || rhs == JitType::Unit {
        return Some(JitType::Value);
    }
    if lhs == JitType::Null || rhs == JitType::Null {
        return Some(JitType::Value);
    }
    None
}

fn coerce_stack_args(
    builder: &mut FunctionBuilder<'_>,
    pointer_ty: types::Type,
    stack: &[StackValue],
    target: &[JitType],
) -> Option<Vec<BlockArg>> {
    if stack.len() != target.len() {
        return None;
    }
    let mut args = Vec::with_capacity(stack.len());
    for (value, target_kind) in stack.iter().zip(target.iter()) {
        if value.kind == *target_kind {
            args.push(BlockArg::Value(value.value));
        } else if *target_kind == JitType::Value {
            let ptr = ensure_value_ptr(builder, pointer_ty, *value)?;
            args.push(BlockArg::Value(ptr));
        } else {
            return None;
        }
    }
    Some(args)
}

fn ensure_value_ptr(
    builder: &mut FunctionBuilder<'_>,
    pointer_ty: types::Type,
    value: StackValue,
) -> Option<ClifValue> {
    if value.kind == JitType::Value {
        return Some(value.value);
    }
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        NATIVE_VALUE_SIZE as u32,
        NATIVE_VALUE_ALIGN_SHIFT,
    ));
    let ptr = builder.ins().stack_addr(pointer_ty, slot, 0);
    store_native_value(builder, ptr, 0, value)?;
    Some(ptr)
}

fn copy_native_value(
    builder: &mut FunctionBuilder<'_>,
    src: ClifValue,
    dst: ClifValue,
) {
    let tag = builder.ins().load(types::I64, MemFlags::new(), src, 0);
    let payload = builder
        .ins()
        .load(types::I64, MemFlags::new(), src, NATIVE_VALUE_PAYLOAD_OFFSET);
    builder.ins().store(MemFlags::new(), tag, dst, 0);
    builder
        .ins()
        .store(MemFlags::new(), payload, dst, NATIVE_VALUE_PAYLOAD_OFFSET);
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

pub(crate) fn jit_symbol(name: &str) -> String {
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
