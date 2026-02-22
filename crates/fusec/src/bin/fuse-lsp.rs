use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use fuse_rt::json::{self, JsonValue};
use fusec::ast::{
    BinaryOp, Block, ConfigDecl, Doc, EnumDecl, Expr, ExprKind, FnDecl, Ident, ImportDecl,
    ImportSpec, Item, Literal, Pattern, PatternKind, Program, ServiceDecl, Stmt, StmtKind,
    TypeDecl, TypeDerive, TypeRef, TypeRefKind, UnaryOp,
};
use fusec::diag::{Diag, Level};
use fusec::loader::{
    ModuleExports, ModuleLink, ModuleMap, ModuleRegistry,
    load_program_with_modules_and_deps_and_overrides,
};
use fusec::parse_source;
use fusec::sema;
use fusec::span::Span;

const SEMANTIC_TOKEN_TYPES: [&str; 12] = [
    "namespace",
    "type",
    "enum",
    "enumMember",
    "function",
    "parameter",
    "variable",
    "property",
    "keyword",
    "string",
    "number",
    "comment",
];
const SEM_NAMESPACE: usize = 0;
const SEM_TYPE: usize = 1;
const SEM_ENUM: usize = 2;
const SEM_ENUM_MEMBER: usize = 3;
const SEM_FUNCTION: usize = 4;
const SEM_PARAMETER: usize = 5;
const SEM_VARIABLE: usize = 6;
const SEM_PROPERTY: usize = 7;
const SEM_KEYWORD: usize = 8;
const SEM_STRING: usize = 9;
const SEM_NUMBER: usize = 10;
const SEM_COMMENT: usize = 11;
const COMPLETION_KEYWORDS: [&str; 34] = [
    "app",
    "service",
    "at",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "fn",
    "type",
    "enum",
    "let",
    "var",
    "return",
    "if",
    "else",
    "match",
    "for",
    "in",
    "while",
    "break",
    "continue",
    "import",
    "from",
    "as",
    "config",
    "migration",
    "table",
    "test",
    "body",
    "and",
    "or",
    "without",
    "spawn",
];
const COMPLETION_BUILTIN_RECEIVERS: [&str; 4] = ["db", "json", "html", "svg"];
const COMPLETION_BUILTIN_FUNCTIONS: [&str; 6] = ["print", "env", "serve", "log", "assert", "asset"];
const COMPLETION_BUILTIN_TYPES: [&str; 14] = [
    "Unit", "Int", "Float", "Bool", "String", "Bytes", "Html", "Id", "Email", "Error", "List",
    "Map", "Task", "Range",
];
const STD_ERROR_MODULE_SOURCE: &str = r#"
type Error:
  code: String
  message: String

type ValidationField:
  path: String
  code: String
  message: String

type Validation:
  message: String
  fields: List<ValidationField>

type BadRequest:
  message: String

type Unauthorized:
  message: String

type Forbidden:
  message: String

type NotFound:
  message: String

type Conflict:
  message: String
"#;

fn main() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut state = LspState::default();
    let mut shutdown = false;

    loop {
        let message = match read_message(&mut stdin)? {
            Some(value) => value,
            None => break,
        };
        let value = match json::decode(&message) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let JsonValue::Object(obj) = value else {
            continue;
        };
        let method = get_string(&obj, "method");
        let id = obj.get("id").cloned();

        if method.as_deref() == Some("$/cancelRequest") {
            handle_cancel(&mut state, &obj);
            continue;
        }

        if let Some(err) = cancelled_error(&mut state, id.as_ref()) {
            if id.is_some() {
                let response = json_error_response(id, -32800, &err);
                write_message(&mut stdout, &response)?;
            }
            continue;
        }

        match method.as_deref() {
            Some("initialize") => {
                state.root_uri = extract_root_uri(&obj);
                state.invalidate_workspace_cache();
                let result = capabilities_result();
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("initialized") => {}
            Some("shutdown") => {
                shutdown = true;
                let response = json_response(id, JsonValue::Null);
                write_message(&mut stdout, &response)?;
            }
            Some("exit") => {
                if shutdown {
                    break;
                } else {
                    std::process::exit(1);
                }
            }
            Some("textDocument/didOpen") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = extract_text_doc_text(&obj) {
                        apply_doc_overlay_change(&mut state, &uri, Some(text.clone()));
                        publish_diagnostics(&mut stdout, &mut state, &uri, &text)?;
                    }
                }
            }
            Some("textDocument/didChange") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = extract_change_text(&obj) {
                        apply_doc_overlay_change(&mut state, &uri, Some(text.clone()));
                        publish_diagnostics(&mut stdout, &mut state, &uri, &text)?;
                    }
                }
            }
            Some("textDocument/didClose") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    apply_doc_overlay_change(&mut state, &uri, None);
                    publish_empty_diagnostics(&mut stdout, &uri)?;
                }
            }
            Some("textDocument/formatting") => {
                let mut edits = Vec::new();
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = state.docs.get(&uri).cloned() {
                        let formatted = fusec::format::format_source(&text);
                        if formatted != text {
                            edits.push(full_document_edit(&text, &formatted));
                            apply_doc_overlay_change(&mut state, &uri, Some(formatted));
                        }
                    }
                }
                let response = json_response(id, JsonValue::Array(edits));
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/definition") => {
                let result = handle_definition(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/hover") => {
                let result = handle_hover(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/signatureHelp") => {
                let result = handle_signature_help(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/completion") => {
                let result = handle_completion(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/rename") => {
                let result = handle_rename(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/prepareRename") => {
                let result = handle_prepare_rename(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/references") => {
                let result = handle_references(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/prepareCallHierarchy") => {
                let result = handle_prepare_call_hierarchy(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("callHierarchy/incomingCalls") => {
                let result = handle_call_hierarchy_incoming(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("callHierarchy/outgoingCalls") => {
                let result = handle_call_hierarchy_outgoing(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("workspace/symbol") => {
                let result = handle_workspace_symbol(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/codeAction") => {
                let result = handle_code_action(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/semanticTokens/full") => {
                let result = handle_semantic_tokens(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/semanticTokens/range") => {
                let result = handle_semantic_tokens_range(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("textDocument/inlayHint") => {
                let result = handle_inlay_hints(&mut state, &obj);
                let response = json_response(id, result);
                write_message(&mut stdout, &response)?;
            }
            Some("fuse/internalWorkspaceStats") => {
                let response = json_response(id, workspace_stats_result(&state));
                write_message(&mut stdout, &response)?;
            }
            _ => {
                if id.is_some() {
                    let response = json_response(id, JsonValue::Null);
                    write_message(&mut stdout, &response)?;
                }
            }
        }
    }
    Ok(())
}

fn capabilities_result() -> JsonValue {
    let mut caps = BTreeMap::new();
    caps.insert("textDocumentSync".to_string(), JsonValue::Number(1.0));
    caps.insert("definitionProvider".to_string(), JsonValue::Bool(true));
    caps.insert("hoverProvider".to_string(), JsonValue::Bool(true));
    let mut signature_help = BTreeMap::new();
    signature_help.insert(
        "triggerCharacters".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("(".to_string()),
            JsonValue::String(",".to_string()),
        ]),
    );
    signature_help.insert(
        "retriggerCharacters".to_string(),
        JsonValue::Array(vec![JsonValue::String(",".to_string())]),
    );
    caps.insert(
        "signatureHelpProvider".to_string(),
        JsonValue::Object(signature_help),
    );
    let mut completion_provider = BTreeMap::new();
    completion_provider.insert("resolveProvider".to_string(), JsonValue::Bool(false));
    completion_provider.insert(
        "triggerCharacters".to_string(),
        JsonValue::Array(vec![JsonValue::String(".".to_string())]),
    );
    caps.insert(
        "completionProvider".to_string(),
        JsonValue::Object(completion_provider),
    );
    let mut rename_provider = BTreeMap::new();
    rename_provider.insert("prepareProvider".to_string(), JsonValue::Bool(true));
    caps.insert(
        "renameProvider".to_string(),
        JsonValue::Object(rename_provider),
    );
    caps.insert("referencesProvider".to_string(), JsonValue::Bool(true));
    caps.insert("callHierarchyProvider".to_string(), JsonValue::Bool(true));
    let mut code_action = BTreeMap::new();
    code_action.insert(
        "codeActionKinds".to_string(),
        JsonValue::Array(vec![
            JsonValue::String("quickfix".to_string()),
            JsonValue::String("source.organizeImports".to_string()),
        ]),
    );
    caps.insert(
        "codeActionProvider".to_string(),
        JsonValue::Object(code_action),
    );
    caps.insert("inlayHintProvider".to_string(), JsonValue::Bool(true));
    let mut semantic = BTreeMap::new();
    let mut legend = BTreeMap::new();
    legend.insert(
        "tokenTypes".to_string(),
        JsonValue::Array(
            SEMANTIC_TOKEN_TYPES
                .iter()
                .map(|name| JsonValue::String((*name).to_string()))
                .collect(),
        ),
    );
    legend.insert("tokenModifiers".to_string(), JsonValue::Array(Vec::new()));
    semantic.insert("legend".to_string(), JsonValue::Object(legend));
    semantic.insert("full".to_string(), JsonValue::Bool(true));
    semantic.insert("range".to_string(), JsonValue::Bool(true));
    caps.insert(
        "semanticTokensProvider".to_string(),
        JsonValue::Object(semantic),
    );
    caps.insert("workspaceSymbolProvider".to_string(), JsonValue::Bool(true));
    let mut root = BTreeMap::new();
    root.insert("capabilities".to_string(), JsonValue::Object(caps));
    JsonValue::Object(root)
}

#[derive(Default)]
struct LspState {
    docs: BTreeMap<String, String>,
    root_uri: Option<String>,
    cancelled: HashSet<String>,
    docs_revision: u64,
    workspace_cache: Option<WorkspaceCache>,
    workspace_builds: u64,
}

impl LspState {
    fn invalidate_workspace_cache(&mut self) {
        self.docs_revision = self.docs_revision.saturating_add(1);
        self.workspace_cache = None;
    }
}

struct WorkspaceCache {
    docs_revision: u64,
    workspace_key: String,
    snapshot: WorkspaceSnapshot,
}

struct WorkspaceSnapshot {
    registry: ModuleRegistry,
    overrides: HashMap<PathBuf, String>,
    workspace_root: PathBuf,
    dep_roots: HashMap<String, PathBuf>,
    loader_diags: Vec<Diag>,
    module_ids_by_path: HashMap<PathBuf, usize>,
    index: Option<WorkspaceIndex>,
}

fn handle_cancel(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) {
    let Some(JsonValue::Object(params)) = obj.get("params") else {
        return;
    };
    let Some(id) = params.get("id") else {
        return;
    };
    if let Some(key) = request_id_key(id) {
        state.cancelled.insert(key);
    }
}

fn cancelled_error(state: &mut LspState, id: Option<&JsonValue>) -> Option<String> {
    let id = id?;
    let key = request_id_key(id)?;
    if state.cancelled.remove(&key) {
        Some("request cancelled".to_string())
    } else {
        None
    }
}

fn request_id_key(id: &JsonValue) -> Option<String> {
    match id {
        JsonValue::Number(num) => Some(format!("{num}")),
        JsonValue::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn apply_doc_overlay_change(state: &mut LspState, uri: &str, text: Option<String>) {
    if let Some(contents) = text.as_ref() {
        state.docs.insert(uri.to_string(), contents.clone());
    } else {
        state.docs.remove(uri);
    }
    state.docs_revision = state.docs_revision.saturating_add(1);
    if !try_incremental_module_update(state, uri, text.as_deref()) {
        state.workspace_cache = None;
    }
}

fn try_incremental_module_update(state: &mut LspState, uri: &str, text: Option<&str>) -> bool {
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

fn workspace_stats_result(state: &LspState) -> JsonValue {
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

fn publish_diagnostics(
    out: &mut impl Write,
    state: &mut LspState,
    uri: &str,
    text: &str,
) -> io::Result<()> {
    let diags = workspace_diags_for_uri(state, uri, text).unwrap_or_else(|| {
        let mut diags = Vec::new();
        let (program, parse_diags) = parse_source(text);
        diags.extend(parse_diags);
        if !diags.iter().any(|d| matches!(d.level, Level::Error)) {
            let (_analysis, sema_diags) = sema::analyze_program(&program);
            diags.extend(sema_diags);
        }
        diags
    });
    let diagnostics = to_lsp_diags(text, &diags);
    let params = diagnostics_params(uri, diagnostics);
    let notification = json_notification("textDocument/publishDiagnostics", params);
    write_message(out, &notification)
}

fn workspace_diags_for_uri(state: &mut LspState, uri: &str, _text: &str) -> Option<Vec<Diag>> {
    let focus_path = uri_to_path(uri)?;
    let focus_key = focus_path
        .canonicalize()
        .unwrap_or_else(|_| focus_path.clone());
    let snapshot = build_workspace_snapshot_cached(state, uri)?;
    if let Some(module_id) = snapshot.module_ids_by_path.get(&focus_key).copied() {
        let (_, sema_diags) = sema::analyze_module(&snapshot.registry, module_id);
        let mut diags = Vec::new();
        for diag in &snapshot.loader_diags {
            if diag.path.is_none() {
                diags.push(diag.clone());
                continue;
            }
            if let Some(path) = diag.path.as_ref() {
                let key = path.canonicalize().unwrap_or_else(|_| path.clone());
                if key == focus_key {
                    diags.push(diag.clone());
                }
            }
        }
        diags.extend(sema_diags);
        return Some(diags);
    }

    let focus_snapshot = build_focus_workspace_snapshot(state, uri)?;
    let module_id = *focus_snapshot.module_ids_by_path.get(&focus_key)?;
    let (_, sema_diags) = sema::analyze_module(&focus_snapshot.registry, module_id);
    let mut diags = Vec::new();
    for diag in &focus_snapshot.loader_diags {
        if diag.path.is_none() {
            diags.push(diag.clone());
            continue;
        }
        if let Some(path) = diag.path.as_ref() {
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if key == focus_key {
                diags.push(diag.clone());
            }
        }
    }
    diags.extend(sema_diags);
    Some(diags)
}

fn publish_empty_diagnostics(out: &mut impl Write, uri: &str) -> io::Result<()> {
    let params = diagnostics_params(uri, Vec::new());
    let notification = json_notification("textDocument/publishDiagnostics", params);
    write_message(out, &notification)
}

fn diagnostics_params(uri: &str, diagnostics: Vec<JsonValue>) -> JsonValue {
    let mut params = BTreeMap::new();
    params.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    params.insert("diagnostics".to_string(), JsonValue::Array(diagnostics));
    JsonValue::Object(params)
}

fn to_lsp_diags(text: &str, diags: &[Diag]) -> Vec<JsonValue> {
    let line_offsets = line_offsets(text);
    diags
        .iter()
        .map(|diag| {
            let (start_line, start_col) = offset_to_line_col(&line_offsets, diag.span.start);
            let (end_line, end_col) = offset_to_line_col(&line_offsets, diag.span.end);
            let range = range_json(start_line, start_col, end_line, end_col);
            let severity = match diag.level {
                Level::Error => 1.0,
                Level::Warning => 2.0,
            };
            let mut out = BTreeMap::new();
            out.insert("range".to_string(), range);
            out.insert("severity".to_string(), JsonValue::Number(severity));
            out.insert(
                "message".to_string(),
                JsonValue::String(diag.message.clone()),
            );
            out.insert("source".to_string(), JsonValue::String("fusec".to_string()));
            JsonValue::Object(out)
        })
        .collect()
}

fn full_document_edit(original: &str, new_text: &str) -> JsonValue {
    let offsets = line_offsets(original);
    let end_offset = original.len();
    let (end_line, end_col) = offset_to_line_col(&offsets, end_offset);
    let range = range_json(0, 0, end_line, end_col);
    let mut edit = BTreeMap::new();
    edit.insert("range".to_string(), range);
    edit.insert(
        "newText".to_string(),
        JsonValue::String(new_text.to_string()),
    );
    JsonValue::Object(edit)
}

fn range_json(start_line: usize, start_col: usize, end_line: usize, end_col: usize) -> JsonValue {
    let mut start = BTreeMap::new();
    start.insert("line".to_string(), JsonValue::Number(start_line as f64));
    start.insert("character".to_string(), JsonValue::Number(start_col as f64));
    let mut end = BTreeMap::new();
    end.insert("line".to_string(), JsonValue::Number(end_line as f64));
    end.insert("character".to_string(), JsonValue::Number(end_col as f64));
    let mut range = BTreeMap::new();
    range.insert("start".to_string(), JsonValue::Object(start));
    range.insert("end".to_string(), JsonValue::Object(end));
    JsonValue::Object(range)
}

fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0usize];
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            offsets.push(idx + 1);
        }
    }
    offsets
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    if !uri.starts_with("file://") {
        return None;
    }
    let mut raw = uri.trim_start_matches("file://").to_string();
    if raw.starts_with('/') && raw.len() > 2 && raw.as_bytes()[2] == b':' {
        raw.remove(0);
    }
    let decoded = decode_uri_component(&raw);
    if decoded.is_empty() {
        return None;
    }
    Some(PathBuf::from(decoded))
}

fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().to_string();
    if raw.contains("://") {
        return raw;
    }
    format!("file://{}", raw)
}

fn decode_uri_component(value: &str) -> String {
    let mut out = String::new();
    let bytes = value.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let (Some(a), Some(b)) = (hex_val(bytes[idx + 1]), hex_val(bytes[idx + 2])) {
                out.push((a * 16 + b) as char);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx] as char);
        idx += 1;
    }
    out
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn offset_to_line_col(offsets: &[usize], offset: usize) -> (usize, usize) {
    let mut lo = 0usize;
    let mut hi = offsets.len();
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if offsets[mid] <= offset {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let line = lo;
    let col = offset.saturating_sub(offsets[lo]);
    (line, col)
}

fn line_col_to_offset(text: &str, offsets: &[usize], line: usize, col: usize) -> usize {
    if offsets.is_empty() {
        return 0;
    }
    let line = line.min(offsets.len() - 1);
    let start = offsets[line];
    let end = offsets.get(line + 1).copied().unwrap_or_else(|| text.len());
    let offset = start.saturating_add(col);
    offset.min(end)
}

fn json_response(id: Option<JsonValue>, result: JsonValue) -> JsonValue {
    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    if let Some(id) = id {
        root.insert("id".to_string(), id);
    } else {
        root.insert("id".to_string(), JsonValue::Null);
    }
    root.insert("result".to_string(), result);
    JsonValue::Object(root)
}

fn json_error_response(id: Option<JsonValue>, code: i64, message: &str) -> JsonValue {
    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    if let Some(id) = id {
        root.insert("id".to_string(), id);
    } else {
        root.insert("id".to_string(), JsonValue::Null);
    }
    let mut err = BTreeMap::new();
    err.insert("code".to_string(), JsonValue::Number(code as f64));
    err.insert(
        "message".to_string(),
        JsonValue::String(message.to_string()),
    );
    root.insert("error".to_string(), JsonValue::Object(err));
    JsonValue::Object(root)
}

fn json_notification(method: &str, params: JsonValue) -> JsonValue {
    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::String("2.0".to_string()));
    root.insert("method".to_string(), JsonValue::String(method.to_string()));
    root.insert("params".to_string(), params);
    JsonValue::Object(root)
}

fn get_string(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(JsonValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn extract_text_doc_uri(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else {
        return None;
    };
    match text_doc.get("uri") {
        Some(JsonValue::String(uri)) => Some(uri.clone()),
        _ => None,
    }
}

fn extract_text_doc_text(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else {
        return None;
    };
    match text_doc.get("text") {
        Some(JsonValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn extract_change_text(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let changes = params.get("contentChanges")?;
    let JsonValue::Array(changes) = changes else {
        return None;
    };
    let first = changes.get(0)?;
    let JsonValue::Object(first) = first else {
        return None;
    };
    match first.get("text") {
        Some(JsonValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn extract_position(obj: &BTreeMap<String, JsonValue>) -> Option<(String, usize, usize)> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let text_doc = params.get("textDocument")?;
    let JsonValue::Object(text_doc) = text_doc else {
        return None;
    };
    let uri = match text_doc.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    let position = params.get("position")?;
    let JsonValue::Object(position) = position else {
        return None;
    };
    let line = match position.get("line") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let character = match position.get("character") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    Some((uri, line, character))
}

fn extract_new_name(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    match params.get("newName") {
        Some(JsonValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn extract_workspace_query(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    match params.get("query") {
        Some(JsonValue::String(query)) => Some(query.clone()),
        _ => None,
    }
}

fn extract_include_declaration(obj: &BTreeMap<String, JsonValue>) -> bool {
    let Some(JsonValue::Object(params)) = obj.get("params") else {
        return true;
    };
    match params.get("context") {
        Some(JsonValue::Object(context)) => match context.get("includeDeclaration") {
            Some(JsonValue::Bool(value)) => *value,
            _ => true,
        },
        _ => true,
    }
}

fn extract_root_uri(obj: &BTreeMap<String, JsonValue>) -> Option<String> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    if let Some(JsonValue::String(uri)) = params.get("rootUri") {
        if !uri.is_empty() {
            return Some(uri.clone());
        }
    }
    if let Some(JsonValue::String(path)) = params.get("rootPath") {
        if !path.is_empty() {
            return Some(path_to_uri(Path::new(path)));
        }
    }
    None
}

#[derive(Clone)]
struct CodeActionDiag {
    message: String,
    span: Option<Span>,
}

fn extract_code_action_diagnostics(
    obj: &BTreeMap<String, JsonValue>,
    text: &str,
) -> Vec<CodeActionDiag> {
    let mut out = Vec::new();
    let Some(JsonValue::Object(params)) = obj.get("params") else {
        return out;
    };
    let Some(JsonValue::Object(context)) = params.get("context") else {
        return out;
    };
    let Some(JsonValue::Array(diags)) = context.get("diagnostics") else {
        return out;
    };
    let offsets = line_offsets(text);
    for diag in diags {
        let JsonValue::Object(diag_obj) = diag else {
            continue;
        };
        let message = match diag_obj.get("message") {
            Some(JsonValue::String(value)) => value.clone(),
            _ => continue,
        };
        let span = diag_obj
            .get("range")
            .and_then(|range| lsp_range_to_span(range, text, &offsets));
        out.push(CodeActionDiag { message, span });
    }
    out
}

fn lsp_range_to_span(range: &JsonValue, text: &str, offsets: &[usize]) -> Option<Span> {
    let JsonValue::Object(range_obj) = range else {
        return None;
    };
    let JsonValue::Object(start) = range_obj.get("start")? else {
        return None;
    };
    let JsonValue::Object(end) = range_obj.get("end")? else {
        return None;
    };
    let start_line = match start.get("line") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let start_col = match start.get("character") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let end_line = match end.get("line") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let end_col = match end.get("character") {
        Some(JsonValue::Number(num)) => *num as usize,
        _ => return None,
    };
    let start_offset = line_col_to_offset(text, offsets, start_line, start_col);
    let end_offset = line_col_to_offset(text, offsets, end_line, end_col);
    Some(Span::new(start_offset, end_offset))
}

fn read_message(reader: &mut impl Read) -> io::Result<Option<String>> {
    let mut header = Vec::new();
    let mut buf = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            if header.is_empty() {
                return Ok(None);
            }
            break;
        }
        header.extend_from_slice(&buf[..n]);
    }
    let header_text = String::from_utf8_lossy(&header);
    let mut content_length = None;
    for line in header_text.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let Some(len) = content_length else {
        return Ok(None);
    };
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(String::from_utf8_lossy(&body).to_string()))
}

fn write_message(out: &mut impl Write, value: &JsonValue) -> io::Result<()> {
    let body = json::encode(value);
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

fn handle_definition(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    let Some(def_text) = index.file_text(&def.uri) else {
        return JsonValue::Null;
    };
    let location = location_json(&def.uri, def_text, def.def.span);
    JsonValue::Array(vec![location])
}

fn handle_hover(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    let mut value = format!(
        "**{}** `{}`\n\n```fuse\n{}\n```",
        def.def.kind.hover_label(),
        def.def.name,
        def.def.detail.trim()
    );
    if let Some(doc) = &def.def.doc {
        if !doc.trim().is_empty() {
            value.push_str("\n\n");
            value.push_str(doc.trim());
        }
    }
    let mut contents = BTreeMap::new();
    contents.insert(
        "kind".to_string(),
        JsonValue::String("markdown".to_string()),
    );
    contents.insert("value".to_string(), JsonValue::String(value));
    let mut out = BTreeMap::new();
    out.insert("contents".to_string(), JsonValue::Object(contents));
    if let Some(text) = index.file_text(&def.uri) {
        out.insert("range".to_string(), span_range_json(text, def.def.span));
    }
    JsonValue::Object(out)
}

struct SignatureInfo {
    label: String,
    params: Vec<String>,
    documentation: Option<String>,
}

#[derive(Clone)]
enum SignatureTarget {
    Ident { name: String, span: Span },
    Member { base: Option<String>, span: Span },
    Other { span: Span },
}

#[derive(Clone)]
struct CallContext {
    span: Span,
    target: SignatureTarget,
    active_arg: usize,
}

fn handle_signature_help(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Null;
    };
    let offsets = line_offsets(&text);
    let cursor = line_col_to_offset(&text, &offsets, line, character);
    let (program, _parse_diags) = parse_source(&text);
    let Some(call) = find_call_context(&program, cursor) else {
        return JsonValue::Null;
    };

    let index = build_workspace_index_cached(state, &uri);
    let signature = match &call.target {
        SignatureTarget::Ident { name, span } => {
            signature_info_for_symbol_ref(index, &uri, &offsets, *span)
                .or_else(|| builtin_signature_info(name))
        }
        SignatureTarget::Member { base, span } => {
            signature_info_for_symbol_ref(index, &uri, &offsets, *span).or_else(|| {
                base.as_deref()
                    .and_then(builtin_member_signature_info)
                    .or_else(|| base.as_deref().and_then(builtin_signature_info))
            })
        }
        SignatureTarget::Other { span } => {
            signature_info_for_symbol_ref(index, &uri, &offsets, *span)
        }
    };

    let Some(signature) = signature else {
        return JsonValue::Null;
    };
    signature_help_json(&signature, call.active_arg)
}

fn signature_help_json(signature: &SignatureInfo, active_arg: usize) -> JsonValue {
    let mut signature_obj = BTreeMap::new();
    signature_obj.insert(
        "label".to_string(),
        JsonValue::String(signature.label.clone()),
    );
    let params = signature
        .params
        .iter()
        .map(|label| {
            let mut param = BTreeMap::new();
            param.insert("label".to_string(), JsonValue::String(label.clone()));
            JsonValue::Object(param)
        })
        .collect();
    signature_obj.insert("parameters".to_string(), JsonValue::Array(params));
    if let Some(doc) = &signature.documentation {
        if !doc.trim().is_empty() {
            signature_obj.insert("documentation".to_string(), JsonValue::String(doc.clone()));
        }
    }

    let mut out = BTreeMap::new();
    out.insert(
        "signatures".to_string(),
        JsonValue::Array(vec![JsonValue::Object(signature_obj)]),
    );
    out.insert("activeSignature".to_string(), JsonValue::Number(0.0));
    let active_param = if signature.params.is_empty() {
        0usize
    } else {
        active_arg.min(signature.params.len().saturating_sub(1))
    };
    out.insert(
        "activeParameter".to_string(),
        JsonValue::Number(active_param as f64),
    );
    JsonValue::Object(out)
}

fn signature_info_for_symbol_ref(
    index: Option<&WorkspaceIndex>,
    uri: &str,
    offsets: &[usize],
    span: Span,
) -> Option<SignatureInfo> {
    let index = index?;
    let (line, col) = offset_to_line_col(offsets, span.start);
    let def = index.definition_at(uri, line, col)?;
    if def.def.kind != SymbolKind::Function {
        return None;
    }
    signature_info_from_function_detail(&def.def.detail, def.def.doc.as_deref())
}

fn signature_info_from_function_detail(detail: &str, doc: Option<&str>) -> Option<SignatureInfo> {
    let params = parse_fn_parameter_labels(detail)?;
    Some(SignatureInfo {
        label: detail.trim().to_string(),
        params,
        documentation: doc.map(|value| value.trim().to_string()),
    })
}

fn builtin_signature_info(name: &str) -> Option<SignatureInfo> {
    match name {
        "print" => Some(SignatureInfo {
            label: "fn print(value)".to_string(),
            params: vec!["value".to_string()],
            documentation: Some("Prints a value to stdout.".to_string()),
        }),
        "log" => Some(SignatureInfo {
            label: "fn log(level_or_message, message?, data?)".to_string(),
            params: vec![
                "level_or_message".to_string(),
                "message".to_string(),
                "data".to_string(),
            ],
            documentation: Some("Writes structured log output.".to_string()),
        }),
        "env" => Some(SignatureInfo {
            label: "fn env(name: String) -> String?".to_string(),
            params: vec!["name: String".to_string()],
            documentation: Some("Returns an environment variable value or null.".to_string()),
        }),
        "serve" => Some(SignatureInfo {
            label: "fn serve(port: Int)".to_string(),
            params: vec!["port: Int".to_string()],
            documentation: Some("Starts HTTP server on the given port.".to_string()),
        }),
        "assert" => Some(SignatureInfo {
            label: "fn assert(cond: Bool, message?)".to_string(),
            params: vec!["cond: Bool".to_string(), "message".to_string()],
            documentation: Some("Raises runtime error when condition is false.".to_string()),
        }),
        "asset" => Some(SignatureInfo {
            label: "fn asset(path: String) -> String".to_string(),
            params: vec!["path: String".to_string()],
            documentation: Some("Resolves logical asset path to public URL.".to_string()),
        }),
        _ => None,
    }
}

fn builtin_member_signature_info(base: &str) -> Option<SignatureInfo> {
    match base {
        "svg" => Some(SignatureInfo {
            label: "fn svg.inline(path: String) -> Html".to_string(),
            params: vec!["path: String".to_string()],
            documentation: Some(
                "Loads an SVG by logical name and returns inline Html.".to_string(),
            ),
        }),
        _ => None,
    }
}

fn find_call_context(program: &Program, cursor: usize) -> Option<CallContext> {
    let mut best = None;
    for item in &program.items {
        collect_call_context_item(item, cursor, &mut best);
    }
    best
}

fn collect_call_context_item(item: &Item, cursor: usize, best: &mut Option<CallContext>) {
    match item {
        Item::Import(_) => {}
        Item::Type(decl) => {
            for field in &decl.fields {
                collect_call_context_type_ref(&field.ty, cursor, best);
                if let Some(default) = &field.default {
                    collect_call_context_expr(default, cursor, best);
                }
            }
        }
        Item::Enum(decl) => {
            for variant in &decl.variants {
                for ty in &variant.payload {
                    collect_call_context_type_ref(ty, cursor, best);
                }
            }
        }
        Item::Fn(decl) => {
            for param in &decl.params {
                collect_call_context_type_ref(&param.ty, cursor, best);
                if let Some(default) = &param.default {
                    collect_call_context_expr(default, cursor, best);
                }
            }
            if let Some(ret) = &decl.ret {
                collect_call_context_type_ref(ret, cursor, best);
            }
            collect_call_context_block(&decl.body, cursor, best);
        }
        Item::Service(decl) => {
            for route in &decl.routes {
                collect_call_context_type_ref(&route.ret_type, cursor, best);
                if let Some(body_ty) = &route.body_type {
                    collect_call_context_type_ref(body_ty, cursor, best);
                }
                collect_call_context_block(&route.body, cursor, best);
            }
        }
        Item::Config(decl) => {
            for field in &decl.fields {
                collect_call_context_type_ref(&field.ty, cursor, best);
                collect_call_context_expr(&field.value, cursor, best);
            }
        }
        Item::App(decl) => collect_call_context_block(&decl.body, cursor, best),
        Item::Migration(decl) => collect_call_context_block(&decl.body, cursor, best),
        Item::Test(decl) => collect_call_context_block(&decl.body, cursor, best),
    }
}

fn collect_call_context_block(block: &Block, cursor: usize, best: &mut Option<CallContext>) {
    if !span_contains(block.span, cursor) {
        return;
    }
    for stmt in &block.stmts {
        collect_call_context_stmt(stmt, cursor, best);
    }
}

fn collect_call_context_stmt(stmt: &Stmt, cursor: usize, best: &mut Option<CallContext>) {
    if !span_contains(stmt.span, cursor) {
        return;
    }
    match &stmt.kind {
        StmtKind::Let { ty, expr, .. } | StmtKind::Var { ty, expr, .. } => {
            if let Some(ty) = ty {
                collect_call_context_type_ref(ty, cursor, best);
            }
            collect_call_context_expr(expr, cursor, best);
        }
        StmtKind::Assign { target, expr } => {
            collect_call_context_expr(target, cursor, best);
            collect_call_context_expr(expr, cursor, best);
        }
        StmtKind::Return { expr } => {
            if let Some(expr) = expr {
                collect_call_context_expr(expr, cursor, best);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            collect_call_context_expr(cond, cursor, best);
            collect_call_context_block(then_block, cursor, best);
            for (cond, block) in else_if {
                collect_call_context_expr(cond, cursor, best);
                collect_call_context_block(block, cursor, best);
            }
            if let Some(block) = else_block {
                collect_call_context_block(block, cursor, best);
            }
        }
        StmtKind::Match { expr, cases } => {
            collect_call_context_expr(expr, cursor, best);
            for (_, block) in cases {
                collect_call_context_block(block, cursor, best);
            }
        }
        StmtKind::For { iter, block, .. } => {
            collect_call_context_expr(iter, cursor, best);
            collect_call_context_block(block, cursor, best);
        }
        StmtKind::While { cond, block } => {
            collect_call_context_expr(cond, cursor, best);
            collect_call_context_block(block, cursor, best);
        }
        StmtKind::Expr(expr) => collect_call_context_expr(expr, cursor, best),
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_call_context_expr(expr: &Expr, cursor: usize, best: &mut Option<CallContext>) {
    if !span_contains(expr.span, cursor) {
        return;
    }
    match &expr.kind {
        ExprKind::Call { callee, args } => {
            consider_call_context(best, expr.span, callee, args, cursor);
            collect_call_context_expr(callee, cursor, best);
            for arg in args {
                collect_call_context_expr(&arg.value, cursor, best);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            collect_call_context_expr(left, cursor, best);
            collect_call_context_expr(right, cursor, best);
        }
        ExprKind::Unary { expr, .. } | ExprKind::Await { expr } | ExprKind::Box { expr } => {
            collect_call_context_expr(expr, cursor, best);
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            collect_call_context_expr(base, cursor, best);
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            collect_call_context_expr(base, cursor, best);
            collect_call_context_expr(index, cursor, best);
        }
        ExprKind::StructLit { fields, .. } => {
            for field in fields {
                collect_call_context_expr(&field.value, cursor, best);
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                collect_call_context_expr(item, cursor, best);
            }
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                collect_call_context_expr(key, cursor, best);
                collect_call_context_expr(value, cursor, best);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(expr) = part {
                    collect_call_context_expr(expr, cursor, best);
                }
            }
        }
        ExprKind::Coalesce { left, right } => {
            collect_call_context_expr(left, cursor, best);
            collect_call_context_expr(right, cursor, best);
        }
        ExprKind::BangChain { expr, error } => {
            collect_call_context_expr(expr, cursor, best);
            if let Some(error) = error {
                collect_call_context_expr(error, cursor, best);
            }
        }
        ExprKind::Spawn { block } => collect_call_context_block(block, cursor, best),
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
    }
}

fn collect_call_context_type_ref(ty: &TypeRef, cursor: usize, best: &mut Option<CallContext>) {
    if !span_contains(ty.span, cursor) {
        return;
    }
    match &ty.kind {
        TypeRefKind::Simple(_) => {}
        TypeRefKind::Generic { args, .. } => {
            for arg in args {
                collect_call_context_type_ref(arg, cursor, best);
            }
        }
        TypeRefKind::Optional(inner) => collect_call_context_type_ref(inner, cursor, best),
        TypeRefKind::Result { ok, err } => {
            collect_call_context_type_ref(ok, cursor, best);
            if let Some(err) = err {
                collect_call_context_type_ref(err, cursor, best);
            }
        }
        TypeRefKind::Refined { args, .. } => {
            for arg in args {
                collect_call_context_expr(arg, cursor, best);
            }
        }
    }
}

fn consider_call_context(
    best: &mut Option<CallContext>,
    span: Span,
    callee: &Expr,
    args: &[fusec::ast::CallArg],
    cursor: usize,
) {
    let target = match &callee.kind {
        ExprKind::Ident(ident) => SignatureTarget::Ident {
            name: ident.name.clone(),
            span: ident.span,
        },
        ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
            let base = match &base.kind {
                ExprKind::Ident(base_ident) => Some(base_ident.name.clone()),
                _ => None,
            };
            SignatureTarget::Member {
                base,
                span: name.span,
            }
        }
        _ => SignatureTarget::Other { span: callee.span },
    };
    let candidate = CallContext {
        span,
        target,
        active_arg: call_active_argument(args, cursor),
    };
    let span_len = candidate.span.end.saturating_sub(candidate.span.start);
    let replace = best.as_ref().map_or(true, |current| {
        span_len < current.span.end.saturating_sub(current.span.start)
    });
    if replace {
        *best = Some(candidate);
    }
}

fn call_active_argument(args: &[fusec::ast::CallArg], cursor: usize) -> usize {
    if args.is_empty() {
        return 0;
    }
    if cursor <= args[0].span.start {
        return 0;
    }
    for (idx, arg) in args.iter().enumerate() {
        if cursor <= arg.span.end {
            return idx;
        }
        if let Some(next) = args.get(idx + 1) {
            if cursor < next.span.start {
                return idx + 1;
            }
        }
    }
    args.len()
}

#[derive(Clone)]
struct CompletionCandidate {
    kind: u32,
    detail: Option<String>,
    sort_group: u8,
}

fn handle_completion(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return completion_list_json(Vec::new());
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return completion_list_json(Vec::new());
    };
    let offsets = line_offsets(&text);
    let offset = line_col_to_offset(&text, &offsets, line, character);
    let prefix_start = completion_ident_start(&text, offset);
    let prefix = text.get(prefix_start..offset).unwrap_or_default();
    let member_base = completion_member_base(&text, prefix_start);
    let (program, _parse_diags) = parse_source(&text);
    let index = build_workspace_index_cached(state, &uri);
    let current_container = completion_callable_name_at_cursor(&program, offset)
        .or_else(|| index.and_then(|index| completion_container_name(index, &uri, offset)));
    let mut candidates: HashMap<String, CompletionCandidate> = HashMap::new();

    if let Some(base) = member_base {
        if let Some(index) = index {
            for name in index.alias_exports_for_module(&uri, &base) {
                let mut kind = 3u32;
                let mut detail = None;
                if let Some(def) = index
                    .defs
                    .iter()
                    .find(|def| def.def.name == name && is_exported_def_kind(def.def.kind))
                {
                    kind = completion_kind_for_symbol_kind(def.def.kind);
                    if !def.def.detail.is_empty() {
                        detail = Some(def.def.detail.clone());
                    }
                }
                upsert_completion_candidate(&mut candidates, &name, kind, detail, 0);
            }
        }
        for method in builtin_receiver_methods(&base) {
            upsert_completion_candidate(
                &mut candidates,
                method,
                2,
                Some(format!("{base} builtin")),
                0,
            );
        }
    } else {
        if let Some(index) = index {
            for def in &index.defs {
                let sort_group =
                    completion_symbol_sort_group(def, &uri, current_container.as_deref());
                let kind = completion_kind_for_symbol_kind(def.def.kind);
                let detail = if def.def.detail.is_empty() {
                    None
                } else {
                    Some(def.def.detail.clone())
                };
                upsert_completion_candidate(
                    &mut candidates,
                    &def.def.name,
                    kind,
                    detail,
                    sort_group,
                );
            }
        }
        for builtin in COMPLETION_BUILTIN_RECEIVERS {
            upsert_completion_candidate(
                &mut candidates,
                builtin,
                9,
                Some("builtin namespace".to_string()),
                3,
            );
        }
        for builtin in COMPLETION_BUILTIN_FUNCTIONS {
            upsert_completion_candidate(
                &mut candidates,
                builtin,
                3,
                Some("builtin function".to_string()),
                3,
            );
        }
        for builtin in COMPLETION_BUILTIN_TYPES {
            upsert_completion_candidate(
                &mut candidates,
                builtin,
                22,
                Some("builtin type".to_string()),
                3,
            );
        }
        for tag in fusec::html_tags::all_html_tags() {
            upsert_completion_candidate(
                &mut candidates,
                tag,
                3,
                Some("html tag builtin".to_string()),
                3,
            );
        }
        for keyword in COMPLETION_KEYWORDS {
            upsert_completion_candidate(&mut candidates, keyword, 14, None, 4);
        }
        for literal in ["true", "false", "null"] {
            upsert_completion_candidate(&mut candidates, literal, 14, None, 4);
        }
    }

    let mut entries: Vec<(String, CompletionCandidate)> = candidates
        .into_iter()
        .filter(|(label, _)| completion_label_matches(label, prefix))
        .collect();
    entries.sort_by(|(left_label, left), (right_label, right)| {
        left.sort_group
            .cmp(&right.sort_group)
            .then_with(|| left_label.to_lowercase().cmp(&right_label.to_lowercase()))
            .then_with(|| left_label.cmp(right_label))
    });
    if entries.len() > 256 {
        entries.truncate(256);
    }
    let items = entries
        .into_iter()
        .map(|(label, candidate)| {
            completion_item_json(
                &label,
                candidate.kind,
                candidate.detail.as_deref(),
                candidate.sort_group,
            )
        })
        .collect();
    completion_list_json(items)
}

fn completion_item_json(label: &str, kind: u32, detail: Option<&str>, sort_group: u8) -> JsonValue {
    let mut item = BTreeMap::new();
    item.insert("label".to_string(), JsonValue::String(label.to_string()));
    item.insert("kind".to_string(), JsonValue::Number(kind as f64));
    if let Some(detail) = detail {
        item.insert("detail".to_string(), JsonValue::String(detail.to_string()));
    }
    item.insert(
        "sortText".to_string(),
        JsonValue::String(format!("{sort_group:02}_{}", label.to_lowercase())),
    );
    JsonValue::Object(item)
}

fn completion_list_json(items: Vec<JsonValue>) -> JsonValue {
    let mut out = BTreeMap::new();
    out.insert("isIncomplete".to_string(), JsonValue::Bool(false));
    out.insert("items".to_string(), JsonValue::Array(items));
    JsonValue::Object(out)
}

fn upsert_completion_candidate(
    candidates: &mut HashMap<String, CompletionCandidate>,
    label: &str,
    kind: u32,
    detail: Option<String>,
    sort_group: u8,
) {
    use std::collections::hash_map::Entry;
    match candidates.entry(label.to_string()) {
        Entry::Vacant(slot) => {
            slot.insert(CompletionCandidate {
                kind,
                detail,
                sort_group,
            });
        }
        Entry::Occupied(mut slot) => {
            let existing = slot.get();
            if sort_group < existing.sort_group {
                slot.insert(CompletionCandidate {
                    kind,
                    detail,
                    sort_group,
                });
                return;
            }
            if sort_group == existing.sort_group && existing.detail.is_none() && detail.is_some() {
                let existing = slot.get_mut();
                existing.detail = detail;
                existing.kind = kind;
            }
        }
    }
}

fn completion_label_matches(label: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let label_lower = label.to_lowercase();
    let prefix_lower = prefix.to_lowercase();
    label_lower.starts_with(&prefix_lower)
}

fn completion_ident_start(text: &str, offset: usize) -> usize {
    let bytes = text.as_bytes();
    let mut pos = offset.min(bytes.len());
    while pos > 0 && is_ident_byte(bytes[pos - 1]) {
        pos -= 1;
    }
    pos
}

fn completion_member_base(text: &str, prefix_start: usize) -> Option<String> {
    if prefix_start == 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let dot = prefix_start - 1;
    if bytes.get(dot).copied() != Some(b'.') {
        return None;
    }
    let mut end = dot;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    completion_receiver_base(text, end)
}

fn is_ident_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn completion_receiver_base(text: &str, end: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let end = completion_skip_ws_left(bytes, end);
    if end == 0 {
        return None;
    }
    let tail = bytes[end - 1];

    if is_ident_byte(tail) {
        let mut start = end;
        while start > 0 && is_ident_byte(bytes[start - 1]) {
            start -= 1;
        }
        if start == end {
            return None;
        }
        let ident = text[start..end].to_string();
        let left = completion_skip_ws_left(bytes, start);
        if left > 0 && bytes[left - 1] == b'.' {
            return completion_receiver_base(text, left - 1);
        }
        return Some(ident);
    }

    if tail == b')' {
        let open = completion_find_matching_left(bytes, end - 1, b'(', b')')?;
        let callee_end = completion_skip_ws_left(bytes, open);
        if callee_end == 0 {
            return None;
        }
        let mut callee_start = callee_end;
        while callee_start > 0 && is_ident_byte(bytes[callee_start - 1]) {
            callee_start -= 1;
        }
        if callee_start == callee_end {
            return None;
        }
        let left = completion_skip_ws_left(bytes, callee_start);
        if left == 0 || bytes[left - 1] != b'.' {
            return None;
        }
        return completion_receiver_base(text, left - 1);
    }

    if tail == b']' {
        let open = completion_find_matching_left(bytes, end - 1, b'[', b']')?;
        return completion_receiver_base(text, open);
    }

    None
}

fn completion_skip_ws_left(bytes: &[u8], mut pos: usize) -> usize {
    while pos > 0 && bytes[pos - 1].is_ascii_whitespace() {
        pos -= 1;
    }
    pos
}

fn completion_find_matching_left(
    bytes: &[u8],
    close_idx: usize,
    open_char: u8,
    close_char: u8,
) -> Option<usize> {
    let mut depth = 0usize;
    let mut idx = close_idx + 1;
    while idx > 0 {
        idx -= 1;
        let ch = bytes[idx];
        if ch == close_char {
            depth += 1;
            continue;
        }
        if ch == open_char {
            if depth == 0 {
                return None;
            }
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn completion_container_name(index: &WorkspaceIndex, uri: &str, offset: usize) -> Option<String> {
    let mut best: Option<(&WorkspaceDef, usize)> = None;
    for def in &index.defs {
        if def.uri != uri || !is_callable_def_kind(def.def.kind) {
            continue;
        }
        if !span_contains(def.def.span, offset) {
            continue;
        }
        let size = def.def.span.end.saturating_sub(def.def.span.start);
        if best.map_or(true, |(_, best_size)| size < best_size) {
            best = Some((def, size));
        }
    }
    best.map(|(def, _)| def.def.name.clone())
}

fn completion_callable_name_at_cursor(program: &Program, cursor: usize) -> Option<String> {
    let mut best: Option<(String, usize)> = None;

    for item in &program.items {
        match item {
            Item::Fn(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.name.clone(), size));
                    }
                }
            }
            Item::Service(decl) => {
                for route in &decl.routes {
                    if span_contains(route.body.span, cursor) {
                        let size = route.body.span.end.saturating_sub(route.body.span.start);
                        if best
                            .as_ref()
                            .map_or(true, |(_, best_size)| size < *best_size)
                        {
                            best = Some((decl.name.name.clone(), size));
                        }
                    }
                }
            }
            Item::App(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.value.clone(), size));
                    }
                }
            }
            Item::Migration(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.clone(), size));
                    }
                }
            }
            Item::Test(decl) => {
                if span_contains(decl.body.span, cursor) {
                    let size = decl.body.span.end.saturating_sub(decl.body.span.start);
                    if best
                        .as_ref()
                        .map_or(true, |(_, best_size)| size < *best_size)
                    {
                        best = Some((decl.name.value.clone(), size));
                    }
                }
            }
            Item::Import(_) | Item::Type(_) | Item::Enum(_) | Item::Config(_) => {}
        }
    }

    best.map(|(name, _)| name)
}

fn completion_symbol_sort_group(def: &WorkspaceDef, uri: &str, container: Option<&str>) -> u8 {
    if def.uri != uri {
        return 2;
    }
    if is_lexical_local_symbol(&def.def) {
        if container.is_some_and(|name| def.def.container.as_deref() == Some(name)) {
            return 0;
        }
        return 1;
    }
    1
}

fn is_lexical_local_symbol(def: &SymbolDef) -> bool {
    if !matches!(def.kind, SymbolKind::Param | SymbolKind::Variable) {
        return false;
    }
    def.detail.starts_with("param ")
        || def.detail.starts_with("let ")
        || def.detail.starts_with("var ")
}

fn completion_kind_for_symbol_kind(kind: SymbolKind) -> u32 {
    match kind {
        SymbolKind::Module => 9,
        SymbolKind::Type | SymbolKind::Config => 22,
        SymbolKind::Enum => 13,
        SymbolKind::EnumVariant => 20,
        SymbolKind::Function
        | SymbolKind::Service
        | SymbolKind::App
        | SymbolKind::Migration
        | SymbolKind::Test => 3,
        SymbolKind::Param | SymbolKind::Variable => 6,
        SymbolKind::Field => 5,
    }
}

fn builtin_receiver_methods(receiver: &str) -> &'static [&'static str] {
    match receiver {
        "db" => &[
            "exec",
            "query",
            "one",
            "from",
            "select",
            "where",
            "all",
            "first",
            "limit",
            "offset",
            "order_by",
            "insert",
            "update",
            "delete",
            "set",
            "join",
            "left_join",
            "right_join",
            "group_by",
            "having",
            "count",
        ],
        "json" => &["encode", "decode"],
        "html" => &["text", "raw", "node", "render"],
        "svg" => &["inline"],
        _ => &[],
    }
}

fn handle_workspace_symbol(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let query = extract_workspace_query(obj)
        .unwrap_or_default()
        .to_lowercase();
    let mut symbols = Vec::new();
    let index = match build_workspace_index_cached(state, "") {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    for def in &index.defs {
        if !query.is_empty() && !def.def.name.to_lowercase().contains(&query) {
            continue;
        }
        let Some(file_idx) = index.file_by_uri.get(&def.uri) else {
            continue;
        };
        let file = &index.files[*file_idx];
        let symbol = symbol_info_json(&def.uri, &file.text, &def.def);
        symbols.push(symbol);
    }
    JsonValue::Array(symbols)
}

fn handle_rename(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let Some(new_name) = extract_new_name(obj) else {
        return JsonValue::Null;
    };
    if !is_valid_ident(&new_name) || is_keyword_or_literal(&new_name) {
        return JsonValue::Null;
    }
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    if !is_renamable_symbol_kind(def.def.kind) {
        return JsonValue::Null;
    }
    let edits = index.rename_edits(def.id, &new_name);
    if edits.is_empty() {
        return JsonValue::Null;
    }
    let mut changes = BTreeMap::new();
    for (uri, edits) in edits {
        changes.insert(uri, JsonValue::Array(edits));
    }
    let mut root = BTreeMap::new();
    root.insert("changes".to_string(), JsonValue::Object(changes));
    JsonValue::Object(root)
}

fn handle_prepare_rename(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    if !is_renamable_symbol_kind(def.def.kind) {
        return JsonValue::Null;
    }
    let Some(text) = index.file_text(&uri) else {
        return JsonValue::Null;
    };
    let offsets = line_offsets(text);
    let offset = line_col_to_offset(text, &offsets, line, character);
    let Some(span) = rename_span_at_position(index, &uri, offset, def.id) else {
        return JsonValue::Null;
    };

    let mut out = BTreeMap::new();
    out.insert("range".to_string(), span_range_json(text, span));
    out.insert(
        "placeholder".to_string(),
        JsonValue::String(def.def.name.clone()),
    );
    JsonValue::Object(out)
}

fn rename_span_at_position(
    index: &WorkspaceIndex,
    uri: &str,
    offset: usize,
    def_id: usize,
) -> Option<Span> {
    let mut best_ref: Option<(Span, usize)> = None;
    for reference in &index.refs {
        if reference.uri != uri || reference.target != def_id {
            continue;
        }
        if !span_contains(reference.span, offset) {
            continue;
        }
        let size = reference.span.end.saturating_sub(reference.span.start);
        if best_ref.map_or(true, |(_, best_size)| size < best_size) {
            best_ref = Some((reference.span, size));
        }
    }
    if let Some((span, _)) = best_ref {
        return Some(span);
    }
    let def = index.def_for_target(def_id)?;
    if def.uri == uri && span_contains(def.def.span, offset) {
        return Some(def.def.span);
    }
    None
}

fn handle_code_action(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Array(Vec::new());
    };
    let text = state
        .docs
        .get(&uri)
        .cloned()
        .or_else(|| uri_to_path(&uri).and_then(|path| std::fs::read_to_string(path).ok()));
    let Some(text) = text else {
        return JsonValue::Array(Vec::new());
    };
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    let (program, _) = parse_source(&text);
    let imports = collect_imports(&program);
    let mut actions = Vec::new();
    let mut seen = HashSet::new();

    for diag in extract_code_action_diagnostics(obj, &text) {
        if let Some(symbol) = parse_unknown_symbol_name(&diag.message) {
            if is_valid_ident(&symbol) {
                if let Some(span) = diag.span {
                    for alias in index.alias_modules_for_symbol(&uri, &symbol) {
                        let replacement = format!("{alias}.{symbol}");
                        let edit = workspace_edit_with_single_span(&uri, &text, span, &replacement);
                        let title = format!("Qualify as {alias}.{symbol}");
                        let key = format!("quickfix:{title}");
                        if seen.insert(key) {
                            actions.push(code_action_json(&title, "quickfix", edit));
                        }
                    }
                }

                for module_path in import_candidates_for_symbol(index, &uri, &symbol)
                    .into_iter()
                    .take(8)
                {
                    let Some(edit) =
                        missing_import_workspace_edit(&uri, &text, &imports, &module_path, &symbol)
                    else {
                        continue;
                    };
                    let title = format!("Import {symbol} from {module_path}");
                    let key = format!("quickfix:{title}");
                    if seen.insert(key) {
                        actions.push(code_action_json(&title, "quickfix", edit));
                    }
                }
            }
        }

        if let Some((field, config)) = parse_unknown_field_on_type(&diag.message) {
            for (title, edit) in missing_config_field_actions(index, &config, &field) {
                let key = format!("quickfix:{title}");
                if seen.insert(key) {
                    actions.push(code_action_json(&title, "quickfix", edit));
                }
            }
        }
    }

    if let Some(edit) = organize_imports_workspace_edit(&uri, &text, &imports) {
        let title = "Organize imports";
        let key = "source:organizeImports".to_string();
        if seen.insert(key) {
            actions.push(code_action_json(title, "source.organizeImports", edit));
        }
    }

    JsonValue::Array(actions)
}

fn handle_semantic_tokens(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Null;
    };
    semantic_tokens_for_text(state, &uri, &text, None)
}

fn handle_semantic_tokens_range(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Null;
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Null;
    };
    let range = extract_lsp_range(obj, &text);
    semantic_tokens_for_text(state, &uri, &text, range)
}

fn semantic_tokens_for_text(
    state: &mut LspState,
    uri: &str,
    text: &str,
    range: Option<Span>,
) -> JsonValue {
    let mut symbol_types: HashMap<(usize, usize), usize> = HashMap::new();
    if let Some(index) = build_workspace_index_cached(state, uri) {
        for def in &index.defs {
            if def.uri != uri {
                continue;
            }
            if let Some(token_type) = semantic_type_for_symbol_kind(def.def.kind) {
                symbol_types.insert((def.def.span.start, def.def.span.end), token_type);
            }
        }
        for reference in &index.refs {
            if reference.uri != uri {
                continue;
            }
            let Some(def) = index.defs.get(reference.target) else {
                continue;
            };
            let Some(token_type) = semantic_type_for_symbol_kind(def.def.kind) else {
                continue;
            };
            symbol_types.insert((reference.span.start, reference.span.end), token_type);
        }
    } else {
        let (program, _) = parse_source(text);
        let index = build_index_with_program(text, &program);
        for def in &index.defs {
            if let Some(token_type) = semantic_type_for_symbol_kind(def.kind) {
                symbol_types.insert((def.span.start, def.span.end), token_type);
            }
        }
        for reference in &index.refs {
            let Some(def) = index.defs.get(reference.target) else {
                continue;
            };
            let Some(token_type) = semantic_type_for_symbol_kind(def.kind) else {
                continue;
            };
            symbol_types.insert((reference.span.start, reference.span.end), token_type);
        }
    }

    let mut token_diags = fusec::diag::Diagnostics::default();
    let tokens = fusec::lexer::lex(text, &mut token_diags);
    let mut rows = Vec::new();
    for (idx, token) in tokens.iter().enumerate() {
        let token_type = match &token.kind {
            fusec::token::TokenKind::Keyword(fusec::token::Keyword::From) => {
                semantic_member_token_type(&tokens, idx).or(Some(SEM_KEYWORD))
            }
            fusec::token::TokenKind::Keyword(_) => Some(SEM_KEYWORD),
            fusec::token::TokenKind::String(_) | fusec::token::TokenKind::InterpString(_) => {
                Some(SEM_STRING)
            }
            fusec::token::TokenKind::Int(_) | fusec::token::TokenKind::Float(_) => Some(SEM_NUMBER),
            fusec::token::TokenKind::DocComment(_) => Some(SEM_COMMENT),
            fusec::token::TokenKind::Bool(_) | fusec::token::TokenKind::Null => Some(SEM_KEYWORD),
            fusec::token::TokenKind::Ident(name) => symbol_types
                .get(&(token.span.start, token.span.end))
                .copied()
                .or_else(|| semantic_ident_fallback(&tokens, idx, name))
                .or(Some(SEM_VARIABLE)),
            _ => None,
        };
        let Some(token_type) = token_type else {
            continue;
        };
        if let Some(range) = range {
            if token.span.end < range.start || token.span.start > range.end {
                continue;
            }
        }
        rows.push(SemanticTokenRow {
            span: token.span,
            token_type,
        });
    }
    rows.sort_by_key(|row| (row.span.start, row.span.end, row.token_type));

    let offsets = line_offsets(text);
    let mut data = Vec::new();
    let mut last_line = 0usize;
    let mut last_col = 0usize;
    let mut first = true;
    for row in rows {
        let Some(slice) = text.get(row.span.start..row.span.end) else {
            continue;
        };
        let length = slice.chars().count();
        if length == 0 {
            continue;
        }
        let (line, col) = offset_to_line_col(&offsets, row.span.start);
        let delta_line = if first {
            line
        } else {
            line.saturating_sub(last_line)
        };
        let delta_start = if first || delta_line > 0 {
            col
        } else {
            col.saturating_sub(last_col)
        };
        data.push(JsonValue::Number(delta_line as f64));
        data.push(JsonValue::Number(delta_start as f64));
        data.push(JsonValue::Number(length as f64));
        data.push(JsonValue::Number(row.token_type as f64));
        data.push(JsonValue::Number(0.0));
        first = false;
        last_line = line;
        last_col = col;
    }

    let mut out = BTreeMap::new();
    out.insert("data".to_string(), JsonValue::Array(data));
    JsonValue::Object(out)
}

fn semantic_ident_fallback(
    tokens: &[fusec::token::Token],
    idx: usize,
    name: &str,
) -> Option<usize> {
    if let Some(token_type) = semantic_std_error_token_type(tokens, idx) {
        return Some(token_type);
    }
    if let Some(token_type) = semantic_member_token_type(tokens, idx) {
        return Some(token_type);
    }
    if is_builtin_receiver(name)
        && matches!(
            next_non_layout_token(tokens, idx).map(|token| &token.kind),
            Some(fusec::token::TokenKind::Punct(fusec::token::Punct::Dot))
        )
    {
        return Some(SEM_NAMESPACE);
    }
    if is_builtin_function_name(name)
        && matches!(
            next_non_layout_token(tokens, idx).map(|token| &token.kind),
            Some(fusec::token::TokenKind::Punct(fusec::token::Punct::LParen))
        )
    {
        return Some(SEM_FUNCTION);
    }
    if is_builtin_type_name(name) && is_type_context(tokens, idx) {
        return Some(SEM_TYPE);
    }
    None
}

fn semantic_std_error_token_type(tokens: &[fusec::token::Token], idx: usize) -> Option<usize> {
    let kind = &tokens.get(idx)?.kind;
    if !matches!(kind, fusec::token::TokenKind::Ident(_)) {
        return None;
    }
    let mut start = idx;
    while start >= 2
        && matches!(
            tokens[start - 1].kind,
            fusec::token::TokenKind::Punct(fusec::token::Punct::Dot)
        )
        && matches!(tokens[start - 2].kind, fusec::token::TokenKind::Ident(_))
    {
        start -= 2;
    }
    let first = ident_token_name(&tokens[start].kind)?;
    if first != "std" {
        return None;
    }
    if start + 2 >= tokens.len()
        || !matches!(
            tokens[start + 1].kind,
            fusec::token::TokenKind::Punct(fusec::token::Punct::Dot)
        )
    {
        return None;
    }
    let second = ident_token_name(&tokens[start + 2].kind)?;
    if second != "Error" {
        return None;
    }
    if idx == start {
        Some(SEM_NAMESPACE)
    } else {
        Some(SEM_VARIABLE)
    }
}

fn ident_token_name(kind: &fusec::token::TokenKind) -> Option<&str> {
    match kind {
        fusec::token::TokenKind::Ident(name) => Some(name.as_str()),
        _ => None,
    }
}

fn semantic_member_token_type(tokens: &[fusec::token::Token], idx: usize) -> Option<usize> {
    let prev = prev_non_layout_token(tokens, idx)?;
    if !matches!(
        prev.kind,
        fusec::token::TokenKind::Punct(fusec::token::Punct::Dot)
    ) {
        return None;
    }
    if matches!(
        next_non_layout_token(tokens, idx).map(|token| &token.kind),
        Some(fusec::token::TokenKind::Punct(fusec::token::Punct::LParen))
    ) {
        Some(SEM_FUNCTION)
    } else {
        Some(SEM_PROPERTY)
    }
}

fn prev_non_layout_token(
    tokens: &[fusec::token::Token],
    idx: usize,
) -> Option<&fusec::token::Token> {
    let mut i = idx;
    while i > 0 {
        i -= 1;
        if !is_layout_token(&tokens[i].kind) {
            return Some(&tokens[i]);
        }
    }
    None
}

fn next_non_layout_token(
    tokens: &[fusec::token::Token],
    idx: usize,
) -> Option<&fusec::token::Token> {
    let mut i = idx + 1;
    while i < tokens.len() {
        if !is_layout_token(&tokens[i].kind) {
            return Some(&tokens[i]);
        }
        i += 1;
    }
    None
}

fn is_layout_token(kind: &fusec::token::TokenKind) -> bool {
    matches!(
        kind,
        fusec::token::TokenKind::Indent
            | fusec::token::TokenKind::Dedent
            | fusec::token::TokenKind::Newline
            | fusec::token::TokenKind::Eof
    )
}

fn is_type_context(tokens: &[fusec::token::Token], idx: usize) -> bool {
    matches!(
        prev_non_layout_token(tokens, idx).map(|token| &token.kind),
        Some(fusec::token::TokenKind::Punct(
            fusec::token::Punct::Colon
                | fusec::token::Punct::Lt
                | fusec::token::Punct::Comma
                | fusec::token::Punct::Arrow
                | fusec::token::Punct::Question
                | fusec::token::Punct::Bang
        ))
    )
}

fn is_builtin_receiver(name: &str) -> bool {
    matches!(name, "db" | "json" | "html" | "svg")
}

fn is_builtin_function_name(name: &str) -> bool {
    matches!(name, "print" | "env" | "serve" | "log" | "assert" | "asset")
        || fusec::html_tags::is_html_tag(name)
}

fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "Unit"
            | "Int"
            | "Float"
            | "Bool"
            | "String"
            | "Bytes"
            | "Html"
            | "Id"
            | "Email"
            | "Error"
            | "List"
            | "Map"
            | "Task"
            | "Range"
    )
}

fn handle_inlay_hints(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some(uri) = extract_text_doc_uri(obj) else {
        return JsonValue::Array(Vec::new());
    };
    let Some(text) = load_text_for_uri(state, &uri) else {
        return JsonValue::Array(Vec::new());
    };
    let range = extract_lsp_range(obj, &text);
    let (program, parse_diags) = parse_source(&text);
    if parse_diags
        .iter()
        .any(|diag| matches!(diag.level, Level::Error))
    {
        return JsonValue::Array(Vec::new());
    }
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Array(Vec::new()),
    };
    let offsets = line_offsets(&text);
    let mut hints = Vec::new();
    let mut seen = HashSet::new();
    for item in &program.items {
        match item {
            Item::Fn(decl) => collect_inlay_hints_block(
                index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Service(decl) => {
                for route in &decl.routes {
                    collect_inlay_hints_block(
                        index,
                        &uri,
                        &text,
                        &offsets,
                        &route.body,
                        range,
                        &mut hints,
                        &mut seen,
                    );
                }
            }
            Item::App(decl) => collect_inlay_hints_block(
                index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Migration(decl) => collect_inlay_hints_block(
                index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            Item::Test(decl) => collect_inlay_hints_block(
                index, &uri, &text, &offsets, &decl.body, range, &mut hints, &mut seen,
            ),
            _ => {}
        }
    }
    JsonValue::Array(hints)
}

fn semantic_type_for_symbol_kind(kind: SymbolKind) -> Option<usize> {
    match kind {
        SymbolKind::Module => Some(SEM_NAMESPACE),
        SymbolKind::Type | SymbolKind::Config => Some(SEM_TYPE),
        SymbolKind::Enum => Some(SEM_ENUM),
        SymbolKind::EnumVariant => Some(SEM_ENUM_MEMBER),
        SymbolKind::Function
        | SymbolKind::Service
        | SymbolKind::App
        | SymbolKind::Migration
        | SymbolKind::Test => Some(SEM_FUNCTION),
        SymbolKind::Param => Some(SEM_PARAMETER),
        SymbolKind::Variable => Some(SEM_VARIABLE),
        SymbolKind::Field => Some(SEM_PROPERTY),
    }
}

fn load_text_for_uri(state: &LspState, uri: &str) -> Option<String> {
    state
        .docs
        .get(uri)
        .cloned()
        .or_else(|| uri_to_path(uri).and_then(|path| std::fs::read_to_string(path).ok()))
}

fn extract_lsp_range(obj: &BTreeMap<String, JsonValue>, text: &str) -> Option<Span> {
    let Some(JsonValue::Object(params)) = obj.get("params") else {
        return None;
    };
    let range = params.get("range")?;
    let offsets = line_offsets(text);
    lsp_range_to_span(range, text, &offsets)
}

fn collect_inlay_hints_block(
    index: &WorkspaceIndex,
    uri: &str,
    text: &str,
    offsets: &[usize],
    block: &Block,
    range: Option<Span>,
    hints: &mut Vec<JsonValue>,
    seen: &mut HashSet<(usize, String)>,
) {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Let { name, ty, expr } | StmtKind::Var { name, ty, expr } => {
                if ty.is_none() {
                    if let Some(ty_name) = infer_expr_type(index, uri, text, expr) {
                        let label = format!(": {ty_name}");
                        push_inlay_hint(
                            offsets,
                            name.span.end,
                            &label,
                            1,
                            false,
                            range,
                            hints,
                            seen,
                        );
                    }
                }
                collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
            }
            StmtKind::Assign { target, expr } => {
                collect_inlay_hints_expr(index, uri, text, offsets, target, range, hints, seen);
                collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
            }
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
                }
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                collect_inlay_hints_expr(index, uri, text, offsets, cond, range, hints, seen);
                collect_inlay_hints_block(
                    index, uri, text, offsets, then_block, range, hints, seen,
                );
                for (else_cond, else_block) in else_if {
                    collect_inlay_hints_expr(
                        index, uri, text, offsets, else_cond, range, hints, seen,
                    );
                    collect_inlay_hints_block(
                        index, uri, text, offsets, else_block, range, hints, seen,
                    );
                }
                if let Some(else_block) = else_block {
                    collect_inlay_hints_block(
                        index, uri, text, offsets, else_block, range, hints, seen,
                    );
                }
            }
            StmtKind::Match { expr, cases } => {
                collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
                for (_, case_block) in cases {
                    collect_inlay_hints_block(
                        index, uri, text, offsets, case_block, range, hints, seen,
                    );
                }
            }
            StmtKind::For { iter, block, .. } => {
                collect_inlay_hints_expr(index, uri, text, offsets, iter, range, hints, seen);
                collect_inlay_hints_block(index, uri, text, offsets, block, range, hints, seen);
            }
            StmtKind::While { cond, block } => {
                collect_inlay_hints_expr(index, uri, text, offsets, cond, range, hints, seen);
                collect_inlay_hints_block(index, uri, text, offsets, block, range, hints, seen);
            }
            StmtKind::Expr(expr) => {
                collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
}

fn collect_inlay_hints_expr(
    index: &WorkspaceIndex,
    uri: &str,
    text: &str,
    offsets: &[usize],
    expr: &Expr,
    range: Option<Span>,
    hints: &mut Vec<JsonValue>,
    seen: &mut HashSet<(usize, String)>,
) {
    match &expr.kind {
        ExprKind::Call { callee, args } => {
            if let Some(param_names) = call_param_names(index, uri, text, callee) {
                for (idx, arg) in args.iter().enumerate() {
                    if arg.name.is_none() {
                        if let Some(param_name) = param_names.get(idx) {
                            if !param_name.is_empty() {
                                let label = format!("{param_name}: ");
                                push_inlay_hint(
                                    offsets,
                                    arg.value.span.start,
                                    &label,
                                    2,
                                    true,
                                    range,
                                    hints,
                                    seen,
                                );
                            }
                        }
                    }
                    collect_inlay_hints_expr(
                        index, uri, text, offsets, &arg.value, range, hints, seen,
                    );
                }
            } else {
                for arg in args {
                    collect_inlay_hints_expr(
                        index, uri, text, offsets, &arg.value, range, hints, seen,
                    );
                }
            }
            collect_inlay_hints_expr(index, uri, text, offsets, callee, range, hints, seen);
        }
        ExprKind::Binary { left, right, .. } => {
            collect_inlay_hints_expr(index, uri, text, offsets, left, range, hints, seen);
            collect_inlay_hints_expr(index, uri, text, offsets, right, range, hints, seen);
        }
        ExprKind::Unary { expr, .. } | ExprKind::Await { expr } | ExprKind::Box { expr } => {
            collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            collect_inlay_hints_expr(index, uri, text, offsets, base, range, hints, seen);
        }
        ExprKind::Index {
            base,
            index: index_expr,
        }
        | ExprKind::OptionalIndex {
            base,
            index: index_expr,
        } => {
            collect_inlay_hints_expr(index, uri, text, offsets, base, range, hints, seen);
            collect_inlay_hints_expr(index, uri, text, offsets, index_expr, range, hints, seen);
        }
        ExprKind::StructLit { fields, .. } => {
            for field in fields {
                collect_inlay_hints_expr(
                    index,
                    uri,
                    text,
                    offsets,
                    &field.value,
                    range,
                    hints,
                    seen,
                );
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                collect_inlay_hints_expr(index, uri, text, offsets, item, range, hints, seen);
            }
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                collect_inlay_hints_expr(index, uri, text, offsets, key, range, hints, seen);
                collect_inlay_hints_expr(index, uri, text, offsets, value, range, hints, seen);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(expr) = part {
                    collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
                }
            }
        }
        ExprKind::Coalesce { left, right } => {
            collect_inlay_hints_expr(index, uri, text, offsets, left, range, hints, seen);
            collect_inlay_hints_expr(index, uri, text, offsets, right, range, hints, seen);
        }
        ExprKind::BangChain { expr, error } => {
            collect_inlay_hints_expr(index, uri, text, offsets, expr, range, hints, seen);
            if let Some(error) = error {
                collect_inlay_hints_expr(index, uri, text, offsets, error, range, hints, seen);
            }
        }
        ExprKind::Spawn { block } => {
            collect_inlay_hints_block(index, uri, text, offsets, block, range, hints, seen);
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
    }
}

fn push_inlay_hint(
    offsets: &[usize],
    offset: usize,
    label: &str,
    kind: u32,
    padding_right: bool,
    range: Option<Span>,
    hints: &mut Vec<JsonValue>,
    seen: &mut HashSet<(usize, String)>,
) {
    if let Some(range) = range {
        if offset < range.start || offset > range.end {
            return;
        }
    }
    let key = (offset, label.to_string());
    if !seen.insert(key) {
        return;
    }
    let (line, col) = offset_to_line_col(offsets, offset);
    let mut position = BTreeMap::new();
    position.insert("line".to_string(), JsonValue::Number(line as f64));
    position.insert("character".to_string(), JsonValue::Number(col as f64));
    let mut out = BTreeMap::new();
    out.insert("position".to_string(), JsonValue::Object(position));
    out.insert("label".to_string(), JsonValue::String(label.to_string()));
    out.insert("kind".to_string(), JsonValue::Number(kind as f64));
    if padding_right {
        out.insert("paddingRight".to_string(), JsonValue::Bool(true));
    }
    hints.push(JsonValue::Object(out));
}

fn call_param_names(
    index: &WorkspaceIndex,
    uri: &str,
    text: &str,
    callee: &Expr,
) -> Option<Vec<String>> {
    let span = match &callee.kind {
        ExprKind::Ident(ident) => ident.span,
        ExprKind::Member { name, .. } | ExprKind::OptionalMember { name, .. } => name.span,
        _ => callee.span,
    };
    let offsets = line_offsets(text);
    let (line, col) = offset_to_line_col(&offsets, span.start);
    let def = index.definition_at(uri, line, col)?;
    if def.def.kind != SymbolKind::Function {
        return None;
    }
    parse_fn_param_names(&def.def.detail)
}

fn parse_fn_param_names(detail: &str) -> Option<Vec<String>> {
    let params = parse_fn_parameter_labels(detail)?;
    let mut names = Vec::new();
    for part in params {
        let name = part.split(':').next().unwrap_or("").trim();
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    Some(names)
}

fn parse_fn_parameter_labels(detail: &str) -> Option<Vec<String>> {
    if !detail.starts_with("fn ") {
        return None;
    }
    let open = detail.find('(')?;
    let close = find_matching_paren(detail, open)?;
    if close <= open {
        return Some(Vec::new());
    }
    let params_text = detail[open + 1..close].trim();
    if params_text.is_empty() {
        return Some(Vec::new());
    }
    Some(split_top_level_csv(params_text))
}

fn find_matching_paren(text: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, ch) in text[open_idx..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(open_idx + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_csv(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (idx, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            ',' if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && angle_depth == 0 =>
            {
                let part = text[start..idx].trim();
                if !part.is_empty() {
                    out.push(part.to_string());
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }

    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn parse_fn_return_type(detail: &str) -> Option<String> {
    let (_, ret) = detail.split_once("->")?;
    let ret = ret.trim();
    if ret.is_empty() {
        return None;
    }
    Some(ret.to_string())
}

fn parse_declared_type_from_detail(detail: &str) -> Option<String> {
    if !(detail.starts_with("let ")
        || detail.starts_with("var ")
        || detail.starts_with("param ")
        || detail.starts_with("field "))
    {
        return None;
    }
    let (_, ty) = detail.split_once(':')?;
    let ty = ty.trim();
    if ty.is_empty() {
        return None;
    }
    Some(ty.to_string())
}

fn infer_expr_type(index: &WorkspaceIndex, uri: &str, text: &str, expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(_)) => Some("Int".to_string()),
        ExprKind::Literal(Literal::Float(_)) => Some("Float".to_string()),
        ExprKind::Literal(Literal::Bool(_)) => Some("Bool".to_string()),
        ExprKind::Literal(Literal::String(_)) => Some("String".to_string()),
        ExprKind::Literal(Literal::Null) => Some("Null".to_string()),
        ExprKind::StructLit { name, .. } => Some(name.name.clone()),
        ExprKind::ListLit(_) => Some("List".to_string()),
        ExprKind::MapLit(_) => Some("Map".to_string()),
        ExprKind::InterpString(_) => Some("String".to_string()),
        ExprKind::Spawn { .. } => Some("Task".to_string()),
        ExprKind::Coalesce { left, .. } => infer_expr_type(index, uri, text, left),
        ExprKind::Await { expr } | ExprKind::Box { expr } | ExprKind::BangChain { expr, .. } => {
            infer_expr_type(index, uri, text, expr)
        }
        ExprKind::Ident(ident) => {
            let offsets = line_offsets(text);
            let (line, col) = offset_to_line_col(&offsets, ident.span.start);
            let def = index.definition_at(uri, line, col)?;
            parse_declared_type_from_detail(&def.def.detail)
        }
        ExprKind::Call { callee, .. } => {
            let span = match &callee.kind {
                ExprKind::Ident(ident) => ident.span,
                ExprKind::Member { name, .. } | ExprKind::OptionalMember { name, .. } => name.span,
                _ => callee.span,
            };
            let offsets = line_offsets(text);
            let (line, col) = offset_to_line_col(&offsets, span.start);
            let def = index.definition_at(uri, line, col)?;
            parse_fn_return_type(&def.def.detail)
        }
        ExprKind::Unary { op, expr } => match op {
            UnaryOp::Not => Some("Bool".to_string()),
            UnaryOp::Neg => infer_expr_type(index, uri, text, expr),
        },
        ExprKind::Binary { op, left, right } => match op {
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq
            | BinaryOp::And
            | BinaryOp::Or => Some("Bool".to_string()),
            BinaryOp::Range => Some("List".to_string()),
            BinaryOp::Add => {
                let left_ty = infer_expr_type(index, uri, text, left)?;
                if left_ty == "String" {
                    Some("String".to_string())
                } else {
                    Some(left_ty)
                }
            }
            _ => infer_expr_type(index, uri, text, left)
                .or_else(|| infer_expr_type(index, uri, text, right)),
        },
        ExprKind::Member { .. }
        | ExprKind::OptionalMember { .. }
        | ExprKind::Index { .. }
        | ExprKind::OptionalIndex { .. } => None,
    }
}

#[derive(Clone)]
struct SemanticTokenRow {
    span: Span,
    token_type: usize,
}

fn handle_references(state: &mut LspState, obj: &BTreeMap<String, JsonValue>) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let include_declaration = extract_include_declaration(obj);
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    JsonValue::Array(index.reference_locations(def.id, include_declaration))
}

fn handle_prepare_call_hierarchy(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some((uri, line, character)) = extract_position(obj) else {
        return JsonValue::Null;
    };
    let index = match build_workspace_index_cached(state, &uri) {
        Some(index) => index,
        None => return JsonValue::Null,
    };
    let Some(def) = index.definition_at(&uri, line, character) else {
        return JsonValue::Null;
    };
    if !is_callable_def_kind(def.def.kind) {
        return JsonValue::Null;
    }
    let Some(item) = call_hierarchy_item_json(index, &def) else {
        return JsonValue::Null;
    };
    JsonValue::Array(vec![item])
}

fn handle_call_hierarchy_incoming(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some(index) = build_workspace_index_for_call_hierarchy(state, obj) else {
        return JsonValue::Null;
    };
    let Some(def_id) = call_hierarchy_target_def_id(index, obj) else {
        return JsonValue::Null;
    };
    let mut result = Vec::new();
    for (from_id, sites) in index.incoming_calls(def_id) {
        let Some(from_def) = index.def_for_target(from_id) else {
            continue;
        };
        let Some(from_item) = call_hierarchy_item_json(index, &from_def) else {
            continue;
        };
        let mut ranges = Vec::new();
        let mut seen = HashSet::new();
        for site in sites {
            if !seen.insert((site.span.start, site.span.end)) {
                continue;
            }
            if let Some(range) = index.span_range_json(&site.uri, site.span) {
                ranges.push(range);
            }
        }
        let mut item = BTreeMap::new();
        item.insert("from".to_string(), from_item);
        item.insert("fromRanges".to_string(), JsonValue::Array(ranges));
        result.push(JsonValue::Object(item));
    }
    JsonValue::Array(result)
}

fn collect_imports(program: &Program) -> Vec<ImportDecl> {
    let mut imports = Vec::new();
    for item in &program.items {
        if let Item::Import(decl) = item {
            imports.push(decl.clone());
        }
    }
    imports.sort_by_key(|decl| decl.span.start);
    imports
}

fn parse_unknown_symbol_name(message: &str) -> Option<String> {
    for prefix in ["unknown identifier ", "unknown type "] {
        if let Some(rest) = message.strip_prefix(prefix) {
            let symbol = rest.trim();
            if !symbol.is_empty() {
                return Some(symbol.to_string());
            }
        }
    }
    None
}

fn parse_unknown_field_on_type(message: &str) -> Option<(String, String)> {
    let rest = message.strip_prefix("unknown field ")?;
    let (field, ty) = rest.split_once(" on ")?;
    let field = field.trim();
    let ty = ty.trim();
    if !is_valid_ident(field) || !is_valid_ident(ty) {
        return None;
    }
    Some((field.to_string(), ty.to_string()))
}

fn missing_config_field_actions(
    index: &WorkspaceIndex,
    config_name: &str,
    field_name: &str,
) -> Vec<(String, JsonValue)> {
    let mut defs = config_defs_named(index, config_name);
    defs.sort_by(|a, b| {
        a.uri
            .cmp(&b.uri)
            .then(a.def.span.start.cmp(&b.def.span.start))
    });
    defs.dedup_by(|a, b| a.uri == b.uri && a.def.span.start == b.def.span.start);

    let multiple = defs.len() > 1;
    let mut out = Vec::new();
    for def in defs {
        let Some(text) = index.file_text(&def.uri) else {
            continue;
        };
        let Some(edit) =
            missing_config_field_workspace_edit(&def.uri, text, config_name, field_name)
        else {
            continue;
        };
        let title = if multiple {
            let location = path_display_for_uri(&def.uri);
            format!("Add {config_name}.{field_name} to config in {location}")
        } else {
            format!("Add {config_name}.{field_name} to config")
        };
        out.push((title, edit));
    }
    out
}

fn config_defs_named(index: &WorkspaceIndex, config_name: &str) -> Vec<WorkspaceDef> {
    index
        .defs
        .iter()
        .filter(|def| def.def.kind == SymbolKind::Config && def.def.name == config_name)
        .cloned()
        .collect()
}

fn missing_config_field_workspace_edit(
    uri: &str,
    text: &str,
    config_name: &str,
    field_name: &str,
) -> Option<JsonValue> {
    let (program, parse_diags) = parse_source(text);
    if parse_diags
        .iter()
        .any(|diag| matches!(diag.level, Level::Error))
    {
        return None;
    }
    let config = program.items.iter().find_map(|item| match item {
        Item::Config(decl) if decl.name.name == config_name => Some(decl),
        _ => None,
    })?;
    if config
        .fields
        .iter()
        .any(|field| field.name.name == field_name)
    {
        return None;
    }

    let insert_offset = config_field_insert_offset(text, config);
    let indent = config_field_indent(text, config);
    let new_text = format!("\n{indent}{field_name}: String = \"\"");
    Some(workspace_edit_with_single_span(
        uri,
        text,
        Span::new(insert_offset, insert_offset),
        &new_text,
    ))
}

fn config_field_insert_offset(text: &str, config: &ConfigDecl) -> usize {
    if let Some(last_field) = config.fields.last() {
        return line_end_offset(text, last_field.span.end);
    }
    line_end_offset(text, config.name.span.end)
}

fn config_field_indent(text: &str, config: &ConfigDecl) -> String {
    if let Some(first_field) = config.fields.first() {
        return line_indent_at(text, first_field.span.start);
    }
    let base = line_indent_at(text, config.span.start);
    format!("{base}  ")
}

fn line_indent_at(text: &str, offset: usize) -> String {
    let line_start = line_start_offset(text, offset);
    let mut idx = line_start;
    let bytes = text.as_bytes();
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    text[line_start..idx].to_string()
}

fn line_start_offset(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[..offset].rfind('\n').map_or(0, |idx| idx + 1)
}

fn line_end_offset(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    match text[offset..].find('\n') {
        Some(rel) => offset + rel,
        None => text.len(),
    }
}

fn path_display_for_uri(uri: &str) -> String {
    uri_to_path(uri)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| uri.to_string())
}

fn import_candidates_for_symbol(index: &WorkspaceIndex, uri: &str, symbol: &str) -> Vec<String> {
    let mut out = Vec::new();
    for def in &index.defs {
        if def.uri == uri {
            continue;
        }
        if def.def.name != symbol {
            continue;
        }
        if !is_exported_def_kind(def.def.kind) {
            continue;
        }
        if let Some(path) = module_import_path_between(uri, &def.uri) {
            out.push(path);
        }
    }
    if is_std_error_symbol(symbol) {
        out.push("std.Error".to_string());
    }
    out.sort();
    out.dedup();
    out
}

fn module_import_path_between(from_uri: &str, to_uri: &str) -> Option<String> {
    let from = uri_to_path(from_uri)?;
    let to = uri_to_path(to_uri)?;
    let from_dir = from.parent()?;
    let to_no_ext = to.with_extension("");

    let mut base = from_dir;
    let mut up_count = 0usize;
    loop {
        if let Ok(rest) = to_no_ext.strip_prefix(base) {
            let rest = rest.to_string_lossy().replace('\\', "/");
            if rest.is_empty() {
                return None;
            }
            if up_count == 0 {
                return Some(format!("./{rest}"));
            }
            return Some(format!("{}{}", "../".repeat(up_count), rest));
        }
        base = base.parent()?;
        up_count += 1;
    }
}

fn is_std_error_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "Error"
            | "ValidationField"
            | "Validation"
            | "BadRequest"
            | "Unauthorized"
            | "Forbidden"
            | "NotFound"
            | "Conflict"
    )
}

fn missing_import_workspace_edit(
    uri: &str,
    text: &str,
    imports: &[ImportDecl],
    module_path: &str,
    symbol: &str,
) -> Option<JsonValue> {
    if let Some(existing) = imports.iter().find(|decl| match &decl.spec {
        ImportSpec::NamedFrom { path, .. } => path.value == module_path,
        _ => false,
    }) {
        let ImportSpec::NamedFrom { names, .. } = &existing.spec else {
            return None;
        };
        let mut merged: Vec<String> = names.iter().map(|ident| ident.name.clone()).collect();
        if merged.iter().any(|name| name == symbol) {
            return None;
        }
        merged.push(symbol.to_string());
        merged.sort();
        merged.dedup();
        let line = render_named_import(module_path, &merged);
        return Some(workspace_edit_with_single_span(
            uri,
            text,
            existing.span,
            &line,
        ));
    }

    if import_already_binds_symbol(imports, symbol) {
        return None;
    }
    let line = render_named_import(module_path, &[symbol.to_string()]);
    let insert_offset = imports.iter().map(|decl| decl.span.end).max().unwrap_or(0);
    let mut new_text = String::new();
    if insert_offset > 0 && !text[..insert_offset].ends_with('\n') {
        new_text.push('\n');
    }
    new_text.push_str(&line);
    new_text.push('\n');
    if insert_offset == 0 && !text.is_empty() {
        new_text.push('\n');
    }
    Some(workspace_edit_with_single_span(
        uri,
        text,
        Span::new(insert_offset, insert_offset),
        &new_text,
    ))
}

fn organize_imports_workspace_edit(
    uri: &str,
    text: &str,
    imports: &[ImportDecl],
) -> Option<JsonValue> {
    if imports.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = imports.iter().map(render_import_decl).collect();
    lines.sort();
    lines.dedup();
    let first = imports.iter().map(|decl| decl.span.start).min()?;
    let mut end = imports.iter().map(|decl| decl.span.end).max()?;
    while end < text.len() && text.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }
    let replacement = if end < text.len() {
        format!("{}\n\n", lines.join("\n"))
    } else {
        format!("{}\n", lines.join("\n"))
    };
    if text.get(first..end) == Some(replacement.as_str()) {
        return None;
    }
    Some(workspace_edit_with_single_span(
        uri,
        text,
        Span::new(first, end),
        &replacement,
    ))
}

fn import_already_binds_symbol(imports: &[ImportDecl], symbol: &str) -> bool {
    for decl in imports {
        match &decl.spec {
            ImportSpec::Module { name } | ImportSpec::ModuleFrom { name, .. } => {
                if name.name == symbol {
                    return true;
                }
            }
            ImportSpec::AliasFrom { alias, .. } => {
                if alias.name == symbol {
                    return true;
                }
            }
            ImportSpec::NamedFrom { names, .. } => {
                if names.iter().any(|name| name.name == symbol) {
                    return true;
                }
            }
        }
    }
    false
}

fn render_import_decl(decl: &ImportDecl) -> String {
    match &decl.spec {
        ImportSpec::Module { name } => format!("import {}", name.name),
        ImportSpec::ModuleFrom { name, path } => {
            format!(
                "import {} from {}",
                name.name,
                render_import_path(&path.value)
            )
        }
        ImportSpec::AliasFrom { name, alias, path } => format!(
            "import {} as {} from {}",
            name.name,
            alias.name,
            render_import_path(&path.value)
        ),
        ImportSpec::NamedFrom { names, path } => {
            let mut symbols: Vec<String> = names.iter().map(|name| name.name.clone()).collect();
            symbols.sort();
            symbols.dedup();
            render_named_import(&path.value, &symbols)
        }
    }
}

fn render_named_import(path: &str, names: &[String]) -> String {
    format!(
        "import {{ {} }} from {}",
        names.join(", "),
        render_import_path(path)
    )
}

fn render_import_path(path: &str) -> String {
    if path.starts_with("./")
        || path.starts_with("../")
        || path.contains('/')
        || path.contains('\\')
    {
        return format!("\"{}\"", path);
    }
    path.to_string()
}

fn workspace_edit_with_single_span(uri: &str, text: &str, span: Span, new_text: &str) -> JsonValue {
    let edit = text_edit_json(text, span, new_text);
    let mut changes = BTreeMap::new();
    changes.insert(uri.to_string(), JsonValue::Array(vec![edit]));
    let mut root = BTreeMap::new();
    root.insert("changes".to_string(), JsonValue::Object(changes));
    JsonValue::Object(root)
}

fn text_edit_json(text: &str, span: Span, new_text: &str) -> JsonValue {
    let offsets = line_offsets(text);
    let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
    let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
    let mut edit = BTreeMap::new();
    edit.insert(
        "range".to_string(),
        range_json(start_line, start_col, end_line, end_col),
    );
    edit.insert(
        "newText".to_string(),
        JsonValue::String(new_text.to_string()),
    );
    JsonValue::Object(edit)
}

fn code_action_json(title: &str, kind: &str, edit: JsonValue) -> JsonValue {
    let mut out = BTreeMap::new();
    out.insert("title".to_string(), JsonValue::String(title.to_string()));
    out.insert("kind".to_string(), JsonValue::String(kind.to_string()));
    out.insert("edit".to_string(), edit);
    JsonValue::Object(out)
}

fn handle_call_hierarchy_outgoing(
    state: &mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let Some(index) = build_workspace_index_for_call_hierarchy(state, obj) else {
        return JsonValue::Null;
    };
    let Some(def_id) = call_hierarchy_target_def_id(index, obj) else {
        return JsonValue::Null;
    };
    let mut result = Vec::new();
    for (to_id, sites) in index.outgoing_calls(def_id) {
        let Some(to_def) = index.def_for_target(to_id) else {
            continue;
        };
        let Some(to_item) = call_hierarchy_item_json(index, &to_def) else {
            continue;
        };
        let mut ranges = Vec::new();
        let mut seen = HashSet::new();
        for site in sites {
            if !seen.insert((site.span.start, site.span.end)) {
                continue;
            }
            if let Some(range) = index.span_range_json(&site.uri, site.span) {
                ranges.push(range);
            }
        }
        let mut item = BTreeMap::new();
        item.insert("to".to_string(), to_item);
        item.insert("fromRanges".to_string(), JsonValue::Array(ranges));
        result.push(JsonValue::Object(item));
    }
    JsonValue::Array(result)
}

fn build_workspace_index_for_call_hierarchy<'a>(
    state: &'a mut LspState,
    obj: &BTreeMap<String, JsonValue>,
) -> Option<&'a WorkspaceIndex> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let item = params.get("item")?;
    let JsonValue::Object(item) = item else {
        return None;
    };
    let uri = match item.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    build_workspace_index_cached(state, &uri)
}

fn call_hierarchy_target_def_id(
    index: &WorkspaceIndex,
    obj: &BTreeMap<String, JsonValue>,
) -> Option<usize> {
    let params = obj.get("params")?;
    let JsonValue::Object(params) = params else {
        return None;
    };
    let item = params.get("item")?;
    let JsonValue::Object(item) = item else {
        return None;
    };
    if let Some(def_id) = item.get("data").and_then(|value| match value {
        JsonValue::Number(num) if *num >= 0.0 => Some(*num as usize),
        _ => None,
    }) {
        return Some(def_id);
    }
    let uri = match item.get("uri") {
        Some(JsonValue::String(uri)) => uri.clone(),
        _ => return None,
    };
    let selection_range = item.get("selectionRange").or_else(|| item.get("range"))?;
    let JsonValue::Object(selection_range) = selection_range else {
        return None;
    };
    let start = selection_range.get("start")?;
    let JsonValue::Object(start) = start else {
        return None;
    };
    let line = match start.get("line") {
        Some(JsonValue::Number(line)) => *line as usize,
        _ => return None,
    };
    let character = match start.get("character") {
        Some(JsonValue::Number(character)) => *character as usize,
        _ => return None,
    };
    let def = index.definition_at(&uri, line, character)?;
    Some(def.id)
}

fn call_hierarchy_item_json(index: &WorkspaceIndex, def: &WorkspaceDef) -> Option<JsonValue> {
    let text = index.file_text(&def.uri)?;
    let range = span_range_json(text, def.def.span);
    let mut out = BTreeMap::new();
    out.insert("name".to_string(), JsonValue::String(def.def.name.clone()));
    out.insert(
        "kind".to_string(),
        JsonValue::Number(def.def.kind.lsp_kind() as f64),
    );
    out.insert("uri".to_string(), JsonValue::String(def.uri.clone()));
    out.insert("range".to_string(), range.clone());
    out.insert("selectionRange".to_string(), range);
    out.insert("data".to_string(), JsonValue::Number(def.id as f64));
    if !def.def.detail.is_empty() {
        out.insert(
            "detail".to_string(),
            JsonValue::String(def.def.detail.clone()),
        );
    }
    Some(JsonValue::Object(out))
}

fn location_json(uri: &str, text: &str, span: Span) -> JsonValue {
    let range = span_range_json(text, span);
    let mut out = BTreeMap::new();
    out.insert("uri".to_string(), JsonValue::String(uri.to_string()));
    out.insert("range".to_string(), range);
    JsonValue::Object(out)
}

fn span_range_json(text: &str, span: Span) -> JsonValue {
    let offsets = line_offsets(text);
    let (start_line, start_col) = offset_to_line_col(&offsets, span.start);
    let (end_line, end_col) = offset_to_line_col(&offsets, span.end);
    range_json(start_line, start_col, end_line, end_col)
}

fn symbol_info_json(uri: &str, text: &str, def: &SymbolDef) -> JsonValue {
    let location = location_json(uri, text, def.span);
    let mut out = BTreeMap::new();
    out.insert("name".to_string(), JsonValue::String(def.name.clone()));
    out.insert(
        "kind".to_string(),
        JsonValue::Number(def.kind.lsp_kind() as f64),
    );
    out.insert("location".to_string(), location);
    if let Some(container) = &def.container {
        out.insert(
            "containerName".to_string(),
            JsonValue::String(container.clone()),
        );
    }
    JsonValue::Object(out)
}

fn is_valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_keyword_or_literal(name: &str) -> bool {
    COMPLETION_KEYWORDS.contains(&name) || matches!(name, "true" | "false" | "null")
}

fn is_renamable_symbol_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Module
            | SymbolKind::Type
            | SymbolKind::Enum
            | SymbolKind::EnumVariant
            | SymbolKind::Function
            | SymbolKind::Config
            | SymbolKind::Param
            | SymbolKind::Variable
            | SymbolKind::Field
    )
}

struct Index {
    defs: Vec<SymbolDef>,
    refs: Vec<SymbolRef>,
    calls: Vec<CallRef>,
    qualified_calls: Vec<QualifiedCallRef>,
}

impl Index {
    fn definition_at(&self, offset: usize) -> Option<usize> {
        if let Some(def_id) = self.reference_at(offset) {
            return Some(def_id);
        }
        self.def_at(offset)
    }

    fn reference_at(&self, offset: usize) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for reference in &self.refs {
            if span_contains(reference.span, offset) {
                let size = reference.span.end.saturating_sub(reference.span.start);
                if best.map_or(true, |(_, best_size)| size < best_size) {
                    best = Some((reference.target, size));
                }
            }
        }
        best.map(|(def_id, _)| def_id)
    }

    fn def_at(&self, offset: usize) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (id, def) in self.defs.iter().enumerate() {
            if span_contains(def.span, offset) {
                let size = def.span.end.saturating_sub(def.span.start);
                if best.map_or(true, |(_, best_size)| size < best_size) {
                    best = Some((id, size));
                }
            }
        }
        best.map(|(id, _)| id)
    }
}

#[derive(Clone)]
struct SymbolDef {
    name: String,
    span: Span,
    kind: SymbolKind,
    detail: String,
    doc: Option<String>,
    container: Option<String>,
}

struct SymbolRef {
    span: Span,
    target: usize,
}

struct CallRef {
    caller: usize,
    callee: usize,
    span: Span,
}

struct QualifiedCallRef {
    caller: usize,
    module: String,
    item: String,
    span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Module,
    Type,
    Enum,
    EnumVariant,
    Function,
    Config,
    Service,
    App,
    Migration,
    Test,
    Param,
    Variable,
    Field,
}

impl SymbolKind {
    fn lsp_kind(self) -> u32 {
        match self {
            SymbolKind::Module => 2,
            SymbolKind::Type => 23,
            SymbolKind::Enum => 10,
            SymbolKind::EnumVariant => 22,
            SymbolKind::Function => 12,
            SymbolKind::Config => 23,
            SymbolKind::Service => 11,
            SymbolKind::App => 5,
            SymbolKind::Migration => 12,
            SymbolKind::Test => 12,
            SymbolKind::Param => 13,
            SymbolKind::Variable => 13,
            SymbolKind::Field => 8,
        }
    }

    fn hover_label(self) -> &'static str {
        match self {
            SymbolKind::Module => "Module",
            SymbolKind::Type => "Type",
            SymbolKind::Enum => "Enum",
            SymbolKind::EnumVariant => "Enum Variant",
            SymbolKind::Function => "Function",
            SymbolKind::Config => "Config",
            SymbolKind::Service => "Service",
            SymbolKind::App => "App",
            SymbolKind::Migration => "Migration",
            SymbolKind::Test => "Test",
            SymbolKind::Param => "Parameter",
            SymbolKind::Variable => "Variable",
            SymbolKind::Field => "Field",
        }
    }
}

fn span_contains(span: Span, offset: usize) -> bool {
    offset >= span.start && offset <= span.end
}

struct WorkspaceIndex {
    files: Vec<WorkspaceFile>,
    file_by_uri: HashMap<String, usize>,
    defs: Vec<WorkspaceDef>,
    refs: Vec<WorkspaceRef>,
    calls: Vec<WorkspaceCall>,
    module_alias_exports: HashMap<String, HashMap<String, HashSet<String>>>,
    redirects: HashMap<usize, usize>,
}

struct WorkspaceFile {
    uri: String,
    text: String,
    index: Index,
    def_map: Vec<usize>,
    qualified_refs: Vec<QualifiedRef>,
}

#[derive(Clone)]
struct WorkspaceDef {
    id: usize,
    uri: String,
    def: SymbolDef,
}

struct WorkspaceRef {
    uri: String,
    span: Span,
    target: usize,
}

#[derive(Clone)]
struct WorkspaceCall {
    uri: String,
    span: Span,
    from: usize,
    to: usize,
}

struct QualifiedRef {
    span: Span,
    target: usize,
}

impl WorkspaceIndex {
    fn definition_at(&self, uri: &str, line: usize, character: usize) -> Option<WorkspaceDef> {
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

    fn rename_edits(&self, def_id: usize, new_name: &str) -> HashMap<String, Vec<JsonValue>> {
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

    fn reference_locations(&self, def_id: usize, include_declaration: bool) -> Vec<JsonValue> {
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

    fn span_range_json(&self, uri: &str, span: Span) -> Option<JsonValue> {
        let text = self.file_text(uri)?;
        Some(span_range_json(text, span))
    }

    fn incoming_calls(&self, target: usize) -> Vec<(usize, Vec<WorkspaceCall>)> {
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

    fn outgoing_calls(&self, source: usize) -> Vec<(usize, Vec<WorkspaceCall>)> {
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

    fn alias_modules_for_symbol(&self, uri: &str, symbol: &str) -> Vec<String> {
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

    fn alias_exports_for_module(&self, uri: &str, alias: &str) -> Vec<String> {
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

    fn def_for_target(&self, target: usize) -> Option<WorkspaceDef> {
        self.defs.get(target).cloned()
    }

    fn file_text(&self, uri: &str) -> Option<&str> {
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

fn build_index_with_program(text: &str, program: &Program) -> Index {
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

fn build_workspace_snapshot_cached<'a>(
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

fn build_workspace_index_cached<'a>(
    state: &'a mut LspState,
    focus_uri: &str,
) -> Option<&'a WorkspaceIndex> {
    let snapshot = build_workspace_snapshot_cached(state, focus_uri)?;
    if snapshot.index.is_none() {
        snapshot.index = build_workspace_from_registry(&snapshot.registry, &snapshot.overrides);
    }
    snapshot.index.as_ref()
}

fn build_workspace_snapshot(state: &LspState, focus_uri: &str) -> Option<WorkspaceSnapshot> {
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

fn build_focus_workspace_snapshot(state: &LspState, focus_uri: &str) -> Option<WorkspaceSnapshot> {
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

fn is_exported_def_kind(kind: SymbolKind) -> bool {
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

fn is_callable_def_kind(kind: SymbolKind) -> bool {
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

struct QualifiedNameRef {
    span: Span,
    module: String,
    item: String,
}

fn collect_qualified_refs(program: &Program) -> Vec<QualifiedNameRef> {
    let mut out = Vec::new();
    for item in &program.items {
        match item {
            Item::Type(decl) => {
                for field in &decl.fields {
                    collect_qualified_type_ref(&field.ty, &mut out);
                }
            }
            Item::Enum(decl) => {
                for variant in &decl.variants {
                    for ty in &variant.payload {
                        collect_qualified_type_ref(ty, &mut out);
                    }
                }
            }
            Item::Fn(decl) => {
                for param in &decl.params {
                    collect_qualified_type_ref(&param.ty, &mut out);
                }
                if let Some(ret) = &decl.ret {
                    collect_qualified_type_ref(ret, &mut out);
                }
                collect_qualified_block(&decl.body, &mut out);
            }
            Item::Service(decl) => {
                for route in &decl.routes {
                    collect_qualified_type_ref(&route.ret_type, &mut out);
                    if let Some(body) = &route.body_type {
                        collect_qualified_type_ref(body, &mut out);
                    }
                    collect_qualified_block(&route.body, &mut out);
                }
            }
            Item::Config(decl) => {
                for field in &decl.fields {
                    collect_qualified_type_ref(&field.ty, &mut out);
                    collect_qualified_expr(&field.value, &mut out);
                }
            }
            Item::App(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Migration(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Test(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Import(_) => {}
        }
    }
    out
}

fn collect_qualified_block(block: &Block, out: &mut Vec<QualifiedNameRef>) {
    for stmt in &block.stmts {
        collect_qualified_stmt(stmt, out);
    }
}

fn collect_qualified_stmt(stmt: &Stmt, out: &mut Vec<QualifiedNameRef>) {
    match &stmt.kind {
        StmtKind::Let { ty, expr, .. } | StmtKind::Var { ty, expr, .. } => {
            if let Some(ty) = ty {
                collect_qualified_type_ref(ty, out);
            }
            collect_qualified_expr(expr, out);
        }
        StmtKind::Assign { target, expr } => {
            collect_qualified_expr(target, out);
            collect_qualified_expr(expr, out);
        }
        StmtKind::Return { expr } => {
            if let Some(expr) = expr {
                collect_qualified_expr(expr, out);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            collect_qualified_expr(cond, out);
            collect_qualified_block(then_block, out);
            for (cond, block) in else_if {
                collect_qualified_expr(cond, out);
                collect_qualified_block(block, out);
            }
            if let Some(block) = else_block {
                collect_qualified_block(block, out);
            }
        }
        StmtKind::Match { expr, cases } => {
            collect_qualified_expr(expr, out);
            for (pat, block) in cases {
                collect_qualified_pattern(pat, out);
                collect_qualified_block(block, out);
            }
        }
        StmtKind::For { pat, iter, block } => {
            collect_qualified_pattern(pat, out);
            collect_qualified_expr(iter, out);
            collect_qualified_block(block, out);
        }
        StmtKind::While { cond, block } => {
            collect_qualified_expr(cond, out);
            collect_qualified_block(block, out);
        }
        StmtKind::Expr(expr) => collect_qualified_expr(expr, out),
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_qualified_expr(expr: &Expr, out: &mut Vec<QualifiedNameRef>) {
    match &expr.kind {
        ExprKind::Literal(_) => {}
        ExprKind::Ident(_) => {}
        ExprKind::Binary { left, right, .. } => {
            collect_qualified_expr(left, out);
            collect_qualified_expr(right, out);
        }
        ExprKind::Unary { expr, .. } => collect_qualified_expr(expr, out),
        ExprKind::Call { callee, args } => {
            collect_qualified_expr(callee, out);
            for arg in args {
                collect_qualified_expr(&arg.value, out);
            }
        }
        ExprKind::Member { base, name } => {
            if let ExprKind::Ident(ident) = &base.kind {
                if let Some((module, item)) =
                    split_qualified_name(&format!("{}.{}", ident.name, name.name))
                {
                    out.push(QualifiedNameRef {
                        span: name.span,
                        module: module.to_string(),
                        item: item.to_string(),
                    });
                }
            }
            collect_qualified_expr(base, out);
        }
        ExprKind::OptionalMember { base, name } => {
            if let ExprKind::Ident(ident) = &base.kind {
                if let Some((module, item)) =
                    split_qualified_name(&format!("{}.{}", ident.name, name.name))
                {
                    out.push(QualifiedNameRef {
                        span: name.span,
                        module: module.to_string(),
                        item: item.to_string(),
                    });
                }
            }
            collect_qualified_expr(base, out);
        }
        ExprKind::StructLit { name, fields } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for field in fields {
                collect_qualified_expr(&field.value, out);
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                collect_qualified_expr(item, out);
            }
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                collect_qualified_expr(key, out);
                collect_qualified_expr(value, out);
            }
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            collect_qualified_expr(base, out);
            collect_qualified_expr(index, out);
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(expr) = part {
                    collect_qualified_expr(expr, out);
                }
            }
        }
        ExprKind::Coalesce { left, right } => {
            collect_qualified_expr(left, out);
            collect_qualified_expr(right, out);
        }
        ExprKind::BangChain { expr, error } => {
            collect_qualified_expr(expr, out);
            if let Some(err) = error {
                collect_qualified_expr(err, out);
            }
        }
        ExprKind::Spawn { block } => collect_qualified_block(block, out),
        ExprKind::Await { expr } => collect_qualified_expr(expr, out),
        ExprKind::Box { expr } => collect_qualified_expr(expr, out),
    }
}

fn collect_qualified_pattern(pattern: &Pattern, out: &mut Vec<QualifiedNameRef>) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => {}
        PatternKind::Ident(_) => {}
        PatternKind::EnumVariant { name, args } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_pattern(arg, out);
            }
        }
        PatternKind::Struct { name, fields } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for field in fields {
                collect_qualified_pattern(&field.pat, out);
            }
        }
    }
}

fn collect_qualified_type_ref(ty: &TypeRef, out: &mut Vec<QualifiedNameRef>) {
    match &ty.kind {
        TypeRefKind::Simple(ident) => {
            if let Some((module, item)) = split_qualified_name(&ident.name) {
                out.push(QualifiedNameRef {
                    span: ident.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
        }
        TypeRefKind::Generic { base, args } => {
            if let Some((module, item)) = split_qualified_name(&base.name) {
                out.push(QualifiedNameRef {
                    span: base.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_type_ref(arg, out);
            }
        }
        TypeRefKind::Optional(inner) => collect_qualified_type_ref(inner, out),
        TypeRefKind::Result { ok, err } => {
            collect_qualified_type_ref(ok, out);
            if let Some(err) = err {
                collect_qualified_type_ref(err, out);
            }
        }
        TypeRefKind::Refined { base, args } => {
            if let Some((module, item)) = split_qualified_name(&base.name) {
                out.push(QualifiedNameRef {
                    span: base.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_expr(arg, out);
            }
        }
    }
}

fn split_qualified_name(name: &str) -> Option<(&str, &str)> {
    let mut iter = name.rsplitn(2, '.');
    let item = iter.next()?;
    let module = iter.next()?;
    if module.is_empty() || item.is_empty() {
        return None;
    }
    Some((module, item))
}

struct IndexBuilder<'a> {
    text: &'a str,
    defs: Vec<SymbolDef>,
    refs: Vec<SymbolRef>,
    calls: Vec<CallRef>,
    qualified_calls: Vec<QualifiedCallRef>,
    scopes: Vec<HashMap<String, usize>>,
    globals: HashMap<String, usize>,
    app_defs: HashMap<String, usize>,
    migration_defs: HashMap<String, usize>,
    test_defs: HashMap<String, usize>,
    type_defs: HashMap<String, usize>,
    enum_variants: HashMap<String, usize>,
    enum_variant_ambiguous: HashSet<String>,
    enum_variants_by_enum: HashMap<String, HashMap<String, usize>>,
    current_callable: Option<usize>,
}

impl<'a> IndexBuilder<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            defs: Vec::new(),
            refs: Vec::new(),
            calls: Vec::new(),
            qualified_calls: Vec::new(),
            scopes: Vec::new(),
            globals: HashMap::new(),
            app_defs: HashMap::new(),
            migration_defs: HashMap::new(),
            test_defs: HashMap::new(),
            type_defs: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_variant_ambiguous: HashSet::new(),
            enum_variants_by_enum: HashMap::new(),
            current_callable: None,
        }
    }

    fn finish(self) -> Index {
        Index {
            defs: self.defs,
            refs: self.refs,
            calls: self.calls,
            qualified_calls: self.qualified_calls,
        }
    }

    fn collect(&mut self, program: &Program) {
        self.collect_globals(program);
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn collect_globals(&mut self, program: &Program) {
        for item in &program.items {
            match item {
                Item::Import(decl) => self.define_import(decl),
                Item::Type(decl) => self.define_type(decl),
                Item::Enum(decl) => self.define_enum(decl),
                Item::Fn(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Function,
                        self.fn_signature(decl),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::Config(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Config,
                        format!("config {}", decl.name.name),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::Service(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Service,
                        format!("service {}", decl.name.name),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::App(decl) => {
                    let detail = format!("app \"{}\"", decl.name.value);
                    let def_id = self.define_literal_decl(
                        &decl.name,
                        SymbolKind::App,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.app_defs.insert(decl.name.value.clone(), def_id);
                }
                Item::Migration(decl) => {
                    let detail = format!("migration {}", decl.name);
                    let def_id = self.define_span_decl(
                        decl.span,
                        decl.name.clone(),
                        SymbolKind::Migration,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.migration_defs.insert(decl.name.clone(), def_id);
                }
                Item::Test(decl) => {
                    let detail = format!("test \"{}\"", decl.name.value);
                    let def_id = self.define_literal_decl(
                        &decl.name,
                        SymbolKind::Test,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.test_defs.insert(decl.name.value.clone(), def_id);
                }
            }
        }
    }

    fn define_import(&mut self, decl: &ImportDecl) {
        match &decl.spec {
            ImportSpec::Module { name } => {
                self.define_global(
                    name,
                    SymbolKind::Module,
                    format!("module {}", name.name),
                    None,
                    None,
                );
            }
            ImportSpec::ModuleFrom { name, .. } => {
                self.define_global(
                    name,
                    SymbolKind::Module,
                    format!("module {}", name.name),
                    None,
                    None,
                );
            }
            ImportSpec::AliasFrom { alias, .. } => {
                self.define_global(
                    alias,
                    SymbolKind::Module,
                    format!("module {}", alias.name),
                    None,
                    None,
                );
            }
            ImportSpec::NamedFrom { names, .. } => {
                for name in names {
                    self.define_global(
                        name,
                        SymbolKind::Variable,
                        format!("import {}", name.name),
                        None,
                        None,
                    );
                }
            }
        }
    }

    fn define_type(&mut self, decl: &TypeDecl) {
        let def_id = self.define_global(
            &decl.name,
            SymbolKind::Type,
            format!("type {}", decl.name.name),
            decl.doc.as_ref(),
            None,
        );
        self.type_defs.insert(decl.name.name.clone(), def_id);
    }

    fn define_enum(&mut self, decl: &EnumDecl) {
        let def_id = self.define_global(
            &decl.name,
            SymbolKind::Enum,
            format!("enum {}", decl.name.name),
            decl.doc.as_ref(),
            None,
        );
        self.type_defs.insert(decl.name.name.clone(), def_id);
        let mut variants = HashMap::new();
        for variant in &decl.variants {
            let detail = if variant.payload.is_empty() {
                format!("variant {}", variant.name.name)
            } else {
                let payload = variant
                    .payload
                    .iter()
                    .map(|ty| self.type_ref_text(ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("variant {}({})", variant.name.name, payload)
            };
            let def_id = self.define_span_decl(
                variant.name.span,
                variant.name.name.clone(),
                SymbolKind::EnumVariant,
                detail,
                decl.doc.as_ref(),
            );
            variants.insert(variant.name.name.clone(), def_id);
            if self.enum_variant_ambiguous.contains(&variant.name.name) {
                continue;
            }
            if self.enum_variants.contains_key(&variant.name.name) {
                self.enum_variants.remove(&variant.name.name);
                self.enum_variant_ambiguous
                    .insert(variant.name.name.clone());
            } else {
                self.enum_variants.insert(variant.name.name.clone(), def_id);
            }
        }
        self.enum_variants_by_enum
            .insert(decl.name.name.clone(), variants);
    }

    fn visit_item(&mut self, item: &Item) {
        match item {
            Item::Import(_) => {}
            Item::Type(decl) => self.visit_type_decl(decl),
            Item::Enum(decl) => self.visit_enum_decl(decl),
            Item::Fn(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.globals.get(&decl.name.name).copied();
                self.visit_fn_decl(decl);
                self.current_callable = prev;
            }
            Item::Config(decl) => self.visit_config_decl(decl),
            Item::Service(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.globals.get(&decl.name.name).copied();
                self.visit_service_decl(decl);
                self.current_callable = prev;
            }
            Item::App(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.app_defs.get(&decl.name.value).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
            Item::Migration(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.migration_defs.get(&decl.name).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
            Item::Test(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.test_defs.get(&decl.name.value).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
        }
    }

    fn visit_type_decl(&mut self, decl: &TypeDecl) {
        for field in &decl.fields {
            self.visit_type_ref(&field.ty);
            if let Some(expr) = &field.default {
                self.visit_expr(expr);
            }
        }
        if let Some(TypeDerive { base, .. }) = &decl.derive {
            self.add_type_ref(base);
        }
    }

    fn visit_enum_decl(&mut self, decl: &EnumDecl) {
        for variant in &decl.variants {
            for ty in &variant.payload {
                self.visit_type_ref(ty);
            }
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.enter_scope();
        let container = self.current_container();
        for param in &decl.params {
            let detail = format!(
                "param {}: {}",
                param.name.name,
                self.type_ref_text(&param.ty)
            );
            let def_id = self.define_local(
                &param.name,
                SymbolKind::Param,
                detail,
                None,
                container.clone(),
            );
            self.insert_local(&param.name.name, def_id);
            self.visit_type_ref(&param.ty);
            if let Some(expr) = &param.default {
                self.visit_expr(expr);
            }
        }
        if let Some(ret) = &decl.ret {
            self.visit_type_ref(ret);
        }
        self.visit_block_body(&decl.body);
        self.exit_scope();
    }

    fn visit_config_decl(&mut self, decl: &ConfigDecl) {
        for field in &decl.fields {
            let detail = format!(
                "field {}: {}",
                field.name.name,
                self.type_ref_text(&field.ty)
            );
            self.define_span_decl(
                field.name.span,
                field.name.name.clone(),
                SymbolKind::Field,
                detail,
                None,
            );
            self.visit_type_ref(&field.ty);
            self.visit_expr(&field.value);
        }
    }

    fn visit_service_decl(&mut self, decl: &ServiceDecl) {
        let container = self.current_container();
        for route in &decl.routes {
            self.visit_type_ref(&route.ret_type);
            if let Some(body_ty) = &route.body_type {
                self.visit_type_ref(body_ty);
            }
            self.enter_scope();
            if let Some(body_ty) = &route.body_type {
                let detail = format!("param body: {}", self.type_ref_text(body_ty));
                let span = route.body_span.unwrap_or(body_ty.span);
                let def_id = self.define_span_decl_with_container(
                    span,
                    "body".to_string(),
                    SymbolKind::Param,
                    detail,
                    None,
                    container.clone(),
                );
                self.insert_local("body", def_id);
            }
            self.visit_block_body(&route.body);
            self.exit_scope();
        }
    }

    fn visit_block(&mut self, block: &Block) {
        self.enter_scope();
        self.visit_block_body(block);
        self.exit_scope();
    }

    fn visit_block_body(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        let container = self.current_container();
        match &stmt.kind {
            StmtKind::Let { name, ty, expr } => {
                if let Some(ty) = ty {
                    self.visit_type_ref(ty);
                }
                self.visit_expr(expr);
                let detail = match ty {
                    Some(ty) => format!("let {}: {}", name.name, self.type_ref_text(ty)),
                    None => format!("let {}", name.name),
                };
                let def_id =
                    self.define_local(name, SymbolKind::Variable, detail, None, container.clone());
                self.insert_local(&name.name, def_id);
            }
            StmtKind::Var { name, ty, expr } => {
                if let Some(ty) = ty {
                    self.visit_type_ref(ty);
                }
                self.visit_expr(expr);
                let detail = match ty {
                    Some(ty) => format!("var {}: {}", name.name, self.type_ref_text(ty)),
                    None => format!("var {}", name.name),
                };
                let def_id =
                    self.define_local(name, SymbolKind::Variable, detail, None, container.clone());
                self.insert_local(&name.name, def_id);
            }
            StmtKind::Assign { target, expr } => {
                self.visit_expr(target);
                self.visit_expr(expr);
            }
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    self.visit_expr(expr);
                }
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                self.visit_expr(cond);
                self.visit_block(then_block);
                for (expr, block) in else_if {
                    self.visit_expr(expr);
                    self.visit_block(block);
                }
                if let Some(block) = else_block {
                    self.visit_block(block);
                }
            }
            StmtKind::Match { expr, cases } => {
                self.visit_expr(expr);
                for (pat, block) in cases {
                    self.enter_scope();
                    self.visit_pattern(pat);
                    self.visit_block_body(block);
                    self.exit_scope();
                }
            }
            StmtKind::For { pat, iter, block } => {
                self.visit_expr(iter);
                self.enter_scope();
                self.visit_pattern(pat);
                self.visit_block_body(block);
                self.exit_scope();
            }
            StmtKind::While { cond, block } => {
                self.visit_expr(cond);
                self.visit_block(block);
            }
            StmtKind::Expr(expr) => {
                self.visit_expr(expr);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Literal(_) => {}
            ExprKind::Ident(ident) => {
                if let Some(def_id) = self.resolve_value(&ident.name) {
                    self.add_ref(ident.span, def_id);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::Unary { expr, .. } => {
                self.visit_expr(expr);
            }
            ExprKind::Call { callee, args } => {
                self.record_call(callee);
                self.visit_expr(callee);
                for arg in args {
                    if let Some(name) = &arg.name {
                        if let Some(def_id) = self.resolve_value(&name.name) {
                            self.add_ref(name.span, def_id);
                        }
                    }
                    self.visit_expr(&arg.value);
                }
            }
            ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
                if let ExprKind::Ident(base_ident) = &base.kind {
                    if let Some(map) = self.enum_variants_by_enum.get(&base_ident.name) {
                        if let Some(def_id) = map.get(&name.name) {
                            self.add_ref(name.span, *def_id);
                        }
                    }
                }
                self.visit_expr(base);
            }
            ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
                self.visit_expr(base);
                self.visit_expr(index);
            }
            ExprKind::StructLit { name, fields } => {
                self.add_type_ref(name);
                for field in fields {
                    self.visit_expr(&field.value);
                }
            }
            ExprKind::ListLit(items) => {
                for item in items {
                    self.visit_expr(item);
                }
            }
            ExprKind::MapLit(items) => {
                for (key, value) in items {
                    self.visit_expr(key);
                    self.visit_expr(value);
                }
            }
            ExprKind::InterpString(parts) => {
                for part in parts {
                    if let fusec::ast::InterpPart::Expr(expr) = part {
                        self.visit_expr(expr);
                    }
                }
            }
            ExprKind::Coalesce { left, right } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::BangChain { expr, error } => {
                self.visit_expr(expr);
                if let Some(err) = error {
                    self.visit_expr(err);
                }
            }
            ExprKind::Spawn { block } => self.visit_block(block),
            ExprKind::Await { expr } => self.visit_expr(expr),
            ExprKind::Box { expr } => self.visit_expr(expr),
        }
    }

    fn visit_type_ref(&mut self, ty: &TypeRef) {
        match &ty.kind {
            TypeRefKind::Simple(ident) => self.add_type_ref(ident),
            TypeRefKind::Generic { base, args } => {
                self.add_type_ref(base);
                for arg in args {
                    self.visit_type_ref(arg);
                }
            }
            TypeRefKind::Optional(inner) => self.visit_type_ref(inner),
            TypeRefKind::Result { ok, err } => {
                self.visit_type_ref(ok);
                if let Some(err) = err {
                    self.visit_type_ref(err);
                }
            }
            TypeRefKind::Refined { base, args } => {
                self.add_type_ref(base);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
        }
    }

    fn visit_pattern(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
            PatternKind::Ident(ident) => {
                let detail = format!("let {}", ident.name);
                let def_id = self.define_local(
                    ident,
                    SymbolKind::Variable,
                    detail,
                    None,
                    self.current_container(),
                );
                self.insert_local(&ident.name, def_id);
            }
            PatternKind::EnumVariant { name, args } => {
                if let Some(def_id) = self.enum_variants.get(&name.name) {
                    self.add_ref(name.span, *def_id);
                }
                for arg in args {
                    self.visit_pattern(arg);
                }
            }
            PatternKind::Struct { name, fields } => {
                self.add_type_ref(name);
                for field in fields {
                    self.visit_pattern(&field.pat);
                }
            }
        }
    }

    fn record_call(&mut self, callee: &Expr) {
        let Some(caller) = self.current_callable else {
            return;
        };
        if let Some(target) = self.call_target_local(callee) {
            self.calls.push(CallRef {
                caller,
                callee: target,
                span: callee.span,
            });
            return;
        }
        if let Some((module, item, span)) = self.call_target_qualified(callee) {
            self.qualified_calls.push(QualifiedCallRef {
                caller,
                module,
                item,
                span,
            });
        }
    }

    fn call_target_local(&self, callee: &Expr) -> Option<usize> {
        match &callee.kind {
            ExprKind::Ident(ident) => self.resolve_value(&ident.name),
            _ => None,
        }
    }

    fn call_target_qualified(&self, callee: &Expr) -> Option<(String, String, Span)> {
        let (base, name) = match &callee.kind {
            ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
                (base, name)
            }
            _ => return None,
        };
        let ExprKind::Ident(base_ident) = &base.kind else {
            return None;
        };
        let Some(base_def_id) = self.resolve_value(&base_ident.name) else {
            return None;
        };
        let Some(base_def) = self.defs.get(base_def_id) else {
            return None;
        };
        if base_def.kind != SymbolKind::Module {
            return None;
        }
        Some((base_ident.name.clone(), name.name.clone(), name.span))
    }

    fn add_type_ref(&mut self, ident: &Ident) {
        if ident.name.contains('.') {
            return;
        }
        if is_builtin_type(&ident.name) {
            return;
        }
        if let Some(def_id) = self
            .type_defs
            .get(&ident.name)
            .copied()
            .or_else(|| self.globals.get(&ident.name).copied())
        {
            self.add_ref(ident.span, def_id);
        }
    }

    fn resolve_value(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(def_id) = scope.get(name) {
                return Some(*def_id);
            }
        }
        self.globals.get(name).copied()
    }

    fn add_ref(&mut self, span: Span, target: usize) {
        self.refs.push(SymbolRef { span, target });
    }

    fn define_global(
        &mut self,
        ident: &Ident,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
        container: Option<String>,
    ) -> usize {
        if let Some(def_id) = self.globals.get(&ident.name) {
            return *def_id;
        }
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name: ident.name.clone(),
            span: ident.span,
            kind,
            detail,
            doc,
            container,
        });
        self.globals.insert(ident.name.clone(), def_id);
        def_id
    }

    fn define_literal_decl(
        &mut self,
        lit: &fusec::ast::StringLit,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
    ) -> usize {
        self.define_span_decl(lit.span, lit.value.clone(), kind, detail, doc)
    }

    fn define_span_decl(
        &mut self,
        span: Span,
        name: String,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
    ) -> usize {
        self.define_span_decl_with_container(span, name, kind, detail, doc, None)
    }

    fn define_span_decl_with_container(
        &mut self,
        span: Span,
        name: String,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
        container: Option<String>,
    ) -> usize {
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name,
            span,
            kind,
            detail,
            doc,
            container,
        });
        def_id
    }

    fn define_local(
        &mut self,
        ident: &Ident,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
        container: Option<String>,
    ) -> usize {
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name: ident.name.clone(),
            span: ident.span,
            kind,
            detail,
            doc,
            container,
        });
        def_id
    }

    fn insert_local(&mut self, name: &str, def_id: usize) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), def_id);
        }
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    fn fn_signature(&self, decl: &FnDecl) -> String {
        let mut out = format!("fn {}(", decl.name.name);
        for (idx, param) in decl.params.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(&param.name.name);
            out.push_str(": ");
            out.push_str(&self.type_ref_text(&param.ty));
        }
        out.push(')');
        if let Some(ret) = &decl.ret {
            out.push_str(" -> ");
            out.push_str(&self.type_ref_text(ret));
        }
        out
    }

    fn type_ref_text(&self, ty: &TypeRef) -> String {
        self.slice_span(ty.span).trim().to_string()
    }

    fn current_container(&self) -> Option<String> {
        let id = self.current_callable?;
        self.defs.get(id).map(|def| def.name.clone())
    }

    fn slice_span(&self, span: Span) -> String {
        self.text
            .get(span.start..span.end)
            .unwrap_or("")
            .to_string()
    }
}

fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "Int"
            | "Float"
            | "Bool"
            | "String"
            | "Bytes"
            | "Html"
            | "Id"
            | "Email"
            | "Error"
            | "List"
            | "Map"
            | "Option"
            | "Result"
    )
}
