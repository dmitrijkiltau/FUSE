//! Canonical `fuse.toml` manifest parser shared by the CLI and LSP.
//!
//! Supports the same three dependency syntaxes as the original line-scanner:
//!
//! ```toml
//! [dependencies]
//! Auth = "./deps/auth"                         # bare path string
//! Math = { path = "./deps/math" }              # inline table
//!
//! [dependencies.Storage]
//! path = "./deps/storage"                      # section table
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Parsed contents of a single `fuse.toml` manifest.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    /// Path to the package entry file (`[package].entry`), resolved relative
    /// to the directory that contains `fuse.toml`.
    pub entry: Option<PathBuf>,
    /// Dependency name → resolved absolute (or canonicalized) root path.
    pub deps: HashMap<String, PathBuf>,
}

/// Parse the `fuse.toml` manifest located in `manifest_dir`.
///
/// Returns `None` if the file does not exist or cannot be read.  Never panics.
pub fn parse_manifest(manifest_dir: &Path) -> Option<Manifest> {
    let manifest_path = manifest_dir.join("fuse.toml");
    let contents = std::fs::read_to_string(&manifest_path).ok()?;
    Some(parse_manifest_contents(manifest_dir, &contents))
}

/// Parse manifest content from a string (useful for testing).
pub fn parse_manifest_contents(manifest_dir: &Path, contents: &str) -> Manifest {
    let mut manifest = Manifest::default();
    let mut in_package = false;
    let mut in_dependencies = false;
    let mut current_dep_table: Option<String> = None;

    for raw_line in contents.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        // Section header: `[foo]` or `[foo.bar]`
        if line.starts_with('[') && line.ends_with(']') {
            in_package = false;
            in_dependencies = false;
            current_dep_table = None;

            let header = line[1..line.len() - 1].trim();
            match header {
                "package" => in_package = true,
                "dependencies" => in_dependencies = true,
                _ => {
                    if let Some(dep_name) = header.strip_prefix("dependencies.") {
                        let dep_name = unquote_toml_key(dep_name.trim());
                        if !dep_name.is_empty() {
                            current_dep_table = Some(dep_name);
                        }
                    }
                }
            }
            continue;
        }

        let Some((key, value)) = split_toml_assignment(line) else {
            continue;
        };

        // [package] section: look for `entry = "..."`.
        if in_package && unquote_toml_key(key) == "entry" {
            if let Some(entry_str) = parse_toml_string(value) {
                if !entry_str.is_empty() {
                    manifest.entry = Some(manifest_dir.join(&entry_str));
                }
            }
            continue;
        }

        // [dependencies.DepName] section table: look for `path = "..."`.
        if let Some(dep_name) = current_dep_table.as_deref() {
            if unquote_toml_key(key) == "path" {
                if let Some(path_str) = parse_toml_string(value) {
                    manifest.deps.insert(
                        dep_name.to_string(),
                        resolve_dep_path(manifest_dir, &path_str),
                    );
                }
            }
            continue;
        }

        // [dependencies] inline: `DepName = "..." | { path = "..." }`.
        if in_dependencies {
            let dep_name = unquote_toml_key(key);
            if dep_name.is_empty() {
                continue;
            }
            if let Some(path_str) = resolve_dependency_path_value(value) {
                manifest
                    .deps
                    .insert(dep_name, resolve_dep_path(manifest_dir, &path_str));
            }
        }
    }

    manifest
}

/// Starting from `initial_deps`, transitively expand each dependency's own
/// `fuse.toml` to produce a flattened map of all reachable packages.
///
/// - Sub-dependency names do NOT override a name that already exists in the
///   current map (the consumer's direct deps take precedence).
/// - Cross-package dependency cycles are detected and reported as error strings.
///
/// Returns `(merged_deps, cycle_errors)`.
pub fn build_transitive_deps(
    initial_deps: &HashMap<String, PathBuf>,
) -> (HashMap<String, PathBuf>, Vec<String>) {
    let mut merged: HashMap<String, PathBuf> = initial_deps.clone();
    let mut errors: Vec<String> = Vec::new();

    // Queue of (dep_root_path, dep_chain_for_cycle_detection).
    let mut queue: Vec<(PathBuf, Vec<String>)> = initial_deps
        .iter()
        .map(|(name, root)| (root.clone(), vec![name.clone()]))
        .collect();

    let mut visited_roots: HashSet<PathBuf> = HashSet::new();

    while let Some((dep_root, chain)) = queue.pop() {
        let canon = canonicalize_or_keep(&dep_root);
        if !visited_roots.insert(canon.clone()) {
            continue; // already fully processed this root
        }

        let Some(sub_manifest) = parse_manifest(&dep_root) else {
            continue; // no fuse.toml in this dep — that's fine
        };

        for (sub_name, sub_root) in sub_manifest.deps {
            let sub_canon = canonicalize_or_keep(&sub_root);

            // Cycle detection: is this sub-dep root already in the active chain?
            let chain_canons: Vec<PathBuf> = chain
                .iter()
                .filter_map(|n| merged.get(n))
                .map(|r| canonicalize_or_keep(r))
                .collect();
            if chain_canons.contains(&sub_canon) {
                let cycle_path = format!("{} → {}", chain.join(" → "), sub_name);
                errors.push(format!("circular package dependency: {cycle_path}"));
                continue;
            }

            // Only add a new entry; direct deps take precedence over transitive.
            if !merged.contains_key(&sub_name) {
                merged.insert(sub_name.clone(), sub_root.clone());
                let mut sub_chain = chain.clone();
                sub_chain.push(sub_name);
                queue.push((sub_root, sub_chain));
            }
        }
    }

    (merged, errors)
}

/// Walk up from the directory containing `entry` to find a `fuse.toml`
/// (same logic as `workspace_root_for_entry` in `loader.rs`).
/// Falls back to the entry's parent directory if no manifest is found.
pub fn find_workspace_root_for_entry(entry: &Path) -> PathBuf {
    let start = if entry.is_dir() {
        entry.to_path_buf()
    } else {
        entry
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    };
    for ancestor in start.ancestors() {
        if ancestor.join("fuse.toml").exists() {
            return ancestor.to_path_buf();
        }
    }
    start
}

/// Walk `root` recursively, collecting the directory path of every
/// `fuse.toml` found (excluding `target/`, `.git/`, and other hidden dirs).
///
/// The returned list is sorted for deterministic ordering.
pub fn find_workspace_manifests(root: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    collect_manifests(root, &mut results);
    results.sort();
    results
}

fn collect_manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut subdirs = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden directories, target/, and common build artefact dirs.
        if path.is_dir() {
            if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
                continue;
            }
            subdirs.push(path);
        } else if name_str == "fuse.toml" {
            if let Some(parent) = path.parent() {
                out.push(parent.to_path_buf());
            }
        }
    }

    for subdir in subdirs {
        collect_manifests(&subdir, out);
    }
}

// ─── Internal TOML mini-parser helpers ───────────────────────────────────────

fn resolve_dependency_path_value(value: &str) -> Option<String> {
    let value = value.trim();
    // Bare string path
    if let Some(path) = parse_toml_string(value) {
        if looks_like_path_dependency(&path) {
            return Some(path);
        }
        return None;
    }
    // Inline table: { path = "..." }
    if value.starts_with('{') && value.ends_with('}') {
        let inner = &value[1..value.len() - 1];
        for part in inner.split(',') {
            let Some((k, v)) = split_toml_assignment(part) else {
                continue;
            };
            if unquote_toml_key(k) == "path" {
                return parse_toml_string(v);
            }
        }
    }
    None
}

fn looks_like_path_dependency(path: &str) -> bool {
    path.starts_with('.') || path.starts_with('/')
}

fn resolve_dep_path(manifest_dir: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    let joined = if path.is_absolute() {
        path
    } else {
        manifest_dir.join(path)
    };
    canonicalize_or_keep(&joined)
}

pub(crate) fn canonicalize_or_keep(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
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

pub(crate) fn parse_toml_string(value: &str) -> Option<String> {
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

pub(crate) fn unquote_toml_key(key: &str) -> String {
    let key = key.trim();
    if key.len() >= 2
        && ((key.starts_with('"') && key.ends_with('"'))
            || (key.starts_with('\'') && key.ends_with('\'')))
    {
        key[1..key.len() - 1].to_string()
    } else {
        key.to_string()
    }
}

pub(crate) fn strip_toml_comment(line: &str) -> &str {
    if let Some(idx) = line.find('#') {
        &line[..idx]
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_bare_string_dep() {
        let contents = r#"
[package]
entry = "src/main.fuse"

[dependencies]
Auth = "./deps/auth"
"#;
        let manifest = parse_manifest_contents(Path::new("/project"), contents);
        assert_eq!(
            manifest.entry,
            Some(PathBuf::from("/project/src/main.fuse"))
        );
        assert_eq!(
            manifest.deps.get("Auth"),
            Some(&PathBuf::from("/project/deps/auth"))
        );
    }

    #[test]
    fn parse_inline_table_dep() {
        let contents = r#"
[dependencies]
Math = { path = "./deps/math" }
"#;
        let manifest = parse_manifest_contents(Path::new("/project"), contents);
        assert_eq!(
            manifest.deps.get("Math"),
            Some(&PathBuf::from("/project/deps/math"))
        );
    }

    #[test]
    fn parse_section_table_dep() {
        let contents = r#"
[dependencies.Storage]
path = "./deps/storage"
"#;
        let manifest = parse_manifest_contents(Path::new("/project"), contents);
        assert_eq!(
            manifest.deps.get("Storage"),
            Some(&PathBuf::from("/project/deps/storage"))
        );
    }

    #[test]
    fn parse_all_three_dep_syntaxes() {
        let contents = r#"
[dependencies]
Auth = "./deps/auth"
Math = { path = "./deps/math" }

[dependencies.Storage]
path = "./deps/storage"
"#;
        let manifest = parse_manifest_contents(Path::new("/project"), contents);
        assert_eq!(manifest.deps.len(), 3);
        assert!(manifest.deps.contains_key("Auth"));
        assert!(manifest.deps.contains_key("Math"));
        assert!(manifest.deps.contains_key("Storage"));
    }

    #[test]
    fn comments_are_stripped() {
        let contents = r#"
[package]
entry = "main.fuse" # the entry point

[dependencies]
# a comment
Auth = "./deps/auth" # inline comment
"#;
        let manifest = parse_manifest_contents(Path::new("/project"), contents);
        assert!(manifest.entry.is_some());
        assert!(manifest.deps.contains_key("Auth"));
    }
}
