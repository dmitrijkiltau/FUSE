use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{ImportDecl, ImportSpec, Item, Program};
use crate::diag::{Diag, Diagnostics};
use crate::parse_source;
use crate::span::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IncludeMode {
    All,
    Names,
}

pub fn load_program(path: &Path, src: &str) -> (Program, Vec<Diag>) {
    let mut loader = ModuleLoader::new();
    let root = loader.insert_root(path, src);
    let mut items = Vec::new();
    if let Some(root) = root {
        loader.include_root(&root, &mut items);
    }
    (Program { items }, loader.diags.into_vec())
}

struct ModuleLoader {
    loaded: HashMap<PathBuf, Program>,
    included_all: HashSet<PathBuf>,
    included_names: HashMap<PathBuf, HashSet<String>>,
    visiting: HashSet<PathBuf>,
    diags: Diagnostics,
}

impl ModuleLoader {
    fn new() -> Self {
        Self {
            loaded: HashMap::new(),
            included_all: HashSet::new(),
            included_names: HashMap::new(),
            visiting: HashSet::new(),
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
        let (path, mode, names, span) = match self.resolve_import(from, import) {
            Some(value) => value,
            None => return,
        };
        let Some(path) = self.load_module(&path, span) else {
            return;
        };
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
    ) -> Option<(PathBuf, IncludeMode, Vec<String>, Span)> {
        let span = import.span;
        let base_dir = from.parent().unwrap_or_else(|| Path::new("."));
        match &import.spec {
            ImportSpec::Module { name } => {
                let mut path = base_dir.join(&name.name);
                if path.extension().is_none() {
                    path.set_extension("fuse");
                }
                Some((path, IncludeMode::All, Vec::new(), span))
            }
            ImportSpec::ModuleFrom { path, .. } | ImportSpec::AliasFrom { path, .. } => {
                let path = self.resolve_path(base_dir, &path.value);
                Some((path, IncludeMode::All, Vec::new(), span))
            }
            ImportSpec::NamedFrom { names, path } => {
                let path = self.resolve_path(base_dir, &path.value);
                let names = names.iter().map(|name| name.name.clone()).collect();
                Some((path, IncludeMode::Names, names, span))
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
