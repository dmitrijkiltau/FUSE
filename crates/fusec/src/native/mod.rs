mod jit;
pub mod value;

use serde::{Deserialize, Serialize};

use crate::interp::{format_error_value, Value};
use crate::ir::Program as IrProgram;
use crate::loader::ModuleRegistry;
use crate::vm::Vm;
use crate::native::value::NativeHeap;
use jit::{JitCallError, JitRuntime};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeProgram {
    pub version: u32,
    pub ir: IrProgram,
}

impl NativeProgram {
    pub const VERSION: u32 = 1;

    pub fn from_ir(ir: IrProgram) -> Self {
        Self {
            version: Self::VERSION,
            ir,
        }
    }
}

pub fn compile_registry(registry: &ModuleRegistry) -> Result<NativeProgram, Vec<String>> {
    let ir = crate::ir::lower::lower_registry(registry)?;
    Ok(NativeProgram::from_ir(ir))
}

pub struct NativeVm<'a> {
    program: &'a NativeProgram,
    vm: Vm<'a>,
    jit: JitRuntime,
    heap: NativeHeap,
}

impl<'a> NativeVm<'a> {
    pub fn new(program: &'a NativeProgram) -> Self {
        Self {
            program,
            vm: Vm::new(&program.ir),
            jit: JitRuntime::build(),
            heap: NativeHeap::new(),
        }
    }

    pub fn run_app(&mut self, name: Option<&str>) -> Result<(), String> {
        self.vm.run_app(name)
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        let func = self
            .program
            .ir
            .functions
            .get(name)
            .ok_or_else(|| format!("unknown function {name}"))?;
        if let Some(result) = self.jit.try_call(&self.program.ir, name, &args, &mut self.heap) {
            let out = match result {
                Ok(value) => Ok(self.wrap_function_result(func, value)),
                Err(JitCallError::Error(err_val)) => {
                    if self.is_result_type(func.ret.as_ref()) {
                        Ok(Value::ResultErr(Box::new(err_val)))
                    } else {
                        Err(format_error_value(&err_val))
                    }
                }
                Err(JitCallError::Runtime(message)) => Err(message),
            };
            self.heap.collect_garbage();
            return out;
        }
        self.heap.collect_garbage();
        self.vm.call_function(name, args)
    }

    pub fn has_jit_function(&self, name: &str) -> bool {
        self.jit.has_function(name)
    }

    fn wrap_function_result(&self, func: &crate::ir::Function, value: Value) -> Value {
        if self.is_result_type(func.ret.as_ref()) {
            match value {
                Value::ResultOk(_) | Value::ResultErr(_) => value,
                _ => Value::ResultOk(Box::new(value)),
            }
        } else {
            value
        }
    }

    fn is_result_type(&self, ty: Option<&crate::ast::TypeRef>) -> bool {
        match ty {
            Some(ty) => match &ty.kind {
                crate::ast::TypeRefKind::Result { .. } => true,
                crate::ast::TypeRefKind::Generic { base, .. } => base.name == "Result",
                _ => false,
            },
            None => false,
        }
    }
}
