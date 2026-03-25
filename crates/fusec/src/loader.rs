use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use fuse_rt::json as rt_json;

use crate::manifest::build_transitive_deps;

use crate::ast::{FieldDecl, ImportDecl, ImportSpec, Item, Program, TypeDecl, TypeDerive};
use crate::diag::{Diag, Diagnostics};
use crate::parse_source;
use crate::span::Span;

pub type ModuleId = usize;

const FUSE_IMPORT_CYCLE: &str = "FUSE_IMPORT_CYCLE";
const FUSE_IMPORT_MODULE_READ: &str = "FUSE_IMPORT_MODULE_READ";
const FUSE_IMPORT_ASSET_FORM: &str = "FUSE_IMPORT_ASSET_FORM";
const FUSE_IMPORT_UNSUPPORTED_EXTENSION: &str = "FUSE_IMPORT_UNSUPPORTED_EXTENSION";
const FUSE_IMPORT_DUPLICATE: &str = "FUSE_IMPORT_DUPLICATE";
const FUSE_IMPORT_UNKNOWN: &str = "FUSE_IMPORT_UNKNOWN";
const FUSE_IMPORT_DEP_PATH: &str = "FUSE_IMPORT_DEP_PATH";
const FUSE_IMPORT_UNKNOWN_DEPENDENCY: &str = "FUSE_IMPORT_UNKNOWN_DEPENDENCY";
const FUSE_IMPORT_ROOT_PATH: &str = "FUSE_IMPORT_ROOT_PATH";
const FUSE_IMPORT_ROOT_ESCAPE: &str = "FUSE_IMPORT_ROOT_ESCAPE";
const FUSE_ASSET_MISSING: &str = "FUSE_ASSET_MISSING";
const FUSE_ASSET_READ: &str = "FUSE_ASSET_READ";
const FUSE_ASSET_UTF8: &str = "FUSE_ASSET_UTF8";
const FUSE_ASSET_JSON_INVALID: &str = "FUSE_ASSET_JSON_INVALID";
const FUSE_DEP_CYCLE: &str = "FUSE_DEP_CYCLE";
const FUSE_TYPE_DERIVE_CYCLE: &str = "FUSE_TYPE_DERIVE_CYCLE";
const FUSE_TYPE_UNKNOWN: &str = "FUSE_TYPE_UNKNOWN";
const FUSE_TYPE_DERIVE_BASE: &str = "FUSE_TYPE_DERIVE_BASE";
const FUSE_TYPE_DERIVE_FIELD: &str = "FUSE_TYPE_DERIVE_FIELD";
const FUSE_SYMBOL_DUPLICATE: &str = "FUSE_SYMBOL_DUPLICATE";

const STD_ERROR_MODULE: &str = r#"
type Error:
  code: String
  message: String
  status: Int = 500

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

#[derive(Clone, Debug)]
pub struct ModuleLink {
    pub id: ModuleId,
    pub exports: ModuleExports,
}

#[derive(Clone, Debug, Default)]
pub struct ModuleMap {
    pub modules: HashMap<String, ModuleLink>,
}

impl ModuleMap {
    pub fn get(&self, name: &str) -> Option<&ModuleLink> {
        self.modules.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModuleExports {
    pub types: HashSet<String>,
    pub enums: HashSet<String>,
    pub interfaces: HashSet<String>,
    pub functions: HashSet<String>,
    pub configs: HashSet<String>,
    pub services: HashSet<String>,
    pub apps: HashSet<String>,
}

impl ModuleExports {
    fn from_program(program: &Program) -> Self {
        let mut exports = ModuleExports::default();
        for item in &program.items {
            match item {
                Item::Type(decl) => {
                    exports.types.insert(decl.name.name.clone());
                }
                Item::Enum(decl) => {
                    exports.enums.insert(decl.name.name.clone());
                }
                Item::Interface(decl) => {
                    exports.interfaces.insert(decl.name.name.clone());
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

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.types.contains(name)
            || self.enums.contains(name)
            || self.interfaces.contains(name)
            || self.functions.contains(name)
            || self.configs.contains(name)
            || self.services.contains(name)
            || self.apps.contains(name)
    }
}

#[derive(Clone, Debug)]
pub struct ModuleUnit {
    pub id: ModuleId,
    pub path: PathBuf,
    pub program: Program,
    pub modules: ModuleMap,
    pub import_items: HashMap<String, ModuleLink>,
    pub import_assets: HashMap<String, ImportedAsset>,
    pub exports: ModuleExports,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportedAssetKind {
    Markdown,
    Json,
}

#[derive(Clone, Debug)]
pub enum ImportedAssetValue {
    Markdown(String),
    Json(rt_json::JsonValue),
}

#[derive(Clone, Debug)]
pub struct ImportedAsset {
    pub path: PathBuf,
    pub kind: ImportedAssetKind,
    pub value: ImportedAssetValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportPathKind {
    Module,
    Asset(ImportedAssetKind),
    UnsupportedAssetExtension,
}

pub fn classify_import_path(raw: &str) -> ImportPathKind {
    if raw == "std.Error" || raw == "<std.Error>" {
        return ImportPathKind::Module;
    }
    classify_path_extension(Path::new(raw))
}

fn classify_path_extension(path: &Path) -> ImportPathKind {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return ImportPathKind::Module;
    };
    match ext.to_ascii_lowercase().as_str() {
        "fuse" => ImportPathKind::Module,
        "md" => ImportPathKind::Asset(ImportedAssetKind::Markdown),
        "json" => ImportPathKind::Asset(ImportedAssetKind::Json),
        _ => ImportPathKind::UnsupportedAssetExtension,
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModuleRegistry {
    pub root: ModuleId,
    pub modules: HashMap<ModuleId, ModuleUnit>,
}

impl ModuleRegistry {
    pub fn root(&self) -> Option<&ModuleUnit> {
        self.modules.get(&self.root)
    }

    pub fn get(&self, id: ModuleId) -> Option<&ModuleUnit> {
        self.modules.get(&id)
    }
}

pub fn load_program_with_modules(path: &Path, src: &str) -> (ModuleRegistry, Vec<Diag>) {
    load_program_with_modules_and_deps(path, src, &HashMap::new())
}

pub fn load_program_with_modules_and_deps(
    path: &Path,
    src: &str,
    deps: &HashMap<String, PathBuf>,
) -> (ModuleRegistry, Vec<Diag>) {
    let (transitive_deps, cycle_errors) = build_transitive_deps(deps);
    let mut loader = ModuleLoader::with_deps(&transitive_deps);
    for msg in cycle_errors {
        loader
            .diags
            .error_with_code(crate::span::Span::default(), FUSE_DEP_CYCLE, msg);
    }
    let root = loader.insert_root(path, src);
    let root = root.unwrap_or(0);
    let mut registry = ModuleRegistry {
        root,
        modules: loader.modules,
    };
    crate::frontend::canonicalize::canonicalize_registry(&mut registry);
    (registry, loader.diags.into_vec())
}

pub fn load_program_with_modules_and_deps_and_overrides(
    path: &Path,
    src: &str,
    deps: &HashMap<String, PathBuf>,
    overrides: &HashMap<PathBuf, String>,
) -> (ModuleRegistry, Vec<Diag>) {
    let (transitive_deps, cycle_errors) = build_transitive_deps(deps);
    let mut loader = ModuleLoader::with_deps_and_overrides(&transitive_deps, overrides);
    for msg in cycle_errors {
        loader
            .diags
            .error_with_code(crate::span::Span::default(), FUSE_DEP_CYCLE, msg);
    }
    let root = loader.insert_root(path, src);
    let root = root.unwrap_or(0);
    let mut registry = ModuleRegistry {
        root,
        modules: loader.modules,
    };
    crate::frontend::canonicalize::canonicalize_registry(&mut registry);
    (registry, loader.diags.into_vec())
}

pub fn load_program(_path: &Path, src: &str) -> (Program, Vec<Diag>) {
    let (program, diags) = parse_source(src);
    (program, diags)
}

struct ModuleLoader {
    next_id: ModuleId,
    by_path: HashMap<PathBuf, ModuleId>,
    assets: HashMap<PathBuf, ImportedAsset>,
    modules: HashMap<ModuleId, ModuleUnit>,
    visiting: HashSet<PathBuf>,
    deps: HashMap<String, PathBuf>,
    workspace_root: PathBuf,
    source_overrides: HashMap<PathBuf, String>,
    diags: Diagnostics,
    global_names: HashMap<String, (ModuleId, Span)>,
}

impl ModuleLoader {
    fn with_deps(deps: &HashMap<String, PathBuf>) -> Self {
        Self {
            next_id: 1,
            by_path: HashMap::new(),
            assets: HashMap::new(),
            modules: HashMap::new(),
            visiting: HashSet::new(),
            deps: deps.clone(),
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            source_overrides: HashMap::new(),
            diags: Diagnostics::default(),
            global_names: HashMap::new(),
        }
    }

    fn with_deps_and_overrides(
        deps: &HashMap<String, PathBuf>,
        overrides: &HashMap<PathBuf, String>,
    ) -> Self {
        let mut source_overrides = HashMap::new();
        for (path, contents) in overrides {
            let key = if let Ok(canon) = path.canonicalize() {
                canon
            } else {
                path.to_path_buf()
            };
            source_overrides.insert(key, contents.clone());
        }
        Self {
            next_id: 1,
            by_path: HashMap::new(),
            assets: HashMap::new(),
            modules: HashMap::new(),
            visiting: HashSet::new(),
            deps: deps.clone(),
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            source_overrides,
            diags: Diagnostics::default(),
            global_names: HashMap::new(),
        }
    }

    fn insert_root(&mut self, path: &Path, src: &str) -> Option<ModuleId> {
        self.workspace_root = workspace_root_for_entry(path);
        self.load_module(path, Some(src), Span::default())
    }

    fn load_module(
        &mut self,
        path: &Path,
        src_override: Option<&str>,
        span: Span,
    ) -> Option<ModuleId> {
        let key = self.normalize_path(path);
        if let Some(id) = self.by_path.get(&key) {
            return Some(*id);
        }
        if self.visiting.contains(&key) {
            // Build the cycle path for a readable error.
            let cycle_path = self
                .visiting
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>();
            self.diags.error_with_code(
                span,
                FUSE_IMPORT_CYCLE,
                format!(
                    "circular import: {} → {}",
                    cycle_path.join(" → "),
                    key.display()
                ),
            );
            return self.by_path.get(&key).copied();
        }
        self.visiting.insert(key.clone());

        let src = match src_override {
            Some(src) => src.to_string(),
            None if is_std_error_virtual_path(&key) => STD_ERROR_MODULE.to_string(),
            None => {
                if let Some(src) = self.source_overrides.get(&key) {
                    src.clone()
                } else {
                    match fs::read_to_string(&key) {
                        Ok(src) => src,
                        Err(err) => {
                            self.diags.error_with_code(
                                span,
                                FUSE_IMPORT_MODULE_READ,
                                format!("failed to read module {}: {err}", key.display()),
                            );
                            self.visiting.remove(&key);
                            return None;
                        }
                    }
                }
            }
        };
        let (program, mut diags) = parse_source(&src);
        for diag in &mut diags {
            diag.path = Some(key.clone());
        }
        self.diags.extend(diags);

        let id = self.next_id;
        self.next_id += 1;
        let exports = ModuleExports::from_program(&program);
        let unit = ModuleUnit {
            id,
            path: key.clone(),
            program,
            modules: ModuleMap::default(),
            import_items: HashMap::new(),
            import_assets: HashMap::new(),
            exports,
        };
        self.by_path.insert(key.clone(), id);
        self.modules.insert(id, unit);
        self.register_global_exports(id);

        self.resolve_imports(id);

        self.visiting.remove(&key);
        Some(id)
    }

    fn resolve_imports(&mut self, id: ModuleId) {
        let (imports, import_items, import_assets, base_dir, importer_path) = {
            let Some(unit) = self.modules.get(&id) else {
                return;
            };
            let base_dir = unit
                .path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            let imports: Vec<ImportDecl> = unit
                .program
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Import(decl) => Some(decl.clone()),
                    _ => None,
                })
                .collect();
            (
                imports,
                HashMap::new(),
                HashMap::new(),
                base_dir,
                unit.path.clone(),
            )
        };

        let mut module_map = ModuleMap::default();
        let mut import_items = import_items;
        let mut import_assets = import_assets;
        let mut import_item_spans: HashMap<String, Span> = HashMap::new();

        for import in imports {
            let span = import.span;
            match import.spec {
                ImportSpec::Module { name } => {
                    let mut path = base_dir.join(&name.name);
                    if path.extension().is_none() {
                        path.set_extension("fuse");
                    }
                    if let Some(module_id) = self.load_module(&path, None, span) {
                        self.insert_module_alias(&mut module_map, &name.name, module_id);
                    }
                }
                ImportSpec::ModuleFrom { name, path } => {
                    match classify_import_path(&path.value) {
                        ImportPathKind::Module => {
                            let module_id = self.load_std_module(&path.value, span).or_else(|| {
                                let path = self.resolve_path(&base_dir, &path.value, path.span);
                                self.load_module(&path, None, span)
                            });
                            if let Some(module_id) = module_id {
                                self.insert_module_alias(&mut module_map, &name.name, module_id);
                            }
                        }
                        ImportPathKind::Asset(kind) => {
                            let target_path =
                                self.resolve_path(&base_dir, &path.value, path.span);
                            if let Some(asset) =
                                self.load_asset(&target_path, &importer_path, path.span, kind)
                            {
                                import_assets.entry(name.name.clone()).or_insert(asset);
                            }
                        }
                        ImportPathKind::UnsupportedAssetExtension => {
                            self.diags.error_at_path_with_code(
                                importer_path.clone(),
                                path.span,
                                FUSE_IMPORT_UNSUPPORTED_EXTENSION,
                                unsupported_import_extension_message(&path.value),
                            );
                        }
                    }
                }
                ImportSpec::AliasFrom { alias, path, .. } => {
                    match classify_import_path(&path.value) {
                        ImportPathKind::Module => {
                            let module_id = self.load_std_module(&path.value, span).or_else(|| {
                                let path = self.resolve_path(&base_dir, &path.value, path.span);
                                self.load_module(&path, None, span)
                            });
                            if let Some(module_id) = module_id {
                                self.insert_module_alias(&mut module_map, &alias.name, module_id);
                            }
                        }
                        ImportPathKind::Asset(_) => {
                            self.diags.error_at_path_with_code(
                                importer_path.clone(),
                                span,
                                FUSE_IMPORT_ASSET_FORM,
                                asset_import_form_message(),
                            );
                        }
                        ImportPathKind::UnsupportedAssetExtension => {
                            self.diags.error_at_path_with_code(
                                importer_path.clone(),
                                path.span,
                                FUSE_IMPORT_UNSUPPORTED_EXTENSION,
                                unsupported_import_extension_message(&path.value),
                            );
                        }
                    }
                }
                ImportSpec::NamedFrom { names, path } => {
                    match classify_import_path(&path.value) {
                        ImportPathKind::Module => {
                            let module_id = self.load_std_module(&path.value, span).or_else(|| {
                                let path = self.resolve_path(&base_dir, &path.value, path.span);
                                self.load_module(&path, None, span)
                            });
                            let Some(module_id) = module_id else {
                                continue;
                            };
                            let exports = self.modules.get(&module_id).map(|unit| &unit.exports);
                            for name in names {
                                if let Some(prev_span) = import_item_spans.get(&name.name).copied() {
                                    self.diags.error_with_code(
                                        name.span,
                                        FUSE_IMPORT_DUPLICATE,
                                        format!("duplicate import {}", name.name),
                                    );
                                    self.diags.error_with_code(
                                        prev_span,
                                        FUSE_IMPORT_DUPLICATE,
                                        format!("previous import of {} here", name.name),
                                    );
                                    continue;
                                }
                                let Some(exports) = exports else { continue };
                                if !exports.contains(&name.name) {
                                    self.diags.error_with_code(
                                        name.span,
                                        FUSE_IMPORT_UNKNOWN,
                                        format!("unknown import {} in {}", name.name, path.value),
                                    );
                                    continue;
                                }
                                import_items.insert(name.name.clone(), self.link_for(module_id));
                                import_item_spans.insert(name.name.clone(), name.span);
                            }
                        }
                        ImportPathKind::Asset(_) => {
                            self.diags.error_at_path_with_code(
                                importer_path.clone(),
                                span,
                                FUSE_IMPORT_ASSET_FORM,
                                asset_import_form_message(),
                            );
                        }
                        ImportPathKind::UnsupportedAssetExtension => {
                            self.diags.error_at_path_with_code(
                                importer_path.clone(),
                                path.span,
                                FUSE_IMPORT_UNSUPPORTED_EXTENSION,
                                unsupported_import_extension_message(&path.value),
                            );
                        }
                    }
                }
            }
        }

        if let Some(unit) = self.modules.get_mut(&id) {
            unit.modules = module_map;
            unit.import_items = import_items;
            unit.import_assets = import_assets;
        }

        self.expand_type_derivations(id);
    }

    fn expand_type_derivations(&mut self, module_id: ModuleId) {
        let mut cache: HashMap<(ModuleId, String), Vec<FieldDecl>> = HashMap::new();
        let mut visiting: HashSet<(ModuleId, String)> = HashSet::new();

        let Some(unit) = self.modules.get(&module_id) else {
            return;
        };
        let derived: Vec<String> = unit
            .program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Type(decl) if decl.derive.is_some() => Some(decl.name.name.clone()),
                _ => None,
            })
            .collect();

        for name in derived {
            let fields = self.resolve_derived_fields(module_id, &name, &mut cache, &mut visiting);
            if let Some(fields) = fields {
                if let Some(unit) = self.modules.get_mut(&module_id) {
                    for item in &mut unit.program.items {
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
    }

    fn resolve_derived_fields(
        &mut self,
        module_id: ModuleId,
        name: &str,
        cache: &mut HashMap<(ModuleId, String), Vec<FieldDecl>>,
        visiting: &mut HashSet<(ModuleId, String)>,
    ) -> Option<Vec<FieldDecl>> {
        let key = (module_id, name.to_string());
        if let Some(fields) = cache.get(&key) {
            return Some(fields.clone());
        }
        if visiting.contains(&key) {
            self.diags.error_with_code(
                Span::default(),
                FUSE_TYPE_DERIVE_CYCLE,
                format!("cyclic type derivation for {name}"),
            );
            return None;
        }
        visiting.insert(key.clone());

        let decl = match self.find_type_decl(module_id, name) {
            Some(decl) => decl,
            None => {
                self.diags.error_with_code(
                    Span::default(),
                    FUSE_TYPE_UNKNOWN,
                    format!("unknown type {name}"),
                );
                visiting.remove(&key);
                return None;
            }
        };

        let fields = if let Some(derive) = &decl.derive {
            self.resolve_without_fields(module_id, derive, cache, visiting)
        } else {
            Some(decl.fields.clone())
        };

        if let Some(fields) = &fields {
            cache.insert(key.clone(), fields.clone());
        }
        visiting.remove(&key);
        fields
    }

    fn resolve_without_fields(
        &mut self,
        module_id: ModuleId,
        derive: &TypeDerive,
        cache: &mut HashMap<(ModuleId, String), Vec<FieldDecl>>,
        visiting: &mut HashSet<(ModuleId, String)>,
    ) -> Option<Vec<FieldDecl>> {
        let (base_module, base_name) = match self.resolve_type_target(module_id, &derive.base) {
            Some(value) => value,
            None => {
                self.diags.error_with_code(
                    derive.base.span,
                    FUSE_TYPE_DERIVE_BASE,
                    format!("unknown base type {}", derive.base.name),
                );
                return None;
            }
        };
        if self.find_type_decl(base_module, &base_name).is_none() {
            self.diags.error_with_code(
                derive.base.span,
                FUSE_TYPE_DERIVE_BASE,
                format!("unknown base type {}", derive.base.name),
            );
            return None;
        }
        let base_fields = self.resolve_derived_fields(base_module, &base_name, cache, visiting)?;

        let mut removed = HashSet::new();
        for field in &derive.without {
            removed.insert(field.name.clone());
        }

        for field in &derive.without {
            if !base_fields.iter().any(|f| f.name.name == field.name) {
                self.diags.error_with_code(
                    field.span,
                    FUSE_TYPE_DERIVE_FIELD,
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

    fn resolve_type_target(
        &self,
        module_id: ModuleId,
        base: &crate::ast::Ident,
    ) -> Option<(ModuleId, String)> {
        let unit = self.modules.get(&module_id)?;
        let name = base.name.as_str();
        if let Some((module, item)) = split_qualified_name(name) {
            let link = unit.modules.get(module)?;
            return Some((link.id, item.to_string()));
        }
        if let Some(link) = unit.import_items.get(name) {
            return Some((link.id, name.to_string()));
        }
        Some((module_id, name.to_string()))
    }

    fn find_type_decl(&self, module_id: ModuleId, name: &str) -> Option<TypeDecl> {
        let unit = self.modules.get(&module_id)?;
        unit.program.items.iter().find_map(|item| match item {
            Item::Type(decl) if decl.name.name == name => Some(decl.clone()),
            _ => None,
        })
    }

    fn insert_module_alias(&mut self, map: &mut ModuleMap, alias: &str, module_id: ModuleId) {
        if map.modules.contains_key(alias) {
            return;
        }
        map.modules
            .insert(alias.to_string(), self.link_for(module_id));
    }

    fn link_for(&self, module_id: ModuleId) -> ModuleLink {
        let exports = self
            .modules
            .get(&module_id)
            .map(|unit| unit.exports.clone())
            .unwrap_or_default();
        ModuleLink {
            id: module_id,
            exports,
        }
    }

    fn register_global_exports(&mut self, module_id: ModuleId) {
        let Some(unit) = self.modules.get(&module_id) else {
            return;
        };
        if is_std_error_virtual_path(&unit.path) {
            return;
        }
        for item in &unit.program.items {
            let (name, span) = match item {
                Item::Type(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::Enum(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::Config(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::Service(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::App(decl) => (decl.name.value.as_str(), decl.name.span),
                Item::Fn(_) => continue,
                _ => continue,
            };
            if let Some((prev_id, prev_span)) = self.global_names.get(name) {
                if *prev_id != module_id {
                    let path = unit.path.clone();
                    self.diags.error_at_path_with_code(
                        path.clone(),
                        span,
                        FUSE_SYMBOL_DUPLICATE,
                        format!("duplicate symbol: {name}"),
                    );
                    if let Some(prev_unit) = self.modules.get(prev_id) {
                        self.diags.error_at_path_with_code(
                            prev_unit.path.clone(),
                            *prev_span,
                            FUSE_SYMBOL_DUPLICATE,
                            format!("previous definition of {name} here"),
                        );
                    } else {
                        self.diags.error_with_code(
                            span,
                            FUSE_SYMBOL_DUPLICATE,
                            format!("previous definition of {name} here"),
                        );
                    }
                }
                continue;
            }
            self.global_names
                .insert(name.to_string(), (module_id, span));
        }
    }

    fn resolve_path(&mut self, base_dir: &Path, raw: &str, span: Span) -> PathBuf {
        if let Some(rest) = raw.strip_prefix("dep:") {
            let (dep, rel) = match rest.split_once('/') {
                Some((dep, rel)) if !dep.is_empty() && !rel.is_empty() => (dep, rel),
                _ => {
                    self.diags.error_with_code(
                        span,
                        FUSE_IMPORT_DEP_PATH,
                        "dependency imports require dep:<name>/<path>",
                    );
                    return base_dir.join(raw);
                }
            };
            let Some(root) = self.deps.get(dep) else {
                let available: Vec<&str> = {
                    let mut names: Vec<&str> = self.deps.keys().map(|s| s.as_str()).collect();
                    names.sort_unstable();
                    names
                };
                let hint = if available.is_empty() {
                    " — no dependencies declared in fuse.toml".to_string()
                } else {
                    format!(" — available: {}", available.join(", "))
                };
                self.diags.error_with_code(
                    span,
                    FUSE_IMPORT_UNKNOWN_DEPENDENCY,
                    format!("unknown dependency '{dep}'{hint}"),
                );
                return base_dir.join(raw);
            };
            let mut path = root.join(rel);
            if path.extension().is_none() {
                path.set_extension("fuse");
            }
            return path;
        }
        if let Some(rel) = raw.strip_prefix("root:") {
            if rel.is_empty() {
                self.diags.error_with_code(
                    span,
                    FUSE_IMPORT_ROOT_PATH,
                    "root imports require root:<path>",
                );
                return base_dir.join(raw);
            }
            if let Some(path) = self.resolve_root_path(rel) {
                return path;
            }
            self.diags.error_with_code(
                span,
                FUSE_IMPORT_ROOT_ESCAPE,
                "root import path escapes workspace root",
            );
            return base_dir.join(raw);
        }
        let mut path = PathBuf::from(raw);
        if path.extension().is_none() {
            path.set_extension("fuse");
        }
        if path.is_relative() {
            path = base_dir.join(path);
        }
        path
    }

    fn resolve_root_path(&self, rel: &str) -> Option<PathBuf> {
        let rel = Path::new(rel);
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
        Some(self.workspace_root.join(normalized_rel))
    }

    fn load_std_module(&mut self, name: &str, span: Span) -> Option<ModuleId> {
        if name != "std.Error" && name != "<std.Error>" {
            return None;
        }
        let path = std_error_virtual_path();
        if let Some(id) = self.by_path.get(&path) {
            return Some(*id);
        }
        self.load_module(&path, Some(STD_ERROR_MODULE), span)
    }

    fn load_asset(
        &mut self,
        path: &Path,
        importer_path: &Path,
        span: Span,
        kind: ImportedAssetKind,
    ) -> Option<ImportedAsset> {
        let key = self.normalize_path(path);
        if let Some(asset) = self.assets.get(&key) {
            return Some(asset.clone());
        }
        let bytes = if let Some(src) = self.source_overrides.get(&key) {
            src.as_bytes().to_vec()
        } else {
            match fs::read(&key) {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    self.diags.error_at_path_with_code(
                        importer_path.to_path_buf(),
                        span,
                        FUSE_ASSET_MISSING,
                        format!("missing asset file {}", key.display()),
                    );
                    return None;
                }
                Err(err) => {
                    self.diags.error_at_path_with_code(
                        importer_path.to_path_buf(),
                        span,
                        FUSE_ASSET_READ,
                        format!("failed to read asset {}: {err}", key.display()),
                    );
                    return None;
                }
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                self.diags.error_at_path_with_code(
                    key.clone(),
                    Span::default(),
                    FUSE_ASSET_UTF8,
                    "asset file is not valid UTF-8",
                );
                return None;
            }
        };
        let value = match kind {
            ImportedAssetKind::Markdown => ImportedAssetValue::Markdown(text),
            ImportedAssetKind::Json => match rt_json::decode(&text) {
                Ok(json) => ImportedAssetValue::Json(json),
                Err(err) => {
                    self.diags.error_at_path_with_code(
                        key.clone(),
                        Span::default(),
                        FUSE_ASSET_JSON_INVALID,
                        format!("invalid json: {err}"),
                    );
                    return None;
                }
            },
        };
        let asset = ImportedAsset {
            path: key.clone(),
            kind,
            value,
        };
        self.assets.insert(key, asset.clone());
        Some(asset)
    }

    fn normalize_path(&self, path: &Path) -> PathBuf {
        if is_std_error_virtual_path(path) {
            return std_error_virtual_path();
        }
        if let Ok(canon) = path.canonicalize() {
            canon
        } else {
            path.to_path_buf()
        }
    }
}

fn workspace_root_for_entry(path: &Path) -> PathBuf {
    let start = if path.is_dir() {
        path.to_path_buf()
    } else if let Some(parent) = path.parent() {
        parent.to_path_buf()
    } else {
        PathBuf::from(".")
    };
    let start = if let Ok(canon) = start.canonicalize() {
        canon
    } else {
        start
    };
    for ancestor in start.ancestors() {
        if ancestor.join("fuse.toml").exists() {
            return ancestor.to_path_buf();
        }
    }
    start
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

fn std_error_virtual_path() -> PathBuf {
    PathBuf::from("<std.Error>")
}

fn is_std_error_virtual_path(path: &Path) -> bool {
    if path.to_string_lossy() == "<std.Error>" {
        return true;
    }
    matches!(path.file_name(), Some(name) if name == OsStr::new("<std.Error>"))
}

fn asset_import_form_message() -> &'static str {
    "asset imports only support `import Name from \"path.ext\"`"
}

fn unsupported_import_extension_message(raw: &str) -> String {
    let ext = Path::new(raw)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{ext}"))
        .unwrap_or_else(|| "unknown".to_string());
    format!("unsupported import extension {ext}; only .fuse, .md, and .json are supported")
}
