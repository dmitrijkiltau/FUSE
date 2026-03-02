use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use serde::{Deserialize, Serialize};

use super::{DependencyDetail, DependencySpec, Manifest};

#[derive(Debug, Serialize, Deserialize, Default)]
struct Lockfile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    dependencies: BTreeMap<String, LockedDependency>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct LockedDependency {
    source: String,
    git: Option<String>,
    rev: Option<String>,
    path: Option<String>,
    subdir: Option<String>,
    requested: Option<String>,
}

struct NormalizedDependency {
    requested: String,
    kind: NormalizedKind,
}

enum NormalizedKind {
    Path {
        path: PathBuf,
    },
    Git {
        git: String,
        reference: GitReference,
    },
}

struct GitReference {
    requested: GitRequest,
    subdir: Option<String>,
}

enum GitRequest {
    Rev(String),
    Tag(String),
    Branch(String),
    Version(String),
    Head,
}

impl GitReference {
    fn descriptor(&self) -> String {
        match &self.requested {
            GitRequest::Rev(value) => format!("rev:{value}"),
            GitRequest::Tag(value) => format!("tag:{value}"),
            GitRequest::Branch(value) => format!("branch:{value}"),
            GitRequest::Version(value) => format!("version:{value}"),
            GitRequest::Head => "head".to_string(),
        }
    }
}

pub fn resolve_dependencies(
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
) -> Result<HashMap<String, PathBuf>, String> {
    let Some(manifest) = manifest else {
        return Ok(HashMap::new());
    };
    if manifest.dependencies.is_empty() {
        return Ok(HashMap::new());
    }
    let Some(root_dir) = manifest_dir else {
        return Err(dep_error(
            "FUSE_DEP_MANIFEST_DIR_REQUIRED",
            "dependencies require a manifest directory",
        ));
    };
    let lock_path = root_dir.join("fuse.lock");
    let mut lock = load_lockfile(&lock_path)?;
    let deps_dir = root_dir.join(".fuse").join("deps");
    if !deps_dir.exists() {
        fs::create_dir_all(&deps_dir).map_err(|err| {
            dep_error(
                "FUSE_DEP_CACHE_DIR_CREATE_FAILED",
                format!("failed to create {}: {err}", deps_dir.display()),
            )
        })?;
    }

    let mut resolved = HashMap::new();
    let mut requests: HashMap<String, (String, PathBuf)> = HashMap::new();
    let mut queue: VecDeque<(String, DependencySpec, PathBuf)> = VecDeque::new();
    for (name, spec) in &manifest.dependencies {
        queue.push_back((name.clone(), spec.clone(), root_dir.to_path_buf()));
    }

    while let Some((name, spec, base_dir)) = queue.pop_front() {
        let requested = dependency_request_key(&name, &spec, &base_dir)?;
        if let Some((prev, prev_base_dir)) = requests.get(&name) {
            if prev != &requested {
                let prev_from = prev_base_dir.join("fuse.toml");
                let next_from = base_dir.join("fuse.toml");
                return Err(dep_error(
                    "FUSE_DEP_CONFLICTING_SPECS",
                    format!(
                        "dependency {name} requested with conflicting specs:\n  - {prev} (from {})\n  - {requested} (from {})",
                        prev_from.display(),
                        next_from.display()
                    ),
                ));
            }
        } else {
            requests.insert(name.clone(), (requested.clone(), base_dir.clone()));
        }
        if resolved.contains_key(&name) {
            continue;
        }
        let root = resolve_dependency(&name, &spec, &base_dir, root_dir, &deps_dir, &mut lock)?;
        resolved.insert(name.clone(), root.clone());

        if let Some(dep_manifest) = load_manifest_from_dir(&root).map_err(|err| {
            dep_error(
                "FUSE_DEP_MANIFEST_LOAD_FAILED",
                format!(
                    "failed to load dependency manifest from {}: {err}",
                    root.display()
                ),
            )
        })? {
            for (dep_name, dep_spec) in dep_manifest.dependencies {
                queue.push_back((dep_name, dep_spec, root.clone()));
            }
        }
    }

    lock.version = 1;
    write_lockfile(&lock_path, &lock)?;
    Ok(resolved)
}

fn dep_error(code: &str, message: impl Into<String>) -> String {
    format!("[{code}] {}", message.into())
}

fn dep_error_with_hint(code: &str, message: impl Into<String>, hint: &str) -> String {
    format!("[{code}] {}. Hint: {hint}", message.into())
}

fn lockfile_remediation_hint() -> &'static str {
    "delete fuse.lock and rerun 'fuse build' (or run 'fuse build --clean')"
}

fn lock_error(code: &str, message: impl Into<String>) -> String {
    dep_error_with_hint(code, message, lockfile_remediation_hint())
}

fn dependency_request_key(
    name: &str,
    spec: &DependencySpec,
    base_dir: &Path,
) -> Result<String, String> {
    let normalized = normalize_dependency_spec(name, spec, base_dir)?;
    Ok(normalized.requested)
}

fn resolve_dependency(
    name: &str,
    spec: &DependencySpec,
    base_dir: &Path,
    root_dir: &Path,
    deps_dir: &Path,
    lock: &mut Lockfile,
) -> Result<PathBuf, String> {
    let normalized = normalize_dependency_spec(name, spec, base_dir)?;
    if let Some(entry) = lock.dependencies.get(name) {
        if entry.requested.as_deref() == Some(normalized.requested.as_str()) {
            return root_from_lock(name, entry, root_dir, deps_dir);
        }
    }

    let (root, entry) = match normalized.kind {
        NormalizedKind::Path { path } => {
            if !path.exists() {
                return Err(dep_error_with_hint(
                    "FUSE_DEP_PATH_NOT_FOUND",
                    format!("dependency {name} path does not exist: {}", path.display()),
                    "fix the dependency path in fuse.toml",
                ));
            }
            let stored_path = store_path(root_dir, &path);
            (
                path,
                LockedDependency {
                    source: "path".to_string(),
                    git: None,
                    rev: None,
                    path: Some(stored_path),
                    subdir: None,
                    requested: Some(normalized.requested),
                },
            )
        }
        NormalizedKind::Git { git, reference } => {
            let rev = resolve_git_revision(&git, &reference)?;
            let checkout = deps_dir.join(name).join(&rev);
            ensure_checkout(&git, &rev, &checkout)?;
            let root = if let Some(subdir) = &reference.subdir {
                checkout.join(subdir)
            } else {
                checkout
            };
            if !root.exists() {
                return Err(dep_error_with_hint(
                    "FUSE_DEP_SUBDIR_NOT_FOUND",
                    format!(
                        "dependency {name} subdir does not exist: {}",
                        root.display()
                    ),
                    "fix the dependency subdir in fuse.toml",
                ));
            }
            (
                root,
                LockedDependency {
                    source: "git".to_string(),
                    git: Some(git),
                    rev: Some(rev),
                    path: None,
                    subdir: reference.subdir,
                    requested: Some(normalized.requested),
                },
            )
        }
    };

    lock.dependencies.insert(name.to_string(), entry);
    Ok(root)
}

fn root_from_lock(
    name: &str,
    entry: &LockedDependency,
    root_dir: &Path,
    deps_dir: &Path,
) -> Result<PathBuf, String> {
    match entry.source.as_str() {
        "path" => {
            let Some(path) = &entry.path else {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_MISSING_PATH",
                    format!("lock entry for {name} missing path"),
                ));
            };
            let path = PathBuf::from(path);
            let resolved = if path.is_absolute() {
                path
            } else {
                root_dir.join(path)
            };
            if !resolved.exists() {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_PATH_NOT_FOUND",
                    format!(
                        "lock entry for {name} points to missing path: {}",
                        resolved.display()
                    ),
                ));
            }
            Ok(resolved)
        }
        "git" => {
            let Some(rev) = &entry.rev else {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_MISSING_REV",
                    format!("lock entry for {name} missing rev"),
                ));
            };
            let Some(git) = &entry.git else {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_MISSING_GIT",
                    format!("lock entry for {name} missing git url"),
                ));
            };
            let base = deps_dir.join(name).join(rev);
            if !base.exists() {
                ensure_checkout(git, rev, &base)?;
            }
            let mut root = base;
            if let Some(subdir) = &entry.subdir {
                root = root.join(subdir);
            }
            if !root.exists() {
                return Err(lock_error(
                    "FUSE_LOCK_ENTRY_SUBDIR_NOT_FOUND",
                    format!(
                        "lock entry for {name} points to missing git path: {}",
                        root.display()
                    ),
                ));
            }
            Ok(root)
        }
        other => Err(lock_error(
            "FUSE_LOCK_ENTRY_UNKNOWN_SOURCE",
            format!("unknown lock source {other} for {name}"),
        )),
    }
}

fn normalize_dependency_spec(
    name: &str,
    spec: &DependencySpec,
    base_dir: &Path,
) -> Result<NormalizedDependency, String> {
    let detail = match spec {
        DependencySpec::Simple(value) => {
            if looks_like_git_url(value) {
                DependencyDetail {
                    git: Some(value.clone()),
                    ..DependencyDetail::default()
                }
            } else if looks_like_path(value) {
                DependencyDetail {
                    path: Some(value.clone()),
                    ..DependencyDetail::default()
                }
            } else {
                return Err(dep_error(
                    "FUSE_DEP_INVALID_SOURCE",
                    format!(
                        "dependency {name} has invalid source {value:?}; use a relative/absolute path or {{ git = \"...\" }}"
                    ),
                ));
            }
        }
        DependencySpec::Detailed(detail) => detail.clone(),
    };

    if let Some(path) = detail.path {
        if path.trim().is_empty() {
            return Err(dep_error(
                "FUSE_DEP_PATH_EMPTY",
                format!("dependency {name} path cannot be empty"),
            ));
        }
        if detail.git.is_some()
            || detail.version.is_some()
            || detail.rev.is_some()
            || detail.tag.is_some()
            || detail.branch.is_some()
            || detail.subdir.is_some()
        {
            return Err(dep_error(
                "FUSE_DEP_PATH_FIELDS_INVALID",
                format!(
                    "dependency {name} path dependencies cannot include git/rev/tag/branch/version/subdir fields"
                ),
            ));
        }
        let path = resolve_path(base_dir, &normalize_dependency_path_input(&path));
        let normalized_path = canonicalize_dependency_path_for_requested(&path);
        let requested = format!("path:{}", normalized_path.display());
        return Ok(NormalizedDependency {
            requested,
            kind: NormalizedKind::Path {
                path: normalized_path,
            },
        });
    }

    let has_ref = detail.rev.is_some()
        || detail.tag.is_some()
        || detail.branch.is_some()
        || detail.version.is_some();
    let Some(git) = detail.git else {
        if has_ref || detail.subdir.is_some() {
            return Err(dep_error(
                "FUSE_DEP_GIT_REQUIRED_FOR_REFS",
                format!(
                    "dependency {name} must specify git when using rev/tag/branch/version/subdir"
                ),
            ));
        }
        return Err(dep_error(
            "FUSE_DEP_SOURCE_REQUIRED",
            format!("dependency {name} must specify either path or git"),
        ));
    };
    if git.trim().is_empty() {
        return Err(dep_error(
            "FUSE_DEP_GIT_EMPTY",
            format!("dependency {name} git url cannot be empty"),
        ));
    }
    if detail.path.is_some() {
        return Err(dep_error(
            "FUSE_DEP_GIT_PATH_CONFLICT",
            format!("dependency {name} cannot specify both git and path"),
        ));
    }
    if let Some(subdir) = detail.subdir.as_ref() {
        if subdir.trim().is_empty() {
            return Err(dep_error(
                "FUSE_DEP_SUBDIR_EMPTY",
                format!("dependency {name} subdir cannot be empty"),
            ));
        }
    }

    let mut selected_refs = 0usize;
    if detail.rev.is_some() {
        selected_refs += 1;
    }
    if detail.tag.is_some() {
        selected_refs += 1;
    }
    if detail.branch.is_some() {
        selected_refs += 1;
    }
    if detail.version.is_some() {
        selected_refs += 1;
    }
    if selected_refs > 1 {
        return Err(dep_error(
            "FUSE_DEP_GIT_REF_CONFLICT",
            format!("dependency {name} must specify at most one of rev, tag, branch, version"),
        ));
    }

    let requested_ref = if let Some(rev) = detail.rev {
        GitRequest::Rev(rev)
    } else if let Some(tag) = detail.tag {
        GitRequest::Tag(tag)
    } else if let Some(branch) = detail.branch {
        GitRequest::Branch(branch)
    } else if let Some(version) = detail.version {
        GitRequest::Version(version)
    } else {
        GitRequest::Head
    };
    let reference = GitReference {
        requested: requested_ref,
        subdir: detail.subdir,
    };
    let mut requested = format!("git:{git}|{}", reference.descriptor());
    if let Some(subdir) = &reference.subdir {
        requested.push_str(&format!("|subdir:{subdir}"));
    }
    Ok(NormalizedDependency {
        requested,
        kind: NormalizedKind::Git { git, reference },
    })
}

fn resolve_git_revision(url: &str, reference: &GitReference) -> Result<String, String> {
    match &reference.requested {
        GitRequest::Rev(value) => Ok(value.clone()),
        GitRequest::Tag(tag) => resolve_git_tag(url, tag),
        GitRequest::Branch(branch) => resolve_git_branch(url, branch),
        GitRequest::Version(version) => resolve_git_version(url, version),
        GitRequest::Head => resolve_git_head(url),
    }
}

fn resolve_git_tag(url: &str, tag: &str) -> Result<String, String> {
    let ref_name = format!("refs/tags/{tag}");
    git_ls_remote(url, &ref_name)
}

fn resolve_git_branch(url: &str, branch: &str) -> Result<String, String> {
    let ref_name = format!("refs/heads/{branch}");
    git_ls_remote(url, &ref_name)
}

fn resolve_git_version(url: &str, version: &str) -> Result<String, String> {
    let tag = format!("v{version}");
    if let Ok(rev) = resolve_git_tag(url, &tag) {
        return Ok(rev);
    }
    resolve_git_tag(url, version)
}

fn resolve_git_head(url: &str) -> Result<String, String> {
    git_ls_remote(url, "HEAD")
}

fn git_ls_remote(url: &str, reference: &str) -> Result<String, String> {
    let output = run_git(&["ls-remote", url, reference], None)?;
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        if let Some(hash) = parts.next() {
            return Ok(hash.to_string());
        }
    }
    Err(dep_error(
        "FUSE_DEP_GIT_REF_RESOLVE_FAILED",
        format!("failed to resolve {reference} for {url}"),
    ))
}

fn ensure_checkout(url: &str, rev: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        if !dest.join(".git").exists() {
            return Err(dep_error_with_hint(
                "FUSE_DEP_CHECKOUT_NOT_GIT",
                format!("dependency checkout is not a git repo: {}", dest.display()),
                "remove .fuse/deps for this package and rerun the command",
            ));
        }
        let dest_str = dest.to_string_lossy();
        let _ = run_git(&["-C", dest_str.as_ref(), "fetch", "--tags"], None);
        run_git(&["-C", dest_str.as_ref(), "checkout", rev], None)?;
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            dep_error(
                "FUSE_DEP_CHECKOUT_DIR_CREATE_FAILED",
                format!("failed to create {}: {err}", parent.display()),
            )
        })?;
    }
    let dest_str = dest.to_string_lossy();
    run_git(&["clone", url, dest_str.as_ref()], None)?;
    run_git(&["-C", dest_str.as_ref(), "checkout", rev], None)?;
    Ok(())
}

fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = ProcessCommand::new("git");
    cmd.args(args).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.output().map_err(|err| {
        dep_error(
            "FUSE_DEP_GIT_COMMAND_START_FAILED",
            format!("failed to run git {:?}: {err}", args),
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(dep_error(
            "FUSE_DEP_GIT_COMMAND_FAILED",
            format!("git {:?} failed: {stderr}", args),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn looks_like_git_url(value: &str) -> bool {
    value.contains("://") || value.starts_with("git@") || value.ends_with(".git")
}

fn looks_like_path(value: &str) -> bool {
    value.starts_with('.') || value.starts_with('/') || value.contains('/') || value.contains('\\')
}

fn resolve_path(base_dir: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn normalize_dependency_path_input(raw: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        raw.to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        raw.replace('\\', "/")
    }
}

fn canonicalize_dependency_path_for_requested(path: &Path) -> PathBuf {
    if path.exists() {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn load_lockfile(path: &Path) -> Result<Lockfile, String> {
    if !path.exists() {
        return Ok(Lockfile::default());
    }
    let content = fs::read_to_string(path).map_err(|err| {
        lock_error(
            "FUSE_LOCK_READ_FAILED",
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    let lock: Lockfile = toml::from_str(&content).map_err(|err| {
        lock_error(
            "FUSE_LOCK_PARSE_FAILED",
            format!("invalid lockfile {}: {err}", path.display()),
        )
    })?;
    if lock.version != 0 && lock.version != 1 {
        return Err(lock_error(
            "FUSE_LOCK_UNSUPPORTED_VERSION",
            format!(
                "unsupported lockfile version {} in {} (supported: 0, 1)",
                lock.version,
                path.display()
            ),
        ));
    }
    Ok(lock)
}

fn write_lockfile(path: &Path, lock: &Lockfile) -> Result<(), String> {
    let content = toml::to_string_pretty(lock).map_err(|err| {
        lock_error(
            "FUSE_LOCK_ENCODE_FAILED",
            format!("lockfile encode failed: {err}"),
        )
    })?;
    fs::write(path, content).map_err(|err| {
        lock_error(
            "FUSE_LOCK_WRITE_FAILED",
            format!("failed to write {}: {err}", path.display()),
        )
    })?;
    Ok(())
}

fn store_path(root_dir: &Path, path: &Path) -> String {
    if let Ok(stripped) = path.strip_prefix(root_dir) {
        stripped.to_string_lossy().to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}

fn load_manifest_from_dir(dir: &Path) -> Result<Option<Manifest>, String> {
    let path = dir.join("fuse.toml");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let manifest: Manifest =
        toml::from_str(&content).map_err(|err| format!("invalid manifest: {err}"))?;
    Ok(Some(manifest))
}
