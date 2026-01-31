use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{ImportDecl, ImportSpec, Item, Program};
use crate::diag::{Diag, Diagnostics};
use crate::parse_source;
use crate::span::Span;

pub type ModuleId = usize;

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
    let mut loader = ModuleLoader::new();
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
    diags: Diagnostics,
    global_names: HashMap<String, (ModuleId, Span)>,
}

impl ModuleLoader {
    fn new() -> Self {
        Self {
            next_id: 1,
            by_path: HashMap::new(),
            modules: HashMap::new(),
            visiting: HashSet::new(),
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
        let (program, diags) = parse_source(&src);
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
                    let path = self.resolve_path(&base_dir, &path.value);
                    if let Some(module_id) = self.load_module(&path, None, span) {
                        self.insert_module_alias(&mut module_map, &name.name, module_id);
                    }
                }
                ImportSpec::AliasFrom { alias, path, .. } => {
                    let path = self.resolve_path(&base_dir, &path.value);
                    if let Some(module_id) = self.load_module(&path, None, span) {
                        self.insert_module_alias(&mut module_map, &alias.name, module_id);
                    }
                }
                ImportSpec::NamedFrom { names, path } => {
                    let path = self.resolve_path(&base_dir, &path.value);
                    let Some(module_id) = self.load_module(&path, None, span) else {
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
                                format!("unknown import {} in {}", name.name, path.display()),
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
                    self.diags.error(span, format!("duplicate symbol: {name}"));
                    self.diags
                        .error(*prev_span, format!("previous definition of {name} here"));
                }
                continue;
            }
            self.global_names
                .insert(name.to_string(), (module_id, span));
        }
    }

    fn resolve_path(&self, base_dir: &Path, raw: &str) -> PathBuf {
        let mut path = PathBuf::from(raw);
        if path.extension().is_none() {
            path.set_extension("fuse");
        }
        if path.is_relative() {
            path = base_dir.join(path);
        }
        path
    }

    fn normalize_path(&self, path: &Path) -> PathBuf {
        if let Ok(canon) = path.canonicalize() {
            canon
        } else {
            path.to_path_buf()
        }
    }
}
