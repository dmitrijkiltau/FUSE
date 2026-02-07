use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{FieldDecl, ImportDecl, ImportSpec, Item, Program, TypeDecl, TypeDerive};
use crate::diag::{Diag, Diagnostics};
use crate::parse_source;
use crate::span::Span;

pub type ModuleId = usize;

const STD_ERROR_MODULE: &str = r#"
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
    pub exports: ModuleExports,
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
    let mut loader = ModuleLoader::with_deps(deps);
    let root = loader.insert_root(path, src);
    let root = root.unwrap_or(0);
    (
        ModuleRegistry {
            root,
            modules: loader.modules,
        },
        loader.diags.into_vec(),
    )
}

pub fn load_program(_path: &Path, src: &str) -> (Program, Vec<Diag>) {
    let (program, diags) = parse_source(src);
    (program, diags)
}

struct ModuleLoader {
    next_id: ModuleId,
    by_path: HashMap<PathBuf, ModuleId>,
    modules: HashMap<ModuleId, ModuleUnit>,
    visiting: HashSet<PathBuf>,
    deps: HashMap<String, PathBuf>,
    diags: Diagnostics,
    global_names: HashMap<String, (ModuleId, Span)>,
}

impl ModuleLoader {
    fn with_deps(deps: &HashMap<String, PathBuf>) -> Self {
        Self {
            next_id: 1,
            by_path: HashMap::new(),
            modules: HashMap::new(),
            visiting: HashSet::new(),
            deps: deps.clone(),
            diags: Diagnostics::default(),
            global_names: HashMap::new(),
        }
    }

    fn insert_root(&mut self, path: &Path, src: &str) -> Option<ModuleId> {
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
            self.diags
                .error(span, format!("cyclic module import {}", key.display()));
            return self.by_path.get(&key).copied();
        }
        self.visiting.insert(key.clone());

        let src = match src_override {
            Some(src) => src.to_string(),
            None => match fs::read_to_string(&key) {
                Ok(src) => src,
                Err(err) => {
                    self.diags.error(
                        span,
                        format!("failed to read module {}: {err}", key.display()),
                    );
                    self.visiting.remove(&key);
                    return None;
                }
            },
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
        let (imports, import_items, base_dir) = {
            let Some(unit) = self.modules.get(&id) else {
                return;
            };
            let base_dir = unit.path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
            let imports: Vec<ImportDecl> = unit
                .program
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Import(decl) => Some(decl.clone()),
                    _ => None,
                })
                .collect();
            (imports, HashMap::new(), base_dir)
        };

        let mut module_map = ModuleMap::default();
        let mut import_items = import_items;

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
                    let module_id = self
                        .load_std_module(&path.value, span)
                        .or_else(|| {
                            let path = self.resolve_path(&base_dir, &path.value, path.span);
                            self.load_module(&path, None, span)
                        });
                    if let Some(module_id) = module_id {
                        self.insert_module_alias(&mut module_map, &name.name, module_id);
                    }
                }
                ImportSpec::AliasFrom { alias, path, .. } => {
                    let module_id = self
                        .load_std_module(&path.value, span)
                        .or_else(|| {
                            let path = self.resolve_path(&base_dir, &path.value, path.span);
                            self.load_module(&path, None, span)
                        });
                    if let Some(module_id) = module_id {
                        self.insert_module_alias(&mut module_map, &alias.name, module_id);
                    }
                }
                ImportSpec::NamedFrom { names, path } => {
                    let module_id = self
                        .load_std_module(&path.value, span)
                        .or_else(|| {
                            let path = self.resolve_path(&base_dir, &path.value, path.span);
                            self.load_module(&path, None, span)
                        });
                    let Some(module_id) = module_id else {
                        continue;
                    };
                    let exports = self.modules.get(&module_id).map(|unit| &unit.exports);
                    for name in names {
                        if import_items.contains_key(&name.name) {
                            continue;
                        }
                        let Some(exports) = exports else { continue };
                        if !exports.contains(&name.name) {
                            self.diags.error(
                                name.span,
                                format!("unknown import {} in {}", name.name, path.value),
                            );
                            continue;
                        }
                        import_items.insert(name.name.clone(), self.link_for(module_id));
                    }
                }
            }
        }

        if let Some(unit) = self.modules.get_mut(&id) {
            unit.modules = module_map;
            unit.import_items = import_items;
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
            let fields =
                self.resolve_derived_fields(module_id, &name, &mut cache, &mut visiting);
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
            self.diags
                .error(Span::default(), format!("cyclic type derivation for {name}"));
            return None;
        }
        visiting.insert(key.clone());

        let decl = match self.find_type_decl(module_id, name) {
            Some(decl) => decl,
            None => {
                self.diags
                    .error(Span::default(), format!("unknown type {name}"));
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
                self.diags.error(
                    derive.base.span,
                    format!("unknown base type {}", derive.base.name),
                );
                return None;
            }
        };
        let base_fields =
            self.resolve_derived_fields(base_module, &base_name, cache, visiting)?;

        let mut removed = HashSet::new();
        for field in &derive.without {
            removed.insert(field.name.clone());
        }

        for field in &derive.without {
            if !base_fields.iter().any(|f| f.name.name == field.name) {
                self.diags.error(
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

    fn resolve_type_target(&self, module_id: ModuleId, base: &crate::ast::Ident) -> Option<(ModuleId, String)> {
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
        map.modules.insert(alias.to_string(), self.link_for(module_id));
    }

    fn link_for(&self, module_id: ModuleId) -> ModuleLink {
        let exports = self
            .modules
            .get(&module_id)
            .map(|unit| unit.exports.clone())
            .unwrap_or_default();
        ModuleLink { id: module_id, exports }
    }

    fn register_global_exports(&mut self, module_id: ModuleId) {
        let Some(unit) = self.modules.get(&module_id) else {
            return;
        };
        if unit.path.to_string_lossy() == "<std.Error>" {
            return;
        }
        for item in &unit.program.items {
            let (name, span) = match item {
                Item::Type(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::Enum(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::Fn(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::Config(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::Service(decl) => (decl.name.name.as_str(), decl.name.span),
                Item::App(decl) => (decl.name.value.as_str(), decl.name.span),
                _ => continue,
            };
            if let Some((prev_id, prev_span)) = self.global_names.get(name) {
                if *prev_id != module_id {
                    let path = unit.path.clone();
                    self.diags
                        .error_at_path(path.clone(), span, format!("duplicate symbol: {name}"));
                    if let Some(prev_unit) = self.modules.get(prev_id) {
                        self.diags.error_at_path(
                            prev_unit.path.clone(),
                            *prev_span,
                            format!("previous definition of {name} here"),
                        );
                    } else {
                        self.diags.error(span, format!("previous definition of {name} here"));
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
                    self.diags
                        .error(span, "dependency imports require dep:<name>/<path>");
                    return base_dir.join(raw);
                }
            };
            let Some(root) = self.deps.get(dep) else {
                self.diags
                    .error(span, format!("unknown dependency {dep}"));
                return base_dir.join(raw);
            };
            let mut path = root.join(rel);
            if path.extension().is_none() {
                path.set_extension("fuse");
            }
            return path;
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

    fn load_std_module(&mut self, name: &str, span: Span) -> Option<ModuleId> {
        if name != "std.Error" {
            return None;
        }
        let path = PathBuf::from("<std.Error>");
        if let Some(id) = self.by_path.get(&path) {
            return Some(*id);
        }
        self.load_module(&path, Some(STD_ERROR_MODULE), span)
    }

    fn normalize_path(&self, path: &Path) -> PathBuf {
        if let Ok(canon) = path.canonicalize() {
            canon
        } else {
            path.to_path_buf()
        }
    }
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
