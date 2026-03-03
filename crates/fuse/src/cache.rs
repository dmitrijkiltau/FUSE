use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub(crate) fn sha1_digest(input: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xEFCD_AB89;
    let mut h2: u32 = 0x98BA_DCFE;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xC3D2_E1F0;

    let mut data = input.to_vec();
    data.push(0x80);
    while (data.len() % 64) != 56 {
        data.push(0);
    }
    let bit_len = (input.len() as u64) * 8;
    data.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in data.chunks(64) {
        let mut words = [0u32; 80];
        for (i, word) in words.iter_mut().enumerate().take(16) {
            let base = i * 4;
            *word = u32::from_be_bytes([
                chunk[base],
                chunk[base + 1],
                chunk[base + 2],
                chunk[base + 3],
            ]);
        }
        for i in 16..80 {
            words[i] = (words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for (i, word) in words.iter().enumerate() {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A82_7999)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9_EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC)
            } else {
                (b ^ c ^ d, 0xCA62_C1D6)
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

pub(crate) fn build_dir(manifest_dir: Option<&Path>) -> Result<PathBuf, String> {
    let base = match manifest_dir {
        Some(dir) => dir.to_path_buf(),
        None => env::current_dir().map_err(|err| format!("cwd error: {err}"))?,
    };
    Ok(base.join(".fuse").join("build"))
}

pub(crate) fn clean_build_dir(manifest_dir: Option<&Path>) -> Result<(), String> {
    let dir = build_dir(manifest_dir)?;
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .map_err(|err| format!("failed to remove {}: {err}", dir.display()))?;
    }
    Ok(())
}

pub(crate) fn build_ir_meta(
    registry: &fusec::ModuleRegistry,
    manifest_dir: Option<&Path>,
) -> Result<super::IrMeta, String> {
    let mut files = Vec::new();
    for unit in registry.modules.values() {
        if is_virtual_module_path(&unit.path) {
            continue;
        }
        files.push(super::IrFileMeta {
            path: unit.path.to_string_lossy().to_string(),
            hash: file_hash_hex(&unit.path)?,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest_hash = manifest_dir
        .map(|dir| dir.join("fuse.toml"))
        .and_then(|path| optional_file_hash_hex(&path).transpose())
        .transpose()?;
    let lock_hash = manifest_dir
        .map(|dir| dir.join("fuse.lock"))
        .and_then(|path| optional_file_hash_hex(&path).transpose())
        .transpose()?;
    Ok(super::IrMeta {
        version: 3,
        native_cache_version: fusec::native::CACHE_VERSION,
        files,
        manifest_hash,
        lock_hash,
        build_target: super::BUILD_TARGET_FINGERPRINT.to_string(),
        rustc_version: super::BUILD_RUSTC_FINGERPRINT.to_string(),
        cli_version: super::BUILD_CLI_VERSION_FINGERPRINT.to_string(),
    })
}

pub(crate) fn load_ir_meta(path: &Path) -> Option<super::IrMeta> {
    let bytes = fs::read(path).ok()?;
    bincode::deserialize(&bytes).ok()
}

pub(crate) fn write_ir_meta(path: &Path, meta: &super::IrMeta) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
    }
    let bytes = bincode::serialize(meta).map_err(|err| format!("ir meta encode failed: {err}"))?;
    fs::write(path, bytes).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(())
}

pub(crate) fn ir_meta_base_is_valid(meta: &super::IrMeta, manifest_dir: Option<&Path>) -> bool {
    if meta.version != 3 || meta.files.is_empty() {
        return false;
    }
    if meta.native_cache_version != fusec::native::CACHE_VERSION {
        return false;
    }
    let current_manifest_hash = manifest_dir
        .map(|dir| dir.join("fuse.toml"))
        .and_then(|path| optional_file_hash_hex(&path).transpose())
        .transpose()
        .ok()
        .flatten();
    if meta.manifest_hash != current_manifest_hash {
        return false;
    }
    let current_lock_hash = manifest_dir
        .map(|dir| dir.join("fuse.lock"))
        .and_then(|path| optional_file_hash_hex(&path).transpose())
        .transpose()
        .ok()
        .flatten();
    if meta.lock_hash != current_lock_hash {
        return false;
    }
    if meta.build_target != super::BUILD_TARGET_FINGERPRINT {
        return false;
    }
    if meta.rustc_version != super::BUILD_RUSTC_FINGERPRINT {
        return false;
    }
    if meta.cli_version != super::BUILD_CLI_VERSION_FINGERPRINT {
        return false;
    }
    true
}

pub(crate) fn ir_meta_is_valid(meta: &super::IrMeta, manifest_dir: Option<&Path>) -> bool {
    if !ir_meta_base_is_valid(meta, manifest_dir) {
        return false;
    }
    for file in &meta.files {
        let path = Path::new(&file.path);
        if is_virtual_module_path(path) {
            continue;
        }
        let hash = match file_hash_hex(path) {
            Ok(hash) => hash,
            Err(_) => return false,
        };
        if hash != file.hash {
            return false;
        }
    }
    true
}

fn check_meta_path(
    manifest_dir: Option<&Path>,
    strict_architecture: bool,
) -> Result<PathBuf, String> {
    let build = build_dir(manifest_dir)?;
    let name = if strict_architecture {
        "check.strict.meta"
    } else {
        "check.meta"
    };
    Ok(build.join(name))
}

pub(crate) fn load_check_ir_meta(
    manifest_dir: Option<&Path>,
    strict_architecture: bool,
) -> Option<super::IrMeta> {
    let path = check_meta_path(manifest_dir, strict_architecture).ok()?;
    load_ir_meta(&path)
}

pub(crate) fn write_check_ir_meta(
    manifest_dir: Option<&Path>,
    strict_architecture: bool,
    meta: &super::IrMeta,
) -> Result<(), String> {
    let path = check_meta_path(manifest_dir, strict_architecture)?;
    write_ir_meta(&path, meta)
}

pub(crate) fn check_meta_files_unchanged(meta: &super::IrMeta) -> bool {
    for file in &meta.files {
        let path = Path::new(&file.path);
        if is_virtual_module_path(path) {
            continue;
        }
        let hash = match file_hash_hex(path) {
            Ok(hash) => hash,
            Err(_) => return false,
        };
        if hash != file.hash {
            return false;
        }
    }
    true
}

pub(crate) fn changed_modules_since_meta(
    registry: &fusec::ModuleRegistry,
    current_meta: &super::IrMeta,
    cached_meta: Option<&super::IrMeta>,
    manifest_dir: Option<&Path>,
) -> HashSet<usize> {
    let Some(cached_meta) = cached_meta else {
        return registry
            .modules
            .iter()
            .filter_map(|(id, unit)| (!is_virtual_module_path(&unit.path)).then_some(*id))
            .collect();
    };
    if !ir_meta_base_is_valid(cached_meta, manifest_dir) {
        return registry
            .modules
            .iter()
            .filter_map(|(id, unit)| (!is_virtual_module_path(&unit.path)).then_some(*id))
            .collect();
    }

    let mut changed = HashSet::new();
    let current_hashes: HashMap<&str, &str> = current_meta
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.hash.as_str()))
        .collect();
    let cached_hashes: HashMap<&str, &str> = cached_meta
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.hash.as_str()))
        .collect();

    for (id, unit) in &registry.modules {
        if is_virtual_module_path(&unit.path) {
            continue;
        }
        let path = unit.path.to_string_lossy();
        let current = current_hashes.get(path.as_ref()).copied();
        let cached = cached_hashes.get(path.as_ref()).copied();
        if current.is_none() || cached.is_none() || current != cached {
            changed.insert(*id);
        }
    }

    if changed.is_empty() && current_hashes.len() != cached_hashes.len() {
        return registry
            .modules
            .iter()
            .filter_map(|(id, unit)| (!is_virtual_module_path(&unit.path)).then_some(*id))
            .collect();
    }

    changed
}

pub(crate) fn is_virtual_module_path(path: &Path) -> bool {
    path.to_string_lossy().starts_with('<')
}

pub(crate) fn affected_modules_for_incremental_check(
    registry: &fusec::ModuleRegistry,
    changed: &HashSet<usize>,
) -> HashSet<usize> {
    if changed.is_empty() {
        return registry.modules.keys().copied().collect();
    }

    let mut reverse: HashMap<usize, HashSet<usize>> = HashMap::new();
    for (id, unit) in &registry.modules {
        for link in unit.modules.modules.values() {
            reverse.entry(link.id).or_default().insert(*id);
        }
    }

    let mut affected = changed.clone();
    let mut stack: Vec<usize> = changed.iter().copied().collect();
    while let Some(module_id) = stack.pop() {
        let Some(importers) = reverse.get(&module_id) else {
            continue;
        };
        for importer in importers {
            if affected.insert(*importer) {
                stack.push(*importer);
            }
        }
    }
    affected
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct FileStamp {
    pub(crate) modified_secs: u64,
    pub(crate) modified_nanos: u32,
    pub(crate) size: u64,
}

pub(crate) fn file_stamp(path: &Path) -> Result<FileStamp, String> {
    let metadata =
        fs::metadata(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let modified = metadata
        .modified()
        .map_err(|err| format!("failed to read mtime for {}: {err}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("mtime before epoch for {}: {err}", path.display()))?;
    Ok(FileStamp {
        modified_secs: duration.as_secs(),
        modified_nanos: duration.subsec_nanos(),
        size: metadata.len(),
    })
}

fn file_hash_hex(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    Ok(hash_hex(&sha1_digest(&bytes)))
}

fn optional_file_hash_hex(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(file_hash_hex(path)?))
}

fn hash_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
