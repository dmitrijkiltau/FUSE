pub mod check;
pub mod symbols;
pub mod types;

use crate::ast::Program;
use crate::diag::{Diag, Diagnostics};
use crate::loader::{ModuleId, ModuleRegistry};

pub struct Analysis {
    pub symbols: symbols::ModuleSymbols,
}

pub fn analyze_program(program: &Program) -> (Analysis, Vec<Diag>) {
    let mut diags = Diagnostics::default();
    let symbols = symbols::collect(program, &mut diags);
    let mut symbols_by_id: std::collections::HashMap<ModuleId, symbols::ModuleSymbols> =
        std::collections::HashMap::new();
    symbols_by_id.insert(0, symbols.clone());
    let empty_items: std::collections::HashMap<String, crate::loader::ModuleLink> =
        std::collections::HashMap::new();
    let mut import_items_by_id: std::collections::HashMap<
        ModuleId,
        std::collections::HashMap<String, crate::loader::ModuleLink>,
    > = std::collections::HashMap::new();
    import_items_by_id.insert(0, empty_items.clone());
    let empty_modules = crate::loader::ModuleMap::default();
    let mut module_maps_by_id: std::collections::HashMap<ModuleId, crate::loader::ModuleMap> =
        std::collections::HashMap::new();
    module_maps_by_id.insert(0, empty_modules.clone());
    let mut checker = check::Checker::new(
        0,
        &symbols,
        &empty_modules,
        &module_maps_by_id,
        &empty_items,
        &symbols_by_id,
        &import_items_by_id,
        &mut diags,
    );
    checker.check_program(program);
    (Analysis { symbols }, diags.into_vec())
}

pub fn analyze_registry(registry: &ModuleRegistry) -> (Analysis, Vec<Diag>) {
    let mut diags = Diagnostics::default();
    let mut symbols_by_id: std::collections::HashMap<ModuleId, symbols::ModuleSymbols> =
        std::collections::HashMap::new();
    let mut import_items_by_id: std::collections::HashMap<
        ModuleId,
        std::collections::HashMap<String, crate::loader::ModuleLink>,
    > = std::collections::HashMap::new();
    let mut module_maps_by_id: std::collections::HashMap<ModuleId, crate::loader::ModuleMap> =
        std::collections::HashMap::new();
    for (id, unit) in &registry.modules {
        let symbols = symbols::collect(&unit.program, &mut diags);
        symbols_by_id.insert(*id, symbols);
        import_items_by_id.insert(*id, unit.import_items.clone());
        module_maps_by_id.insert(*id, unit.modules.clone());
    }
    for (id, unit) in &registry.modules {
        let symbols = match symbols_by_id.get(id) {
            Some(symbols) => symbols,
            None => continue,
        };
        let mut checker = check::Checker::new(
            *id,
            symbols,
            &unit.modules,
            &module_maps_by_id,
            &unit.import_items,
            &symbols_by_id,
            &import_items_by_id,
            &mut diags,
        );
        checker.check_program(&unit.program);
    }
    let root_symbols = registry
        .root()
        .and_then(|unit| symbols_by_id.get(&unit.id))
        .cloned()
        .unwrap_or_else(|| symbols::ModuleSymbols::default());
    (Analysis { symbols: root_symbols }, diags.into_vec())
}
