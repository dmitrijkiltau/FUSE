pub mod check;
pub mod symbols;
pub mod types;

use std::collections::{HashMap, HashSet};

use crate::ast::{FieldDecl, Item, Program, TypeDecl, TypeDerive};
use crate::diag::{Diag, Diagnostics};
use crate::loader::{ModuleId, ModuleRegistry};
use crate::span::Span;

pub struct Analysis {
    pub symbols: symbols::ModuleSymbols,
}

pub fn analyze_program(program: &Program) -> (Analysis, Vec<Diag>) {
    let mut diags = Diagnostics::default();
    let mut expanded = program.clone();
    expand_type_derivations(&mut expanded, &mut diags);
    crate::frontend::canonicalize::canonicalize_program(&mut expanded);
    let symbols = symbols::collect(&expanded, &mut diags);
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
    checker.check_program(&expanded);
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
    (
        Analysis {
            symbols: root_symbols,
        },
        diags.into_vec(),
    )
}

pub fn analyze_module(registry: &ModuleRegistry, module_id: ModuleId) -> (Analysis, Vec<Diag>) {
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

    let symbols = symbols_by_id
        .get(&module_id)
        .cloned()
        .unwrap_or_else(symbols::ModuleSymbols::default);
    if let Some(unit) = registry.modules.get(&module_id) {
        let mut checker = check::Checker::new(
            module_id,
            &symbols,
            &unit.modules,
            &module_maps_by_id,
            &unit.import_items,
            &symbols_by_id,
            &import_items_by_id,
            &mut diags,
        );
        checker.check_program(&unit.program);
    }

    (Analysis { symbols }, diags.into_vec())
}

fn expand_type_derivations(program: &mut Program, diags: &mut Diagnostics) {
    let mut cache: HashMap<String, Vec<FieldDecl>> = HashMap::new();
    let mut visiting: HashSet<String> = HashSet::new();
    let derived: Vec<String> = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Type(decl) if decl.derive.is_some() => Some(decl.name.name.clone()),
            _ => None,
        })
        .collect();

    for name in derived {
        let fields = resolve_derived_fields(program, diags, &name, &mut cache, &mut visiting);
        if let Some(fields) = fields {
            for item in &mut program.items {
                if let Item::Type(decl) = item {
                    if decl.name.name == name {
                        decl.fields = fields.clone();
                        decl.derive = None;
                    }
                }
            }
        }
    }
}

fn resolve_derived_fields(
    program: &Program,
    diags: &mut Diagnostics,
    name: &str,
    cache: &mut HashMap<String, Vec<FieldDecl>>,
    visiting: &mut HashSet<String>,
) -> Option<Vec<FieldDecl>> {
    if let Some(fields) = cache.get(name) {
        return Some(fields.clone());
    }
    if visiting.contains(name) {
        diags.error(
            Span::default(),
            format!("cyclic type derivation for {name}"),
        );
        return None;
    }
    visiting.insert(name.to_string());

    let decl = match find_type_decl(program, name) {
        Some(decl) => decl,
        None => {
            diags.error(Span::default(), format!("unknown type {name}"));
            visiting.remove(name);
            return None;
        }
    };

    let fields = if let Some(derive) = &decl.derive {
        resolve_without_fields(program, diags, derive, cache, visiting)
    } else {
        Some(decl.fields.clone())
    };

    if let Some(fields) = &fields {
        cache.insert(name.to_string(), fields.clone());
    }
    visiting.remove(name);
    fields
}

fn resolve_without_fields(
    program: &Program,
    diags: &mut Diagnostics,
    derive: &TypeDerive,
    cache: &mut HashMap<String, Vec<FieldDecl>>,
    visiting: &mut HashSet<String>,
) -> Option<Vec<FieldDecl>> {
    let base_name = derive.base.name.as_str();
    if base_name.contains('.') {
        diags.error(
            derive.base.span,
            format!("unknown base type {}", derive.base.name),
        );
        return None;
    }
    let base_fields = resolve_derived_fields(program, diags, base_name, cache, visiting)?;

    let mut removed = HashSet::new();
    for field in &derive.without {
        removed.insert(field.name.clone());
    }

    for field in &derive.without {
        if !base_fields.iter().any(|f| f.name.name == field.name) {
            diags.error(
                field.span,
                format!("unknown field {} in {}", field.name, derive.base.name),
            );
        }
    }

    let fields = base_fields
        .into_iter()
        .filter(|field| !removed.contains(&field.name.name))
        .collect();
    Some(fields)
}

fn find_type_decl(program: &Program, name: &str) -> Option<TypeDecl> {
    program.items.iter().find_map(|item| match item {
        Item::Type(decl) if decl.name.name == name => Some(decl.clone()),
        _ => None,
    })
}
