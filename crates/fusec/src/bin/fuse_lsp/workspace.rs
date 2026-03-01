use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use fuse_rt::json::JsonValue;
use fusec::ast::{ImportSpec, Item, Program};
use fusec::diag::{Diag, Level};
use fusec::loader::{
    ModuleExports, ModuleLink, ModuleMap, ModuleRegistry,
    load_program_with_modules_and_deps_and_overrides,
};
use fusec::parse_source;
use fusec::span::Span;

use super::super::{
    Index, IndexBuilder, LspState, STD_ERROR_MODULE_SOURCE, SymbolDef, SymbolKind,
    collect_qualified_refs, line_col_to_offset, line_offsets, location_json, offset_to_line_col,
    path_to_uri, range_json, span_contains, span_range_json, uri_to_path,
};

pub(crate) struct WorkspaceCache {
    pub(crate) docs_revision: u64,
    pub(crate) workspace_key: String,
    pub(crate) snapshot: WorkspaceSnapshot,
}

pub(crate) struct WorkspaceSnapshot {
    pub(crate) registry: ModuleRegistry,
    pub(crate) overrides: HashMap<PathBuf, String>,
    pub(crate) workspace_root: PathBuf,
    pub(crate) dep_roots: HashMap<String, PathBuf>,
    pub(crate) loader_diags: Vec<Diag>,
    pub(crate) module_ids_by_path: HashMap<PathBuf, usize>,
    pub(crate) index: Option<WorkspaceIndex>,
}

pub(crate) fn try_incremental_module_update(
    state: &mut LspState,
    uri: &str,
    text: Option<&str>,
) -> bool {
    let Some(cache) = state.workspace_cache.as_mut() else {
        return true;
    };
    cache.docs_revision = state.docs_revision;
    let Some(path) = uri_to_path(uri) else {
        return true;
    };
    let module_key = normalized_path(path.as_path());
    if let Some(contents) = text {
        cache
            .snapshot
            .overrides
            .insert(module_key.clone(), contents.to_string());
    } else {
        cache.snapshot.overrides.remove(&module_key);
    }
    let Some(module_id) = cache.snapshot.module_ids_by_path.get(&module_key).copied() else {
        return true;
    };
    let next_source = if let Some(contents) = text {
        contents.to_string()
    } else {
        match std::fs::read_to_string(&module_key) {
            Ok(contents) => contents,
            Err(_) => return false,
        }
    };
    let (next_program, mut parse_diags) = parse_source(&next_source);
    for diag in &mut parse_diags {
        diag.path = Some(module_key.clone());
    }
    let Some(unit) = cache.snapshot.registry.modules.get(&module_id) else {
        return false;
    };
    let prev_program = unit.program.clone();
    let import_changed =
        module_import_signature(&prev_program) != module_import_signature(&next_program);
    let export_changed =
        module_export_signature(&prev_program) != module_export_signature(&next_program);
    if has_unexpanded_type_derives(&prev_program) || has_unexpanded_type_derives(&next_program) {
        return false;
    }

    if import_changed || export_changed {
        if !try_partial_relink_for_structural_change(
            &mut cache.snapshot,
            module_id,
            &module_key,
            next_program,
            parse_diags,
            export_changed,
        ) {
            return false;
        }
        cache.snapshot.index = None;
        return true;
    }

    let Some(unit) = cache.snapshot.registry.modules.get_mut(&module_id) else {
        return false;
    };
    unit.program = next_program;
    unit.exports = module_exports_from_program(&unit.program);
    fusec::frontend::canonicalize::canonicalize_registry(&mut cache.snapshot.registry);

    let mut module_diags = HashMap::new();
    module_diags.insert(module_key, parse_diags);
    replace_loader_diags_for_modules(&mut cache.snapshot.loader_diags, &module_diags);
    refresh_global_duplicate_symbol_diags(
        &mut cache.snapshot.loader_diags,
        &cache.snapshot.registry,
    );

    cache.snapshot.index = None;
    true
}

fn try_partial_relink_for_structural_change(
    snapshot: &mut WorkspaceSnapshot,
    changed_module_id: usize,
    changed_module_path: &Path,
    changed_program: Program,
    changed_parse_diags: Vec<Diag>,
    export_changed: bool,
) -> bool {
    let mut path_to_id = snapshot.module_ids_by_path.clone();
    let mut affected = collect_modules_for_structural_relink(
        &snapshot.registry,
        &snapshot.workspace_root,
        &snapshot.dep_roots,
        &path_to_id,
        changed_module_id,
        export_changed,
    );
    affected.insert(changed_module_id);

    let mut affected_sorted: Vec<usize> = affected.iter().copied().collect();
    affected_sorted.sort_unstable();

    let mut staged_programs: HashMap<usize, Program> = HashMap::new();
    let mut next_module_diags: HashMap<PathBuf, Vec<Diag>> = HashMap::new();
    next_module_diags.insert(changed_module_path.to_path_buf(), changed_parse_diags);

    for module_id in &affected_sorted {
        if *module_id == changed_module_id {
            if has_unexpanded_type_derives(&changed_program) {
                return false;
            }
            staged_programs.insert(*module_id, changed_program.clone());
            continue;
        }
        let Some(unit) = snapshot.registry.modules.get(module_id) else {
            return false;
        };
        if unit.path.to_string_lossy().starts_with('<') {
            staged_programs.insert(*module_id, unit.program.clone());
            continue;
        }
        let module_path = normalized_path(unit.path.as_path());
        let source = if let Some(override_text) = snapshot.overrides.get(&module_path) {
            override_text.clone()
        } else {
            match std::fs::read_to_string(&module_path) {
                Ok(text) => text,
                Err(_) => return false,
            }
        };
        let (program, mut parse_diags) = parse_source(&source);
        if has_unexpanded_type_derives(&program) {
            return false;
        }
        for diag in &mut parse_diags {
            diag.path = Some(module_path.clone());
        }
        next_module_diags.insert(module_path, parse_diags);
        staged_programs.insert(*module_id, program);
    }

    for module_id in &affected_sorted {
        let Some(program) = staged_programs.remove(module_id) else {
            return false;
        };
        let Some(unit) = snapshot.registry.modules.get_mut(module_id) else {
            return false;
        };
        unit.program = program;
        unit.exports = module_exports_from_program(&unit.program);
    }

    let mut pending_links: HashMap<usize, (ModuleMap, HashMap<String, ModuleLink>)> =
        HashMap::new();
    let mut relink_queue = affected_sorted.clone();
    let mut queued: HashSet<usize> = relink_queue.iter().copied().collect();
    while let Some(module_id) = relink_queue.pop() {
        let Some((module_map, import_items, relink_diags, newly_loaded_ids)) =
            build_module_links_for_registry(
                snapshot,
                module_id,
                &mut path_to_id,
                &mut next_module_diags,
            )
        else {
            return false;
        };
        let Some(path) = snapshot
            .registry
            .modules
            .get(&module_id)
            .map(|unit| unit.path.clone())
        else {
            return false;
        };
        let module_path = normalized_path(path.as_path());
        next_module_diags
            .entry(module_path)
            .or_default()
            .extend(relink_diags);
        pending_links.insert(module_id, (module_map, import_items));

        for new_id in newly_loaded_ids {
            if queued.insert(new_id) {
                relink_queue.push(new_id);
            }
        }
    }

    for (module_id, (module_map, import_items)) in pending_links {
        let Some(unit) = snapshot.registry.modules.get_mut(&module_id) else {
            return false;
        };
        unit.modules = module_map;
        unit.import_items = import_items;
    }

    snapshot.module_ids_by_path = path_to_id;

    if has_import_cycle(&snapshot.registry) {
        return false;
    }

    fusec::frontend::canonicalize::canonicalize_registry(&mut snapshot.registry);
    replace_loader_diags_for_modules(&mut snapshot.loader_diags, &next_module_diags);
    refresh_global_duplicate_symbol_diags(&mut snapshot.loader_diags, &snapshot.registry);
    true
}

fn collect_modules_for_structural_relink(
    registry: &ModuleRegistry,
    workspace_root: &Path,
    dep_roots: &HashMap<String, PathBuf>,
    path_to_id: &HashMap<PathBuf, usize>,
    changed_module_id: usize,
    export_changed: bool,
) -> HashSet<usize> {
    let mut affected = HashSet::new();
    affected.insert(changed_module_id);
    if !export_changed {
        return affected;
    }
    let reverse = build_reverse_import_graph(registry, workspace_root, dep_roots, path_to_id);
    let mut stack = vec![changed_module_id];
    while let Some(target) = stack.pop() {
        let Some(dependents) = reverse.get(&target) else {
            continue;
        };
        for dependent in dependents {
            if affected.insert(*dependent) {
                stack.push(*dependent);
            }
        }
    }
    affected
}

fn build_reverse_import_graph(
    registry: &ModuleRegistry,
    workspace_root: &Path,
    dep_roots: &HashMap<String, PathBuf>,
    path_to_id: &HashMap<PathBuf, usize>,
) -> HashMap<usize, HashSet<usize>> {
    let mut reverse: HashMap<usize, HashSet<usize>> = HashMap::new();
    let mut ids: Vec<usize> = registry.modules.keys().copied().collect();
    ids.sort_unstable();
    for module_id in ids {
        let Some(unit) = registry.modules.get(&module_id) else {
            continue;
        };
        let base_dir = unit
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        for target in declared_import_targets_for_program(
            &unit.program,
            &base_dir,
            workspace_root,
            dep_roots,
            path_to_id,
        ) {
            reverse.entry(target).or_default().insert(module_id);
        }
    }
    reverse
}

fn declared_import_targets_for_program(
    program: &Program,
    base_dir: &Path,
    workspace_root: &Path,
    dep_roots: &HashMap<String, PathBuf>,
    path_to_id: &HashMap<PathBuf, usize>,
) -> HashSet<usize> {
    let mut out = HashSet::new();
    for item in &program.items {
        let Item::Import(decl) = item else {
            continue;
        };
        if let Some(target) =
            import_target_for_spec(base_dir, workspace_root, dep_roots, &decl.spec, path_to_id)
        {
            out.insert(target);
        }
    }
    out
}

fn import_target_for_spec(
    base_dir: &Path,
    workspace_root: &Path,
    dep_roots: &HashMap<String, PathBuf>,
    spec: &ImportSpec,
    path_to_id: &HashMap<PathBuf, usize>,
) -> Option<usize> {
    match spec {
        ImportSpec::Module { name } => {
            let path = resolve_module_name_import_path(base_dir, &name.name);
            path_to_id.get(&path).copied()
        }
        ImportSpec::ModuleFrom { path, .. }
        | ImportSpec::NamedFrom { path, .. }
        | ImportSpec::AliasFrom { path, .. } => {
            if let Some(path) = resolve_dep_import_path(dep_roots, &path.value) {
                return path_to_id.get(&path).copied();
            }
            resolve_import_path_value(base_dir, workspace_root, &path.value)
                .and_then(|path| path_to_id.get(&path).copied())
        }
    }
}

fn resolve_module_name_import_path(base_dir: &Path, name: &str) -> PathBuf {
    let mut path = base_dir.join(name);
    if path.extension().is_none() {
        path.set_extension("fuse");
    }
    normalized_path(path.as_path())
}

fn resolve_import_path_value(base_dir: &Path, workspace_root: &Path, raw: &str) -> Option<PathBuf> {
    if raw == "std.Error" {
        return Some(PathBuf::from("<std.Error>"));
    }
    if raw.starts_with("dep:") {
        return None;
    }
    if raw.starts_with("root:") {
        let rel = parse_root_import(raw)?;
        return resolve_root_import_path(workspace_root, rel);
    }
    let mut path = PathBuf::from(raw);
    if path.extension().is_none() {
        path.set_extension("fuse");
    }
    if path.is_relative() {
        path = base_dir.join(path);
    }
    Some(normalized_path(path.as_path()))
}

fn parse_dep_import(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix("dep:")?;
    let (dep, rel) = rest.split_once('/')?;
    if dep.is_empty() || rel.is_empty() {
        return None;
    }
    Some((dep, rel))
}

fn parse_root_import(raw: &str) -> Option<&str> {
    let rel = raw.strip_prefix("root:")?;
    if rel.is_empty() {
        return None;
    }
    Some(rel)
}

fn resolve_dep_import_path(dep_roots: &HashMap<String, PathBuf>, raw: &str) -> Option<PathBuf> {
    let (dep, rel) = parse_dep_import(raw)?;
    let dep_root = dep_roots.get(dep)?;
    let mut path = dep_root.join(rel);
    if path.extension().is_none() {
        path.set_extension("fuse");
    }
    Some(normalized_path(path.as_path()))
}

fn resolve_root_import_path(workspace_root: &Path, raw: &str) -> Option<PathBuf> {
    let rel = Path::new(raw);
    if rel.is_absolute() {
        return None;
    }
    let mut normalized_rel = PathBuf::new();
    for comp in rel.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(seg) => normalized_rel.push(seg),
            std::path::Component::ParentDir => {
                if !normalized_rel.pop() {
                    return None;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
        }
    }
    if normalized_rel.as_os_str().is_empty() {
        return None;
    }
    if normalized_rel.extension().is_none() {
        normalized_rel.set_extension("fuse");
    }
    Some(normalized_path(
        workspace_root.join(normalized_rel).as_path(),
    ))
}

fn build_module_links_for_registry(
    snapshot: &mut WorkspaceSnapshot,
    module_id: usize,
    path_to_id: &mut HashMap<PathBuf, usize>,
    next_module_diags: &mut HashMap<PathBuf, Vec<Diag>>,
) -> Option<(
    ModuleMap,
    HashMap<String, ModuleLink>,
    Vec<Diag>,
    Vec<usize>,
)> {
    let (unit_path, unit_program) = {
        let unit = snapshot.registry.modules.get(&module_id)?;
        (unit.path.clone(), unit.program.clone())
    };
    let base_dir = unit_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut module_map = ModuleMap::default();
    let mut import_items = HashMap::new();
    let mut import_item_spans: HashMap<String, Span> = HashMap::new();
    let mut diags = Vec::new();
    let mut newly_loaded_ids = Vec::new();

    for item in &unit_program.items {
        let Item::Import(decl) = item else {
            continue;
        };
        match &decl.spec {
            ImportSpec::Module { name } => {
                let target = resolve_module_name_import_path(&base_dir, &name.name);
                let Ok((target_id, loaded_now)) = ensure_module_loaded_for_relink(
                    snapshot,
                    path_to_id,
                    &target,
                    next_module_diags,
                ) else {
                    return None;
                };
                if loaded_now {
                    newly_loaded_ids.push(target_id);
                }
                module_map
                    .modules
                    .entry(name.name.clone())
                    .or_insert_with(|| module_link_for_registry(&snapshot.registry, target_id));
            }
            ImportSpec::ModuleFrom { name, path } => {
                let target_id = match resolve_import_target_for_relink(
                    &base_dir,
                    &path.value,
                    path.span,
                    snapshot,
                    path_to_id,
                    &mut diags,
                    next_module_diags,
                ) {
                    Ok(Some((target, loaded_now))) => {
                        if loaded_now {
                            newly_loaded_ids.push(target);
                        }
                        target
                    }
                    Ok(None) => continue,
                    Err(()) => return None,
                };
                module_map
                    .modules
                    .entry(name.name.clone())
                    .or_insert_with(|| module_link_for_registry(&snapshot.registry, target_id));
            }
            ImportSpec::AliasFrom { alias, path, .. } => {
                let target_id = match resolve_import_target_for_relink(
                    &base_dir,
                    &path.value,
                    path.span,
                    snapshot,
                    path_to_id,
                    &mut diags,
                    next_module_diags,
                ) {
                    Ok(Some((target, loaded_now))) => {
                        if loaded_now {
                            newly_loaded_ids.push(target);
                        }
                        target
                    }
                    Ok(None) => continue,
                    Err(()) => return None,
                };
                module_map
                    .modules
                    .entry(alias.name.clone())
                    .or_insert_with(|| module_link_for_registry(&snapshot.registry, target_id));
            }
            ImportSpec::NamedFrom { names, path } => {
                let target_id = match resolve_import_target_for_relink(
                    &base_dir,
                    &path.value,
                    path.span,
                    snapshot,
                    path_to_id,
                    &mut diags,
                    next_module_diags,
                ) {
                    Ok(Some((target, loaded_now))) => {
                        if loaded_now {
                            newly_loaded_ids.push(target);
                        }
                        target
                    }
                    Ok(None) => continue,
                    Err(()) => return None,
                };
                let Some(target_exports) = snapshot
                    .registry
                    .modules
                    .get(&target_id)
                    .map(|unit| &unit.exports)
                else {
                    return None;
                };
                for name in names {
                    if let Some(prev_span) = import_item_spans.get(&name.name).copied() {
                        diags.push(Diag {
                            level: Level::Error,
                            message: format!("duplicate import {}", name.name),
                            span: name.span,
                            path: Some(unit_path.clone()),
                        });
                        diags.push(Diag {
                            level: Level::Error,
                            message: format!("previous import of {} here", name.name),
                            span: prev_span,
                            path: Some(unit_path.clone()),
                        });
                        continue;
                    }
                    if !module_exports_contains(target_exports, &name.name) {
                        diags.push(Diag {
                            level: Level::Error,
                            message: format!("unknown import {} in {}", name.name, path.value),
                            span: name.span,
                            path: Some(unit_path.clone()),
                        });
                        continue;
                    }
                    import_items.insert(
                        name.name.clone(),
                        module_link_for_registry(&snapshot.registry, target_id),
                    );
                    import_item_spans.insert(name.name.clone(), name.span);
                }
            }
        }
    }

    newly_loaded_ids.sort_unstable();
    newly_loaded_ids.dedup();
    Some((module_map, import_items, diags, newly_loaded_ids))
}

fn resolve_import_target_for_relink(
    base_dir: &Path,
    raw: &str,
    span: Span,
    snapshot: &mut WorkspaceSnapshot,
    path_to_id: &mut HashMap<PathBuf, usize>,
    diags: &mut Vec<Diag>,
    next_module_diags: &mut HashMap<PathBuf, Vec<Diag>>,
) -> Result<Option<(usize, bool)>, ()> {
    if raw.starts_with("dep:") {
        let Some((dep, rel)) = parse_dep_import(raw) else {
            diags.push(Diag {
                level: Level::Error,
                message: "dependency imports require dep:<name>/<path>".to_string(),
                span,
                path: None,
            });
            return Ok(None);
        };
        let Some(dep_root) = snapshot.dep_roots.get(dep) else {
            diags.push(Diag {
                level: Level::Error,
                message: format!("unknown dependency {dep}"),
                span,
                path: None,
            });
            return Ok(None);
        };
        let mut dep_path = dep_root.join(rel);
        if dep_path.extension().is_none() {
            dep_path.set_extension("fuse");
        }
        let (id, loaded_now) =
            ensure_module_loaded_for_relink(snapshot, path_to_id, &dep_path, next_module_diags)?;
        return Ok(Some((id, loaded_now)));
    }
    let path = if raw.starts_with("root:") {
        let Some(rel) = parse_root_import(raw) else {
            diags.push(Diag {
                level: Level::Error,
                message: "root imports require root:<path>".to_string(),
                span,
                path: None,
            });
            return Ok(None);
        };
        let Some(path) = resolve_root_import_path(&snapshot.workspace_root, rel) else {
            diags.push(Diag {
                level: Level::Error,
                message: "root import path escapes workspace root".to_string(),
                span,
                path: None,
            });
            return Ok(None);
        };
        path
    } else {
        let Some(path) = resolve_import_path_value(base_dir, &snapshot.workspace_root, raw) else {
            return Ok(None);
        };
        path
    };
    let (id, loaded_now) =
        ensure_module_loaded_for_relink(snapshot, path_to_id, &path, next_module_diags)?;
    Ok(Some((id, loaded_now)))
}

fn ensure_module_loaded_for_relink(
    snapshot: &mut WorkspaceSnapshot,
    path_to_id: &mut HashMap<PathBuf, usize>,
    path: &Path,
    next_module_diags: &mut HashMap<PathBuf, Vec<Diag>>,
) -> Result<(usize, bool), ()> {
    let key = normalized_path(path);
    if let Some(id) = path_to_id.get(&key).copied() {
        return Ok((id, false));
    }
    if key.to_string_lossy() == "<std.Error>" {
        let (program, mut parse_diags) = parse_source(STD_ERROR_MODULE_SOURCE);
        if has_unexpanded_type_derives(&program) {
            return Err(());
        }
        for diag in &mut parse_diags {
            diag.path = Some(key.clone());
        }
        let next_id = snapshot
            .registry
            .modules
            .keys()
            .copied()
            .max()
            .map_or(1, |id| id.saturating_add(1));
        let unit = fusec::loader::ModuleUnit {
            id: next_id,
            path: key.clone(),
            program,
            modules: ModuleMap::default(),
            import_items: HashMap::new(),
            exports: ModuleExports::default(),
        };
        snapshot.registry.modules.insert(next_id, unit);
        let Some(unit) = snapshot.registry.modules.get_mut(&next_id) else {
            return Err(());
        };
        unit.exports = module_exports_from_program(&unit.program);
        path_to_id.insert(key.clone(), next_id);
        next_module_diags.insert(key, parse_diags);
        return Ok((next_id, true));
    }
    if key.to_string_lossy().starts_with('<') {
        return Err(());
    }
    let source = if let Some(override_text) = snapshot.overrides.get(&key) {
        override_text.clone()
    } else {
        std::fs::read_to_string(&key).map_err(|_| ())?
    };
    let (program, mut parse_diags) = parse_source(&source);
    if has_unexpanded_type_derives(&program) {
        return Err(());
    }
    for diag in &mut parse_diags {
        diag.path = Some(key.clone());
    }
    let next_id = snapshot
        .registry
        .modules
        .keys()
        .copied()
        .max()
        .map_or(1, |id| id.saturating_add(1));
    let unit = fusec::loader::ModuleUnit {
        id: next_id,
        path: key.clone(),
        program,
        modules: ModuleMap::default(),
        import_items: HashMap::new(),
        exports: ModuleExports::default(),
    };
    snapshot.registry.modules.insert(next_id, unit);
    let Some(unit) = snapshot.registry.modules.get_mut(&next_id) else {
        return Err(());
    };
    unit.exports = module_exports_from_program(&unit.program);
    path_to_id.insert(key.clone(), next_id);
    next_module_diags.insert(key, parse_diags);
    Ok((next_id, true))
}

fn replace_loader_diags_for_modules(
    loader_diags: &mut Vec<Diag>,
    next_by_module: &HashMap<PathBuf, Vec<Diag>>,
) {
    let module_paths: HashSet<PathBuf> = next_by_module.keys().cloned().collect();
    loader_diags.retain(|diag| match diag.path.as_ref() {
        Some(path) => !module_paths.contains(&normalized_path(path.as_path())),
        None => true,
    });
    for diags in next_by_module.values() {
        loader_diags.extend(diags.clone());
    }
}

fn refresh_global_duplicate_symbol_diags(loader_diags: &mut Vec<Diag>, registry: &ModuleRegistry) {
    loader_diags.retain(|diag| !is_global_duplicate_symbol_diag(diag));
    loader_diags.extend(collect_global_duplicate_symbol_diags(registry));
}

fn is_global_duplicate_symbol_diag(diag: &Diag) -> bool {
    diag.message.starts_with("duplicate symbol: ")
        || (diag.message.starts_with("previous definition of ") && diag.message.ends_with(" here"))
}

fn collect_global_duplicate_symbol_diags(registry: &ModuleRegistry) -> Vec<Diag> {
    let mut out = Vec::new();
    let mut ids: Vec<usize> = registry.modules.keys().copied().collect();
    ids.sort_unstable();
    let mut seen: HashMap<String, (usize, PathBuf, Span)> = HashMap::new();
    for module_id in ids {
        let Some(unit) = registry.modules.get(&module_id) else {
            continue;
        };
        if unit.path.to_string_lossy() == "<std.Error>" {
            continue;
        }
        for item in &unit.program.items {
            let symbol = match item {
                Item::Type(decl) => Some((decl.name.name.clone(), decl.name.span)),
                Item::Enum(decl) => Some((decl.name.name.clone(), decl.name.span)),
                Item::Config(decl) => Some((decl.name.name.clone(), decl.name.span)),
                Item::Service(decl) => Some((decl.name.name.clone(), decl.name.span)),
                Item::App(decl) => Some((decl.name.value.clone(), decl.name.span)),
                _ => None,
            };
            let Some((name, span)) = symbol else {
                continue;
            };
            if let Some((prev_id, prev_path, prev_span)) = seen.get(&name) {
                if *prev_id != module_id {
                    out.push(Diag {
                        level: Level::Error,
                        message: format!("duplicate symbol: {name}"),
                        span,
                        path: Some(unit.path.clone()),
                    });
                    out.push(Diag {
                        level: Level::Error,
                        message: format!("previous definition of {name} here"),
                        span: *prev_span,
                        path: Some(prev_path.clone()),
                    });
                }
                continue;
            }
            seen.insert(name, (module_id, unit.path.clone(), span));
        }
    }
    out
}

fn has_import_cycle(registry: &ModuleRegistry) -> bool {
    fn visit(
        node: usize,
        registry: &ModuleRegistry,
        active: &mut HashSet<usize>,
        done: &mut HashSet<usize>,
    ) -> bool {
        if done.contains(&node) {
            return false;
        }
        if !active.insert(node) {
            return true;
        }
        let Some(unit) = registry.modules.get(&node) else {
            active.remove(&node);
            done.insert(node);
            return false;
        };
        let mut deps = HashSet::new();
        for link in unit.modules.modules.values() {
            deps.insert(link.id);
        }
        for link in unit.import_items.values() {
            deps.insert(link.id);
        }
        for dep in deps {
            if visit(dep, registry, active, done) {
                return true;
            }
        }
        active.remove(&node);
        done.insert(node);
        false
    }

    let mut active = HashSet::new();
    let mut done = HashSet::new();
    for module_id in registry.modules.keys().copied() {
        if visit(module_id, registry, &mut active, &mut done) {
            return true;
        }
    }
    false
}

fn module_exports_from_program(program: &Program) -> ModuleExports {
    let mut exports = ModuleExports::default();
    for item in &program.items {
        match item {
            Item::Type(decl) => {
                exports.types.insert(decl.name.name.clone());
            }
            Item::Enum(decl) => {
                exports.enums.insert(decl.name.name.clone());
            }
            Item::Fn(decl) => {
                exports.functions.insert(decl.name.name.clone());
            }
            Item::Config(decl) => {
                exports.configs.insert(decl.name.name.clone());
            }
            Item::Service(decl) => {
                exports.services.insert(decl.name.name.clone());
            }
            Item::App(decl) => {
                exports.apps.insert(decl.name.value.clone());
            }
            _ => {}
        }
    }
    exports
}

fn module_exports_contains(exports: &ModuleExports, name: &str) -> bool {
    exports.types.contains(name)
        || exports.enums.contains(name)
        || exports.functions.contains(name)
        || exports.configs.contains(name)
        || exports.services.contains(name)
        || exports.apps.contains(name)
}

fn module_link_for_registry(registry: &ModuleRegistry, module_id: usize) -> ModuleLink {
    let exports = registry
        .modules
        .get(&module_id)
        .map(|unit| unit.exports.clone())
        .unwrap_or_default();
    ModuleLink {
        id: module_id,
        exports,
    }
}

fn has_unexpanded_type_derives(program: &Program) -> bool {
    program.items.iter().any(|item| match item {
        Item::Type(decl) => decl.derive.is_some(),
        _ => false,
    })
}

fn module_import_signature(program: &Program) -> Vec<String> {
    let mut imports = Vec::new();
    for item in &program.items {
        let Item::Import(decl) = item else {
            continue;
        };
        imports.push(import_spec_signature(&decl.spec));
    }
    imports.sort();
    imports
}

fn import_spec_signature(spec: &ImportSpec) -> String {
    match spec {
        ImportSpec::Module { name } => format!("module:{}", name.name),
        ImportSpec::ModuleFrom { name, path } => {
            format!("module_from:{}@{}", name.name, path.value)
        }
        ImportSpec::NamedFrom { names, path } => {
            let mut symbols: Vec<String> = names.iter().map(|name| name.name.clone()).collect();
            symbols.sort();
            format!("named_from:{}@{}", symbols.join(","), path.value)
        }
        ImportSpec::AliasFrom { name, alias, path } => {
            format!("alias_from:{}:{}@{}", name.name, alias.name, path.value)
        }
    }
}

fn module_export_signature(program: &Program) -> Vec<String> {
    let mut exports = Vec::new();
    for item in &program.items {
        match item {
            Item::Type(decl) => exports.push(format!("type:{}", decl.name.name)),
            Item::Enum(decl) => exports.push(format!("enum:{}", decl.name.name)),
            Item::Fn(decl) => exports.push(format!("fn:{}", decl.name.name)),
            Item::Config(decl) => exports.push(format!("config:{}", decl.name.name)),
            Item::Service(decl) => exports.push(format!("service:{}", decl.name.name)),
            Item::App(decl) => exports.push(format!("app:{}", decl.name.value)),
            _ => {}
        }
    }
    exports.sort();
    exports
}

fn normalized_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn workspace_stats_result(state: &LspState) -> JsonValue {
    let mut out = BTreeMap::new();
    out.insert(
        "docsRevision".to_string(),
        JsonValue::Number(state.docs_revision as f64),
    );
    out.insert(
        "workspaceBuilds".to_string(),
        JsonValue::Number(state.workspace_builds as f64),
    );
    out.insert(
        "cachePresent".to_string(),
        JsonValue::Bool(state.workspace_cache.is_some()),
    );
    JsonValue::Object(out)
}

pub(crate) struct WorkspaceIndex {
    pub(crate) files: Vec<WorkspaceFile>,
    pub(crate) file_by_uri: HashMap<String, usize>,
    pub(crate) defs: Vec<WorkspaceDef>,
    pub(crate) refs: Vec<WorkspaceRef>,
    pub(crate) calls: Vec<WorkspaceCall>,
    pub(crate) module_alias_exports: HashMap<String, HashMap<String, HashSet<String>>>,
    pub(crate) redirects: HashMap<usize, usize>,
}

pub(crate) struct WorkspaceFile {
    pub(crate) uri: String,
    pub(crate) text: String,
    pub(crate) index: Index,
    pub(crate) def_map: Vec<usize>,
    pub(crate) qualified_refs: Vec<QualifiedRef>,
}

#[derive(Clone)]
pub(crate) struct WorkspaceDef {
    pub(crate) id: usize,
    pub(crate) uri: String,
    pub(crate) def: SymbolDef,
}

pub(crate) struct WorkspaceRef {
    pub(crate) uri: String,
    pub(crate) span: Span,
    pub(crate) target: usize,
}

#[derive(Clone)]
pub(crate) struct WorkspaceCall {
    pub(crate) uri: String,
    pub(crate) span: Span,
    pub(crate) from: usize,
    pub(crate) to: usize,
}

pub(crate) struct QualifiedRef {
    pub(crate) span: Span,
    pub(crate) target: usize,
}

impl WorkspaceIndex {
    pub(crate) fn definition_at(
        &self,
        uri: &str,
        line: usize,
        character: usize,
    ) -> Option<WorkspaceDef> {
        let file_idx = *self.file_by_uri.get(uri)?;
        let file = &self.files[file_idx];
        let offsets = line_offsets(&file.text);
        let offset = line_col_to_offset(&file.text, &offsets, line, character);
        if let Some(target) = best_ref_target(&file.qualified_refs, offset) {
            let def = self.def_for_target(target)?;
            return Some(def);
        }
        let local_def_id = file.index.definition_at(offset)?;
        let mut def_id = *file.def_map.get(local_def_id)?;
        while let Some(next) = self.redirects.get(&def_id) {
            if *next == def_id {
                break;
            }
            def_id = *next;
        }
        let def = self.def_for_target(def_id)?;
        Some(def)
    }

    pub(crate) fn rename_edits(
        &self,
        def_id: usize,
        new_name: &str,
    ) -> HashMap<String, Vec<JsonValue>> {
        let mut spans_by_uri: HashMap<String, Vec<Span>> = HashMap::new();
        if let Some(def) = self.def_for_target(def_id) {
            spans_by_uri
                .entry(def.uri.clone())
                .or_default()
                .push(def.def.span);
        }
        for reference in &self.refs {
            if reference.target == def_id {
                spans_by_uri
                    .entry(reference.uri.clone())
                    .or_default()
                    .push(reference.span);
            }
        }
        let mut edits_by_uri = HashMap::new();
        for (uri, spans) in spans_by_uri {
            let Some(text) = self.file_text(&uri) else {
                continue;
            };
            let offsets = line_offsets(text);
            let mut edits = Vec::new();
            let mut seen = HashSet::new();
            for span in spans {
                if !seen.insert((span.start, span.end)) {
                    continue;
                }
                let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
                let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
                let range = range_json(start_line, start_col, end_line, end_col);
                let mut edit = BTreeMap::new();
                edit.insert("range".to_string(), range);
                edit.insert(
                    "newText".to_string(),
                    JsonValue::String(new_name.to_string()),
                );
                edits.push(JsonValue::Object(edit));
            }
            if !edits.is_empty() {
                edits_by_uri.insert(uri, edits);
            }
        }
        edits_by_uri
    }

    pub(crate) fn reference_locations(
        &self,
        def_id: usize,
        include_declaration: bool,
    ) -> Vec<JsonValue> {
        let related_targets = self.related_targets_for_def(def_id);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if include_declaration {
            if let Some(def) = self.def_for_target(def_id) {
                if let Some(text) = self.file_text(&def.uri) {
                    let key = (def.uri.clone(), def.def.span.start, def.def.span.end);
                    if seen.insert(key) {
                        out.push(location_json(&def.uri, text, def.def.span));
                    }
                }
            }
        }
        for reference in &self.refs {
            if !related_targets.contains(&reference.target) {
                continue;
            }
            let key = (
                reference.uri.clone(),
                reference.span.start,
                reference.span.end,
            );
            if !seen.insert(key) {
                continue;
            }
            let Some(text) = self.file_text(&reference.uri) else {
                continue;
            };
            out.push(location_json(&reference.uri, text, reference.span));
        }
        for call in &self.calls {
            if !related_targets.contains(&call.to) {
                continue;
            }
            let key = (call.uri.clone(), call.span.start, call.span.end);
            if !seen.insert(key) {
                continue;
            }
            let Some(text) = self.file_text(&call.uri) else {
                continue;
            };
            out.push(location_json(&call.uri, text, call.span));
        }
        out
    }

    pub(crate) fn span_range_json(&self, uri: &str, span: Span) -> Option<JsonValue> {
        let text = self.file_text(uri)?;
        Some(span_range_json(text, span))
    }

    pub(crate) fn incoming_calls(&self, target: usize) -> Vec<(usize, Vec<WorkspaceCall>)> {
        let related_targets = self.related_targets_for_def(target);
        let mut grouped: HashMap<usize, Vec<WorkspaceCall>> = HashMap::new();
        for call in &self.calls {
            if !related_targets.contains(&call.to) {
                continue;
            }
            grouped.entry(call.from).or_default().push(call.clone());
        }
        let mut out: Vec<(usize, Vec<WorkspaceCall>)> = grouped.into_iter().collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    pub(crate) fn outgoing_calls(&self, source: usize) -> Vec<(usize, Vec<WorkspaceCall>)> {
        let mut grouped: HashMap<usize, Vec<WorkspaceCall>> = HashMap::new();
        for call in &self.calls {
            if call.from != source {
                continue;
            }
            grouped.entry(call.to).or_default().push(call.clone());
        }
        let mut out: Vec<(usize, Vec<WorkspaceCall>)> = grouped.into_iter().collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    pub(crate) fn alias_modules_for_symbol(&self, uri: &str, symbol: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(aliases) = self.module_alias_exports.get(uri) else {
            return out;
        };
        for (alias, exports) in aliases {
            if exports.contains(symbol) {
                out.push(alias.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub(crate) fn alias_exports_for_module(&self, uri: &str, alias: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(aliases) = self.module_alias_exports.get(uri) else {
            return out;
        };
        let Some(exports) = aliases.get(alias) else {
            return out;
        };
        out.extend(exports.iter().cloned());
        out.sort();
        out.dedup();
        out
    }

    pub(crate) fn def_for_target(&self, target: usize) -> Option<WorkspaceDef> {
        self.defs.get(target).cloned()
    }

    pub(crate) fn file_text(&self, uri: &str) -> Option<&str> {
        let idx = *self.file_by_uri.get(uri)?;
        Some(self.files[idx].text.as_str())
    }

    fn resolve_redirect_target(&self, mut target: usize) -> usize {
        while let Some(next) = self.redirects.get(&target) {
            if *next == target {
                break;
            }
            target = *next;
        }
        target
    }

    fn related_targets_for_def(&self, def_id: usize) -> HashSet<usize> {
        let mut related = HashSet::new();
        related.insert(def_id);
        let Some(def) = self.def_for_target(def_id) else {
            return related;
        };

        for (from, to) in &self.redirects {
            if self.resolve_redirect_target(*to) == def_id {
                related.insert(*from);
            }
        }

        if matches!(
            def.def.kind,
            SymbolKind::Function
                | SymbolKind::Type
                | SymbolKind::Enum
                | SymbolKind::Config
                | SymbolKind::Service
                | SymbolKind::App
                | SymbolKind::Migration
                | SymbolKind::Test
        ) {
            let import_detail = format!("import {}", def.def.name);
            for cand in &self.defs {
                if cand.def.kind == SymbolKind::Variable && cand.def.detail == import_detail {
                    related.insert(cand.id);
                }
            }
        }
        related
    }
}

fn best_ref_target(refs: &[QualifiedRef], offset: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for reference in refs {
        if span_contains(reference.span, offset) {
            let size = reference.span.end.saturating_sub(reference.span.start);
            if best.map_or(true, |(_, best_size)| size <= best_size) {
                best = Some((reference.target, size));
            }
        }
    }
    best.map(|(target, _)| target)
}

pub(crate) fn build_index_with_program(text: &str, program: &Program) -> Index {
    let mut builder = IndexBuilder::new(text);
    builder.collect(program);
    builder.finish()
}

fn workspace_index_key(state: &LspState, focus_uri: &str) -> Option<String> {
    let focus_path = if !focus_uri.is_empty() {
        uri_to_path(focus_uri)
    } else {
        None
    };
    let root_path = state
        .root_uri
        .as_deref()
        .and_then(uri_to_path)
        .or_else(|| focus_path.clone())?;
    let (_workspace_root, entry_path) =
        resolve_workspace_context(&root_path, focus_path.as_deref())?;
    let entry_key = entry_path.canonicalize().unwrap_or(entry_path);
    Some(entry_key.to_string_lossy().to_string())
}

pub(crate) fn build_workspace_snapshot_cached<'a>(
    state: &'a mut LspState,
    focus_uri: &str,
) -> Option<&'a mut WorkspaceSnapshot> {
    let workspace_key = workspace_index_key(state, focus_uri)?;
    let cache_hit = state.workspace_cache.as_ref().is_some_and(|cache| {
        cache.docs_revision == state.docs_revision && cache.workspace_key == workspace_key
    });
    if !cache_hit {
        let snapshot = build_workspace_snapshot(state, focus_uri)?;
        state.workspace_cache = Some(WorkspaceCache {
            docs_revision: state.docs_revision,
            workspace_key,
            snapshot,
        });
        state.workspace_builds = state.workspace_builds.saturating_add(1);
    }
    state
        .workspace_cache
        .as_mut()
        .map(|cache| &mut cache.snapshot)
}

pub(crate) fn build_workspace_index_cached<'a>(
    state: &'a mut LspState,
    focus_uri: &str,
) -> Option<&'a WorkspaceIndex> {
    let snapshot = build_workspace_snapshot_cached(state, focus_uri)?;
    if snapshot.index.is_none() {
        snapshot.index = build_workspace_from_registry(&snapshot.registry, &snapshot.overrides);
    }
    snapshot.index.as_ref()
}

pub(crate) fn build_workspace_snapshot(
    state: &LspState,
    focus_uri: &str,
) -> Option<WorkspaceSnapshot> {
    let focus_path = if !focus_uri.is_empty() {
        uri_to_path(focus_uri)
    } else {
        None
    };
    let root_path = state
        .root_uri
        .as_deref()
        .and_then(uri_to_path)
        .or_else(|| focus_path.clone())?;
    let (workspace_root, entry_path) =
        resolve_workspace_context(&root_path, focus_path.as_deref())?;
    let overrides = doc_overrides(state);
    build_workspace_snapshot_from_entry(entry_path, workspace_root, overrides)
}

pub(crate) fn build_focus_workspace_snapshot(
    state: &LspState,
    focus_uri: &str,
) -> Option<WorkspaceSnapshot> {
    let focus_path = uri_to_path(focus_uri)?;
    let root_path = state
        .root_uri
        .as_deref()
        .and_then(uri_to_path)
        .or_else(|| Some(focus_path.clone()))?;
    let (workspace_root, _entry_path) = resolve_workspace_context(&root_path, Some(&focus_path))?;
    let overrides = doc_overrides(state);
    build_workspace_snapshot_from_entry(focus_path, workspace_root, overrides)
}

fn build_workspace_snapshot_from_entry(
    entry_path: PathBuf,
    workspace_root: PathBuf,
    overrides: HashMap<PathBuf, String>,
) -> Option<WorkspaceSnapshot> {
    let dep_roots = resolve_dependency_roots_for_workspace(&workspace_root);
    let entry_key = entry_path
        .canonicalize()
        .unwrap_or_else(|_| entry_path.clone());
    let root_text = overrides
        .get(&entry_key)
        .cloned()
        .or_else(|| std::fs::read_to_string(&entry_path).ok())?;
    let (registry, loader_diags) = load_program_with_modules_and_deps_and_overrides(
        &entry_path,
        &root_text,
        &dep_roots,
        &overrides,
    );
    let mut module_ids_by_path = HashMap::new();
    for (id, unit) in &registry.modules {
        let key = unit
            .path
            .canonicalize()
            .unwrap_or_else(|_| unit.path.clone());
        module_ids_by_path.insert(key, *id);
    }
    Some(WorkspaceSnapshot {
        registry,
        overrides,
        workspace_root,
        dep_roots,
        loader_diags,
        module_ids_by_path,
        index: None,
    })
}

fn doc_overrides(state: &LspState) -> HashMap<PathBuf, String> {
    let mut overrides = HashMap::new();
    for (uri, text) in &state.docs {
        if let Some(path) = uri_to_path(uri) {
            let key = path.canonicalize().unwrap_or(path);
            overrides.insert(key, text.clone());
        }
    }
    overrides
}

fn resolve_workspace_context(root_path: &Path, focus: Option<&Path>) -> Option<(PathBuf, PathBuf)> {
    if let Some(path) = focus {
        if path.is_file() || path.is_dir() {
            if let Some(manifest_root) = nearest_manifest_root(path) {
                if let Some(entry_path) = resolve_entry_path(&manifest_root, focus) {
                    return Some((manifest_root, entry_path));
                }
            }
        }
    }
    let entry_path = resolve_entry_path(root_path, focus)?;
    let workspace_root = workspace_root_dir(root_path, &entry_path);
    Some((workspace_root, entry_path))
}

fn nearest_manifest_root(path: &Path) -> Option<PathBuf> {
    let start = if path.is_file() { path.parent()? } else { path };
    for ancestor in start.ancestors() {
        if ancestor.join("fuse.toml").exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn workspace_root_dir(root_path: &Path, entry_path: &Path) -> PathBuf {
    if root_path.is_dir() {
        return root_path.to_path_buf();
    }
    if let Some(parent) = entry_path.parent() {
        return parent.to_path_buf();
    }
    root_path.to_path_buf()
}

fn resolve_entry_path(root_path: &Path, focus: Option<&Path>) -> Option<PathBuf> {
    if root_path.is_dir() {
        if let Some(entry) = read_manifest_entry(root_path) {
            return Some(entry);
        }
        if let Some(path) = focus {
            if path.is_file() {
                return Some(path.to_path_buf());
            }
        }
        let candidate = root_path.join("src").join("main.fuse");
        if candidate.exists() {
            return Some(candidate);
        }
        if let Some(first) = find_first_fuse_file(root_path) {
            return Some(first);
        }
    }
    if let Some(path) = focus {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    if root_path.is_file() {
        return Some(root_path.to_path_buf());
    }
    None
}

fn read_manifest_entry(root: &Path) -> Option<PathBuf> {
    let manifest = root.join("fuse.toml");
    let contents = std::fs::read_to_string(&manifest).ok()?;
    let mut in_package = false;
    for raw_line in contents.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = parts.next()?.trim();
        if key != "entry" {
            continue;
        }
        let value = parts.next()?.trim();
        let value = value.trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            continue;
        }
        return Some(root.join(value));
    }
    None
}

fn resolve_dependency_roots_for_workspace(workspace_root: &Path) -> HashMap<String, PathBuf> {
    let manifest = workspace_root.join("fuse.toml");
    let contents = match std::fs::read_to_string(&manifest) {
        Ok(contents) => contents,
        Err(_) => return HashMap::new(),
    };

    let mut roots = HashMap::new();
    let mut in_dependencies = false;
    let mut dependency_table: Option<String> = None;

    for raw_line in contents.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            in_dependencies = false;
            dependency_table = None;
            let header = line[1..line.len() - 1].trim();
            if header == "dependencies" {
                in_dependencies = true;
            } else if let Some(dep_name) = header.strip_prefix("dependencies.") {
                let dep_name = unquote_toml_key(dep_name.trim());
                if !dep_name.is_empty() {
                    dependency_table = Some(dep_name);
                }
            }
            continue;
        }

        if let Some(dep_name) = dependency_table.as_deref() {
            let Some((key, value)) = split_toml_assignment(line) else {
                continue;
            };
            if unquote_toml_key(key) != "path" {
                continue;
            }
            let Some(path) = parse_toml_string(value) else {
                continue;
            };
            roots.insert(
                dep_name.to_string(),
                dependency_root_path(workspace_root, &path),
            );
            continue;
        }

        if !in_dependencies {
            continue;
        }
        let Some((dep_name_raw, dep_value_raw)) = split_toml_assignment(line) else {
            continue;
        };
        let dep_name = unquote_toml_key(dep_name_raw);
        if dep_name.is_empty() {
            continue;
        }
        let Some(path) = resolve_dependency_path_value(dep_value_raw) else {
            continue;
        };
        roots.insert(dep_name, dependency_root_path(workspace_root, &path));
    }

    roots
}

fn resolve_dependency_path_value(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(path) = parse_toml_string(value) {
        if looks_like_path_dependency(&path) {
            return Some(path);
        }
        return None;
    }
    if !(value.starts_with('{') && value.ends_with('}')) {
        return None;
    }
    let inner = &value[1..value.len() - 1];
    for part in inner.split(',') {
        let Some((key, path_value)) = split_toml_assignment(part) else {
            continue;
        };
        if unquote_toml_key(key) != "path" {
            continue;
        }
        return parse_toml_string(path_value);
    }
    None
}

fn dependency_root_path(workspace_root: &Path, raw_path: &str) -> PathBuf {
    let path = PathBuf::from(raw_path);
    let joined = if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    };
    normalized_path(joined.as_path())
}

fn split_toml_assignment(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.splitn(2, '=');
    let key = parts.next()?.trim();
    let value = parts.next()?.trim();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    Some((key, value))
}

fn parse_toml_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 2 {
        return None;
    }
    let quote = value.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    if !value.ends_with(quote) {
        return None;
    }
    Some(value[1..value.len() - 1].to_string())
}

fn unquote_toml_key(key: &str) -> String {
    let key = key.trim();
    if key.len() >= 2
        && ((key.starts_with('"') && key.ends_with('"'))
            || (key.starts_with('\'') && key.ends_with('\'')))
    {
        return key[1..key.len() - 1].to_string();
    }
    key.to_string()
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_quote: Option<char> = None;
    for (idx, ch) in line.char_indices() {
        if ch == '"' || ch == '\'' {
            if let Some(active) = in_quote {
                if active == ch {
                    in_quote = None;
                }
            } else {
                in_quote = Some(ch);
            }
            continue;
        }
        if ch == '#' && in_quote.is_none() {
            return &line[..idx];
        }
    }
    line
}

fn looks_like_path_dependency(value: &str) -> bool {
    value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with('/')
        || value.contains('/')
        || value.contains('\\')
}

fn find_first_fuse_file(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let ignore_dirs = [
        ".git",
        ".fuse",
        "target",
        "tmp",
        "dist",
        "build",
        ".cargo-target",
        ".cargo-tmp",
        "node_modules",
    ];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if ignore_dirs.contains(&name) {
                        continue;
                    }
                }
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("fuse") {
                return Some(path);
            }
        }
    }
    None
}

fn build_workspace_from_registry(
    registry: &ModuleRegistry,
    overrides: &HashMap<PathBuf, String>,
) -> Option<WorkspaceIndex> {
    let mut files = Vec::new();
    let mut file_by_uri = HashMap::new();
    let mut module_to_file = HashMap::new();
    let mut modules_sorted: Vec<(usize, &fusec::loader::ModuleUnit)> = registry
        .modules
        .iter()
        .map(|(id, unit)| (*id, unit))
        .collect();
    modules_sorted.sort_by_key(|(id, _)| *id);
    for (id, unit) in modules_sorted {
        let path_str = unit.path.to_string_lossy();
        if path_str.starts_with('<') {
            continue;
        }
        let uri = path_to_uri(&unit.path);
        let key = unit
            .path
            .canonicalize()
            .unwrap_or_else(|_| unit.path.clone());
        let text = overrides
            .get(&key)
            .cloned()
            .or_else(|| std::fs::read_to_string(&unit.path).ok())
            .unwrap_or_default();
        let index = build_index_with_program(&text, &unit.program);
        let def_map = vec![0; index.defs.len()];
        let file_idx = files.len();
        files.push(WorkspaceFile {
            uri: uri.clone(),
            text,
            index,
            def_map,
            qualified_refs: Vec::new(),
        });
        file_by_uri.insert(uri.clone(), file_idx);
        module_to_file.insert(id, file_idx);
    }

    let mut defs = Vec::new();
    for file in files.iter_mut() {
        for (local_id, def) in file.index.defs.iter().enumerate() {
            let global_id = defs.len();
            defs.push(WorkspaceDef {
                id: global_id,
                uri: file.uri.clone(),
                def: def.clone(),
            });
            file.def_map[local_id] = global_id;
        }
    }

    let mut refs = Vec::new();
    for file in &files {
        for reference in &file.index.refs {
            if let Some(global_id) = file.def_map.get(reference.target) {
                refs.push(WorkspaceRef {
                    uri: file.uri.clone(),
                    span: reference.span,
                    target: *global_id,
                });
            }
        }
    }
    let mut calls = Vec::new();
    let mut module_alias_exports = HashMap::new();

    let mut exports_by_module: HashMap<usize, HashMap<String, usize>> = HashMap::new();
    for (module_id, file_idx) in &module_to_file {
        let file = &files[*file_idx];
        let mut exports = HashMap::new();
        for (local_id, def) in file.index.defs.iter().enumerate() {
            if !is_exported_def_kind(def.kind) {
                continue;
            }
            let global_id = file.def_map[local_id];
            exports.entry(def.name.clone()).or_insert(global_id);
        }
        exports_by_module.insert(*module_id, exports);
    }

    let mut redirects = HashMap::new();
    let mut modules_sorted: Vec<(usize, &fusec::loader::ModuleUnit)> = registry
        .modules
        .iter()
        .map(|(id, unit)| (*id, unit))
        .collect();
    modules_sorted.sort_by_key(|(id, _)| *id);
    for (module_id, unit) in modules_sorted {
        let Some(file_idx) = module_to_file.get(&module_id) else {
            continue;
        };
        let file = &mut files[*file_idx];
        for (name, link) in &unit.import_items {
            let Some(exports) = exports_by_module.get(&link.id) else {
                continue;
            };
            let Some(target) = exports.get(name) else {
                continue;
            };
            if let Some(local_def_id) = find_import_def(&file.index, name) {
                let global_id = file.def_map[local_def_id];
                redirects.insert(global_id, *target);
                refs.push(WorkspaceRef {
                    uri: file.uri.clone(),
                    span: file.index.defs[local_def_id].span,
                    target: *target,
                });
            }
        }

        let module_aliases: HashMap<String, usize> = unit
            .modules
            .modules
            .iter()
            .map(|(name, link)| (name.clone(), link.id))
            .collect();
        let mut alias_exports = HashMap::new();
        for (alias, module_id) in &module_aliases {
            if let Some(exports) = exports_by_module.get(module_id) {
                alias_exports.insert(alias.clone(), exports.keys().cloned().collect());
            }
        }
        if !alias_exports.is_empty() {
            module_alias_exports.insert(file.uri.clone(), alias_exports);
        }

        for call in &file.index.calls {
            let Some(from) = file.def_map.get(call.caller).copied() else {
                continue;
            };
            let Some(to) = file.def_map.get(call.callee).copied() else {
                continue;
            };
            calls.push(WorkspaceCall {
                uri: file.uri.clone(),
                span: call.span,
                from,
                to,
            });
        }

        for call in &file.index.qualified_calls {
            let Some(module_id) = module_aliases.get(&call.module) else {
                continue;
            };
            let Some(exports) = exports_by_module.get(module_id) else {
                continue;
            };
            let Some(target) = exports.get(&call.item) else {
                continue;
            };
            let Some(from) = file.def_map.get(call.caller).copied() else {
                continue;
            };
            calls.push(WorkspaceCall {
                uri: file.uri.clone(),
                span: call.span,
                from,
                to: *target,
            });
        }

        let qualified_refs = collect_qualified_refs(&unit.program);
        for qualified in qualified_refs {
            let Some(module_id) = module_aliases.get(&qualified.module) else {
                continue;
            };
            let Some(exports) = exports_by_module.get(module_id) else {
                continue;
            };
            let Some(target) = exports.get(&qualified.item) else {
                continue;
            };
            file.qualified_refs.push(QualifiedRef {
                span: qualified.span,
                target: *target,
            });
            refs.push(WorkspaceRef {
                uri: file.uri.clone(),
                span: qualified.span,
                target: *target,
            });
        }
    }

    for reference in refs.iter_mut() {
        let mut target = reference.target;
        while let Some(next) = redirects.get(&target) {
            if *next == target {
                break;
            }
            target = *next;
        }
        reference.target = target;
    }
    for call in calls.iter_mut() {
        while let Some(next) = redirects.get(&call.from) {
            if *next == call.from {
                break;
            }
            call.from = *next;
        }
        while let Some(next) = redirects.get(&call.to) {
            if *next == call.to {
                break;
            }
            call.to = *next;
        }
        if let Some(to_def) = defs.get(call.to) {
            if to_def.def.kind == SymbolKind::Variable {
                if let Some(import_name) = import_binding_name(&to_def.def.detail) {
                    if let Some(mapped) = unique_callable_target_by_name(&defs, import_name) {
                        call.to = mapped;
                    }
                }
            }
        }
    }
    calls.retain(|call| {
        let Some(from) = defs.get(call.from) else {
            return false;
        };
        let Some(to) = defs.get(call.to) else {
            return false;
        };
        is_callable_def_kind(from.def.kind) && is_callable_def_kind(to.def.kind)
    });

    Some(WorkspaceIndex {
        files,
        file_by_uri,
        defs,
        refs,
        calls,
        module_alias_exports,
        redirects,
    })
}

fn import_binding_name(detail: &str) -> Option<&str> {
    detail
        .strip_prefix("import ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn unique_callable_target_by_name(defs: &[WorkspaceDef], name: &str) -> Option<usize> {
    let mut match_id = None;
    for def in defs {
        if !is_callable_def_kind(def.def.kind) || def.def.name != name {
            continue;
        }
        if match_id.is_some() {
            return None;
        }
        match_id = Some(def.id);
    }
    match_id
}

pub(crate) fn is_exported_def_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Type
            | SymbolKind::Enum
            | SymbolKind::Function
            | SymbolKind::Config
            | SymbolKind::Service
            | SymbolKind::App
            | SymbolKind::Migration
            | SymbolKind::Test
    )
}

pub(crate) fn is_callable_def_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Service
            | SymbolKind::App
            | SymbolKind::Migration
            | SymbolKind::Test
    )
}

fn find_import_def(index: &Index, name: &str) -> Option<usize> {
    index.defs.iter().enumerate().find_map(|(idx, def)| {
        if def.kind != SymbolKind::Variable {
            return None;
        }
        if def.name != name {
            return None;
        }
        if def.detail.starts_with("import ") {
            return Some(idx);
        }
        None
    })
}
