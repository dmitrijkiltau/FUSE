use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{ImportDecl, ImportSpec, Item, Program};
use crate::diag::{Diag, Diagnostics};
use crate::parse_source;
use crate::span::Span;

#[derive(Clone, Debug, Default)]
pub struct ModuleMap {
    pub modules: HashMap<String, ModuleExports>,
}

impl ModuleMap {
    pub fn get(&self, name: &str) -> Option<&ModuleExports> {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IncludeMode {
    All,
    Names,
}

pub fn load_program_with_modules(path: &Path, src: &str) -> (Program, ModuleMap, Vec<Diag>) {
    let mut loader = ModuleLoader::new();
    let root = loader.insert_root(path, src);
    let mut items = Vec::new();
    if let Some(root) = root {
        loader.include_root(&root, &mut items);
    }
    (
        Program { items },
        loader.module_map,
        loader.diags.into_vec(),
    )
}

pub fn load_program(path: &Path, src: &str) -> (Program, Vec<Diag>) {
    let (program, _modules, diags) = load_program_with_modules(path, src);
    (program, diags)
}

struct ModuleLoader {
    loaded: HashMap<PathBuf, Program>,
    included_all: HashSet<PathBuf>,
    included_names: HashMap<PathBuf, HashSet<String>>,
    visiting: HashSet<PathBuf>,
    module_exports: HashMap<PathBuf, ModuleExports>,
    module_aliases: HashMap<String, PathBuf>,
    module_map: ModuleMap,
    diags: Diagnostics,
}

impl ModuleLoader {
    fn new() -> Self {
        Self {
            loaded: HashMap::new(),
            included_all: HashSet::new(),
            included_names: HashMap::new(),
            visiting: HashSet::new(),
            module_exports: HashMap::new(),
            module_aliases: HashMap::new(),
            module_map: ModuleMap::default(),
            diags: Diagnostics::default(),
        }
    }

    fn insert_root(&mut self, path: &Path, src: &str) -> Option<PathBuf> {
        let key = self.normalize_path(path);
        let (program, diags) = parse_source(src);
        self.diags.extend(diags);
        self.loaded.insert(key.clone(), program);
        Some(key)
    }

    fn include_root(&mut self, root: &PathBuf, items: &mut Vec<Item>) {
        let Some(program) = self.loaded.get(root).cloned() else {
            return;
        };
        for item in &program.items {
            if matches!(item, Item::Import(_)) {
                continue;
            }
            items.push(item.clone());
        }
        self.included_all.insert(root.clone());
        for import in program.items.iter().filter_map(|item| match item {
            Item::Import(decl) => Some(decl.clone()),
            _ => None,
        }) {
            self.include_import(root, &import, items);
        }
    }

    fn include_import(&mut self, from: &PathBuf, import: &ImportDecl, items: &mut Vec<Item>) {
        let (path, mode, names, alias, span) = match self.resolve_import(from, import) {
            Some(value) => value,
            None => return,
        };
        let Some(path) = self.load_module(&path, span) else {
            return;
        };
        if let Some(alias) = alias {
            self.register_module_alias(alias, &path, span);
        }
        self.include_module(&path, mode, names, span, items);
    }

    fn include_module(
        &mut self,
        path: &PathBuf,
        mode: IncludeMode,
        names: Vec<String>,
        span: Span,
        items: &mut Vec<Item>,
    ) {
        if self.included_all.contains(path) {
            return;
        }
        if self.visiting.contains(path) {
            return;
        }
        self.visiting.insert(path.clone());

        let Some(program) = self.loaded.get(path).cloned() else {
            self.visiting.remove(path);
            return;
        };

        for import in program.items.iter().filter_map(|item| match item {
            Item::Import(decl) => Some(decl.clone()),
            _ => None,
        }) {
            self.include_import(path, &import, items);
        }

        match mode {
            IncludeMode::All => {
                let existing = self
                    .included_names
                    .entry(path.clone())
                    .or_insert_with(HashSet::new);
                for item in &program.items {
                    if matches!(item, Item::Import(_)) {
                        continue;
                    }
                    if let Some(name) = item_ident_name(item) {
                        if existing.contains(name) {
                            continue;
                        }
                        existing.insert(name.to_string());
                    }
                    items.push(item.clone());
                }
                self.included_all.insert(path.clone());
            }
            IncludeMode::Names => {
                let available = module_item_names(&program);
                for name in &names {
                    if !available.contains(name.as_str()) {
                        self.diags.error(
                            span,
                            format!("unknown import {name} in {}", path.display()),
                        );
                    }
                }
                let entry = self
                    .included_names
                    .entry(path.clone())
                    .or_insert_with(HashSet::new);
                for item in &program.items {
                    let Some(name) = item_ident_name(item) else {
                        continue;
                    };
                    if !names.iter().any(|want| want == name) {
                        continue;
                    }
                    if !entry.insert(name.to_string()) {
                        continue;
                    }
                    items.push(item.clone());
                }
            }
        }

        self.visiting.remove(path);
    }

    fn resolve_import(
        &mut self,
        from: &PathBuf,
        import: &ImportDecl,
    ) -> Option<(PathBuf, IncludeMode, Vec<String>, Option<String>, Span)> {
        let span = import.span;
        let base_dir = from.parent().unwrap_or_else(|| Path::new("."));
        match &import.spec {
            ImportSpec::Module { name } => {
                let mut path = base_dir.join(&name.name);
                if path.extension().is_none() {
                    path.set_extension("fuse");
                }
                Some((
                    path,
                    IncludeMode::All,
                    Vec::new(),
                    Some(name.name.clone()),
                    span,
                ))
            }
            ImportSpec::ModuleFrom { name, path } => {
                let path = self.resolve_path(base_dir, &path.value);
                Some((
                    path,
                    IncludeMode::All,
                    Vec::new(),
                    Some(name.name.clone()),
                    span,
                ))
            }
            ImportSpec::AliasFrom { alias, path, .. } => {
                let path = self.resolve_path(base_dir, &path.value);
                Some((
                    path,
                    IncludeMode::All,
                    Vec::new(),
                    Some(alias.name.clone()),
                    span,
                ))
            }
            ImportSpec::NamedFrom { names, path } => {
                let path = self.resolve_path(base_dir, &path.value);
                let names = names.iter().map(|name| name.name.clone()).collect();
                Some((path, IncludeMode::Names, names, None, span))
            }
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

    fn load_module(&mut self, path: &PathBuf, span: Span) -> Option<PathBuf> {
        let key = self.normalize_path(path);
        if self.loaded.contains_key(&key) {
            return Some(key);
        }
        let src = match fs::read_to_string(&key) {
            Ok(src) => src,
            Err(err) => {
                self.diags
                    .error(span, format!("failed to read module {}: {err}", key.display()));
                return None;
            }
        };
        let (program, diags) = parse_source(&src);
        self.diags.extend(diags);
        self.loaded.insert(key.clone(), program);
        Some(key)
    }

    fn normalize_path(&self, path: &Path) -> PathBuf {
        if let Ok(canon) = path.canonicalize() {
            canon
        } else {
            path.to_path_buf()
        }
    }

    fn module_exports_for(&mut self, path: &PathBuf) -> Option<ModuleExports> {
        if let Some(exports) = self.module_exports.get(path) {
            return Some(exports.clone());
        }
        let program = self.loaded.get(path)?;
        let exports = ModuleExports::from_program(program);
        self.module_exports.insert(path.clone(), exports.clone());
        Some(exports)
    }

    fn register_module_alias(&mut self, alias: String, path: &PathBuf, span: Span) {
        if let Some(existing) = self.module_aliases.get(&alias) {
            if existing != path {
                self.diags.error(
                    span,
                    format!(
                        "module alias {alias} already used for {}",
                        existing.display()
                    ),
                );
            }
            return;
        }
        let Some(exports) = self.module_exports_for(path) else {
            self.diags
                .error(span, format!("unknown module {}", path.display()));
            return;
        };
        self.module_aliases.insert(alias.clone(), path.clone());
        self.module_map.modules.insert(alias, exports);
    }
}

fn module_item_names(program: &Program) -> HashSet<&str> {
    program
        .items
        .iter()
        .filter_map(item_ident_name)
        .collect()
}

fn item_ident_name(item: &Item) -> Option<&str> {
    match item {
        Item::Type(decl) => Some(decl.name.name.as_str()),
        Item::Enum(decl) => Some(decl.name.name.as_str()),
        Item::Fn(decl) => Some(decl.name.name.as_str()),
        Item::Service(decl) => Some(decl.name.name.as_str()),
        Item::Config(decl) => Some(decl.name.name.as_str()),
        _ => None,
    }
}
