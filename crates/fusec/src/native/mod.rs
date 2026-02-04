use serde::{Deserialize, Serialize};

use crate::interp::Value;
use crate::ir::Program as IrProgram;
use crate::loader::ModuleRegistry;
use crate::vm::Vm;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeProgram {
    pub version: u32,
    pub ir: IrProgram,
}

impl NativeProgram {
    pub const VERSION: u32 = 1;
}

pub fn compile_registry(registry: &ModuleRegistry) -> Result<NativeProgram, Vec<String>> {
    let ir = crate::ir::lower::lower_registry(registry)?;
    Ok(NativeProgram {
        version: NativeProgram::VERSION,
        ir,
    })
}

pub struct NativeVm<'a> {
    vm: Vm<'a>,
}

impl<'a> NativeVm<'a> {
    pub fn new(program: &'a NativeProgram) -> Self {
        Self {
            vm: Vm::new(&program.ir),
        }
    }

    pub fn run_app(&mut self, name: Option<&str>) -> Result<(), String> {
        self.vm.run_app(name)
    }

    pub fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        self.vm.call_function(name, args)
    }
}
