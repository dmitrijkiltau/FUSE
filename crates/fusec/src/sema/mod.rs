pub mod check;
pub mod symbols;
pub mod types;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::ast::{
    Capability, FieldDecl, Item, Program, TypeDecl, TypeDerive, TypeRef, TypeRefKind,
};
use crate::diag::{Diag, Diagnostics};
use crate::loader::{ModuleId, ModuleRegistry};
use crate::span::Span;

pub struct Analysis {
    pub symbols: symbols::ModuleSymbols,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AnalyzeOptions {
    pub strict_architecture: bool,
}

pub fn analyze_program(program: &Program) -> (Analysis, Vec<Diag>) {
    let mut diags = Diagnostics::default();
    let mut expanded = program.clone();
    expand_type_derivations(&mut expanded, &mut diags);
    crate::frontend::canonicalize::canonicalize_program(&mut expanded);
    let declared_caps = collect_declared_capabilities(&expanded, &mut diags);
    let symbols = symbols::collect(&expanded, &mut diags);
    let mut symbols_by_id: std::collections::HashMap<ModuleId, symbols::ModuleSymbols> =
        std::collections::HashMap::new();
    symbols_by_id.insert(0, symbols.clone());
    let mut module_caps_by_id: std::collections::HashMap<
        ModuleId,
        std::collections::HashSet<Capability>,
    > = std::collections::HashMap::new();
    module_caps_by_id.insert(0, declared_caps);
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
        &module_caps_by_id,
        &mut diags,
    );
    checker.check_program(&expanded);
    (Analysis { symbols }, diags.into_vec())
}

pub fn analyze_registry(registry: &ModuleRegistry) -> (Analysis, Vec<Diag>) {
    analyze_registry_with_options(registry, AnalyzeOptions::default())
}

pub fn analyze_registry_with_options(
    registry: &ModuleRegistry,
    options: AnalyzeOptions,
) -> (Analysis, Vec<Diag>) {
    let mut diags = Diagnostics::default();
    let mut symbols_by_id: std::collections::HashMap<ModuleId, symbols::ModuleSymbols> =
        std::collections::HashMap::new();
    let mut module_caps_by_id: std::collections::HashMap<
        ModuleId,
        std::collections::HashSet<Capability>,
    > = std::collections::HashMap::new();
    let mut import_items_by_id: std::collections::HashMap<
        ModuleId,
        std::collections::HashMap<String, crate::loader::ModuleLink>,
    > = std::collections::HashMap::new();
    let mut module_maps_by_id: std::collections::HashMap<ModuleId, crate::loader::ModuleMap> =
        std::collections::HashMap::new();
    let mut used_caps_by_id: std::collections::HashMap<
        ModuleId,
        std::collections::HashSet<Capability>,
    > = std::collections::HashMap::new();
    for (id, unit) in &registry.modules {
        let symbols = symbols::collect(&unit.program, &mut diags);
        symbols_by_id.insert(*id, symbols);
        module_caps_by_id.insert(
            *id,
            collect_declared_capabilities(&unit.program, &mut diags),
        );
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
            &module_caps_by_id,
            &mut diags,
        );
        checker.check_program(&unit.program);
        used_caps_by_id.insert(*id, checker.used_capabilities().clone());
    }
    if options.strict_architecture {
        validate_strict_capability_purity(
            registry,
            &module_caps_by_id,
            &used_caps_by_id,
            &mut diags,
        );
        validate_strict_cross_layer_import_cycles(registry, &mut diags);
        validate_strict_error_domain_isolation(
            registry,
            &module_maps_by_id,
            &import_items_by_id,
            &symbols_by_id,
            &mut diags,
        );
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
    let mut module_caps_by_id: std::collections::HashMap<
        ModuleId,
        std::collections::HashSet<Capability>,
    > = std::collections::HashMap::new();
    let mut import_items_by_id: std::collections::HashMap<
        ModuleId,
        std::collections::HashMap<String, crate::loader::ModuleLink>,
    > = std::collections::HashMap::new();
    let mut module_maps_by_id: std::collections::HashMap<ModuleId, crate::loader::ModuleMap> =
        std::collections::HashMap::new();

    for (id, unit) in &registry.modules {
        let symbols = symbols::collect(&unit.program, &mut diags);
        symbols_by_id.insert(*id, symbols);
        module_caps_by_id.insert(
            *id,
            collect_declared_capabilities(&unit.program, &mut diags),
        );
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
            &module_caps_by_id,
            &mut diags,
        );
        checker.check_program(&unit.program);
    }

    (Analysis { symbols }, diags.into_vec())
}

fn validate_strict_capability_purity(
    registry: &ModuleRegistry,
    declared_caps_by_id: &HashMap<ModuleId, HashSet<Capability>>,
    used_caps_by_id: &HashMap<ModuleId, HashSet<Capability>>,
    diags: &mut Diagnostics,
) {
    for (module_id, declared) in declared_caps_by_id {
        let used = used_caps_by_id.get(module_id).cloned().unwrap_or_default();
        let mut extras: Vec<Capability> = declared.difference(&used).copied().collect();
        extras.sort_by_key(|cap| cap.as_str());
        if extras.is_empty() {
            continue;
        }
        let label = module_label(registry, *module_id);
        let names: Vec<&str> = extras.iter().map(|cap| cap.as_str()).collect();
        let span = registry
            .get(*module_id)
            .and_then(|unit| {
                extras
                    .iter()
                    .find_map(|capability| require_span_for_capability(unit, *capability))
            })
            .unwrap_or_default();
        diags.error(
            span,
            format!(
                "strict architecture: capability purity violation in {label}; remove unused capability declarations: {}",
                names.join(", ")
            ),
        );
    }
}

fn validate_strict_cross_layer_import_cycles(registry: &ModuleRegistry, diags: &mut Diagnostics) {
    let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
    let mut layers: HashSet<String> = HashSet::new();
    for unit in registry.modules.values() {
        let from = module_layer_name(&unit.path);
        layers.insert(from.clone());
        for imported_id in imported_module_ids(unit) {
            let Some(target) = registry.get(imported_id) else {
                continue;
            };
            let to = module_layer_name(&target.path);
            layers.insert(to.clone());
            if from != to {
                edges.entry(from.clone()).or_default().insert(to);
            }
        }
    }
    if let Some(cycle) = find_layer_cycle(&edges, &layers) {
        diags.error(
            Span::default(),
            format!(
                "strict architecture: cross-layer import cycle detected: {}",
                cycle.join(" -> ")
            ),
        );
    }
}

fn validate_strict_error_domain_isolation(
    registry: &ModuleRegistry,
    module_maps_by_id: &HashMap<ModuleId, crate::loader::ModuleMap>,
    import_items_by_id: &HashMap<ModuleId, HashMap<String, crate::loader::ModuleLink>>,
    symbols_by_id: &HashMap<ModuleId, symbols::ModuleSymbols>,
    diags: &mut Diagnostics,
) {
    for (module_id, unit) in &registry.modules {
        let mut owners: HashMap<ModuleId, Vec<Span>> = HashMap::new();
        for item in &unit.program.items {
            match item {
                Item::Fn(decl) => {
                    if let Some(ret) = &decl.ret {
                        collect_error_domain_owners(
                            *module_id,
                            ret,
                            module_maps_by_id,
                            import_items_by_id,
                            symbols_by_id,
                            &mut owners,
                        );
                    }
                }
                Item::Service(decl) => {
                    for route in &decl.routes {
                        collect_error_domain_owners(
                            *module_id,
                            &route.ret_type,
                            module_maps_by_id,
                            import_items_by_id,
                            symbols_by_id,
                            &mut owners,
                        );
                    }
                }
                _ => {}
            }
        }
        if owners.len() <= 1 {
            continue;
        }
        let mut owner_labels: Vec<String> = owners
            .keys()
            .map(|owner_id| module_label(registry, *owner_id))
            .collect();
        owner_labels.sort();
        owner_labels.dedup();
        let span = owners
            .values()
            .find_map(|spans| spans.first().copied())
            .unwrap_or_default();
        diags.error(
            span,
            format!(
                "strict architecture: error domain isolation violation in {}: boundary signatures mix domains from {}",
                unit.path.display(),
                owner_labels.join(", ")
            ),
        );
    }
}

fn collect_error_domain_owners(
    module_id: ModuleId,
    ret_ty: &TypeRef,
    module_maps_by_id: &HashMap<ModuleId, crate::loader::ModuleMap>,
    import_items_by_id: &HashMap<ModuleId, HashMap<String, crate::loader::ModuleLink>>,
    symbols_by_id: &HashMap<ModuleId, symbols::ModuleSymbols>,
    out: &mut HashMap<ModuleId, Vec<Span>>,
) {
    let mut domains: Vec<&TypeRef> = Vec::new();
    collect_result_error_types(ret_ty, &mut domains);
    for domain in domains {
        let Some(owner) = resolve_error_domain_owner(
            module_id,
            domain,
            module_maps_by_id,
            import_items_by_id,
            symbols_by_id,
        ) else {
            continue;
        };
        out.entry(owner).or_default().push(domain.span);
    }
}

fn collect_result_error_types<'a>(ret_ty: &'a TypeRef, out: &mut Vec<&'a TypeRef>) {
    let mut current = ret_ty;
    loop {
        let TypeRefKind::Result { err, .. } = &current.kind else {
            break;
        };
        let Some(err_ty) = err.as_deref() else {
            break;
        };
        match &err_ty.kind {
            TypeRefKind::Result { ok, .. } => {
                out.push(ok.as_ref());
                current = err_ty;
            }
            _ => {
                out.push(err_ty);
                break;
            }
        }
    }
}

fn resolve_error_domain_owner(
    module_id: ModuleId,
    domain_ty: &TypeRef,
    module_maps_by_id: &HashMap<ModuleId, crate::loader::ModuleMap>,
    import_items_by_id: &HashMap<ModuleId, HashMap<String, crate::loader::ModuleLink>>,
    symbols_by_id: &HashMap<ModuleId, symbols::ModuleSymbols>,
) -> Option<ModuleId> {
    let TypeRefKind::Simple(name) = &domain_ty.kind else {
        return None;
    };
    resolve_named_type_owner(
        module_id,
        &name.name,
        module_maps_by_id,
        import_items_by_id,
        symbols_by_id,
    )
}

fn resolve_named_type_owner(
    module_id: ModuleId,
    name: &str,
    module_maps_by_id: &HashMap<ModuleId, crate::loader::ModuleMap>,
    import_items_by_id: &HashMap<ModuleId, HashMap<String, crate::loader::ModuleLink>>,
    symbols_by_id: &HashMap<ModuleId, symbols::ModuleSymbols>,
) -> Option<ModuleId> {
    if let Some((module_name, item_name)) = split_qualified_name(name) {
        let link = module_maps_by_id.get(&module_id)?.get(module_name)?;
        let symbols = symbols_by_id.get(&link.id)?;
        if symbols.types.contains_key(item_name) || symbols.enums.contains_key(item_name) {
            return Some(link.id);
        }
        return None;
    }
    let symbols = symbols_by_id.get(&module_id)?;
    if symbols.types.contains_key(name) || symbols.enums.contains_key(name) {
        return Some(module_id);
    }
    let link = import_items_by_id.get(&module_id)?.get(name)?;
    let imported_symbols = symbols_by_id.get(&link.id)?;
    if imported_symbols.types.contains_key(name) || imported_symbols.enums.contains_key(name) {
        return Some(link.id);
    }
    None
}

fn find_layer_cycle(
    edges: &HashMap<String, HashSet<String>>,
    layers: &HashSet<String>,
) -> Option<Vec<String>> {
    let mut nodes: Vec<String> = layers.iter().cloned().collect();
    nodes.sort();
    let mut state: HashMap<String, u8> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    for node in &nodes {
        if state.get(node).copied().unwrap_or(0) != 0 {
            continue;
        }
        if let Some(cycle) = dfs_layer_cycle(node, edges, &mut state, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

fn dfs_layer_cycle(
    node: &str,
    edges: &HashMap<String, HashSet<String>>,
    state: &mut HashMap<String, u8>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    state.insert(node.to_string(), 1);
    stack.push(node.to_string());
    let mut next_nodes: Vec<String> = edges
        .get(node)
        .map(|targets| targets.iter().cloned().collect())
        .unwrap_or_default();
    next_nodes.sort();
    for next in next_nodes {
        match state.get(&next).copied().unwrap_or(0) {
            0 => {
                if let Some(cycle) = dfs_layer_cycle(&next, edges, state, stack) {
                    return Some(cycle);
                }
            }
            1 => {
                if let Some(idx) = stack.iter().position(|name| name == &next) {
                    let mut cycle = stack[idx..].to_vec();
                    cycle.push(next);
                    return Some(cycle);
                }
            }
            _ => {}
        }
    }
    stack.pop();
    state.insert(node.to_string(), 2);
    None
}

fn imported_module_ids(unit: &crate::loader::ModuleUnit) -> HashSet<ModuleId> {
    let mut out = HashSet::new();
    for link in unit.modules.modules.values() {
        out.insert(link.id);
    }
    for link in unit.import_items.values() {
        out.insert(link.id);
    }
    out
}

fn module_layer_name(path: &Path) -> String {
    let parts: Vec<String> = path
        .components()
        .filter_map(|part| part.as_os_str().to_str().map(|s| s.to_string()))
        .collect();
    if let Some(src_idx) = parts.iter().rposition(|part| part == "src") {
        if src_idx + 1 < parts.len() {
            let first = parts[src_idx + 1].clone();
            if src_idx + 1 == parts.len() - 1 {
                if let Some(stem) = Path::new(&first).file_stem().and_then(|stem| stem.to_str()) {
                    return stem.to_string();
                }
            }
            return first;
        }
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn module_label(registry: &ModuleRegistry, module_id: ModuleId) -> String {
    registry
        .get(module_id)
        .map(|unit| unit.path.display().to_string())
        .unwrap_or_else(|| format!("module{module_id}"))
}

fn require_span_for_capability(
    unit: &crate::loader::ModuleUnit,
    capability: Capability,
) -> Option<Span> {
    unit.program
        .requires
        .iter()
        .find(|require| require.capability == capability)
        .map(|require| require.span)
}

fn split_qualified_name(name: &str) -> Option<(&str, &str)> {
    let mut parts = name.split('.');
    let module = parts.next()?;
    let item = parts.next()?;
    if module.is_empty() || item.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((module, item))
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

fn collect_declared_capabilities(
    program: &Program,
    diags: &mut Diagnostics,
) -> std::collections::HashSet<Capability> {
    let mut out = std::collections::HashSet::new();
    let mut seen: HashMap<Capability, Span> = HashMap::new();
    for require in &program.requires {
        if let Some(prev_span) = seen.get(&require.capability).copied() {
            let name = require.capability.as_str();
            diags.error(
                require.span,
                format!("duplicate requires declaration for {name}"),
            );
            diags.error(prev_span, format!("previous requires {name} here"));
            continue;
        }
        seen.insert(require.capability, require.span);
        out.insert(require.capability);
    }
    out
}
