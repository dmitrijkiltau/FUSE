use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) package: PackageConfig,
    #[serde(default)]
    pub(crate) build: Option<BuildConfig>,
    #[serde(default)]
    pub(crate) serve: Option<ServeConfig>,
    #[serde(default)]
    pub(crate) assets: Option<AssetsConfig>,
    #[serde(default)]
    pub(crate) vite: Option<ViteConfig>,
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PackageConfig {
    #[serde(alias = "main")]
    pub(crate) entry: Option<String>,
    pub(crate) app: Option<String>,
    pub(crate) backend: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BuildConfig {
    pub(crate) openapi: Option<String>,
    pub(crate) native_bin: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ServeConfig {
    pub(crate) static_dir: Option<String>,
    pub(crate) static_index: Option<String>,
    pub(crate) openapi_ui: Option<bool>,
    pub(crate) openapi_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AssetsConfig {
    pub(crate) css: Option<String>,
    pub(crate) watch: Option<bool>,
    pub(crate) hash: Option<bool>,
    #[serde(default)]
    pub(crate) hooks: Option<AssetHooksConfig>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AssetHooksConfig {
    pub(crate) before_build: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ViteConfig {
    pub(crate) dev_url: Option<String>,
    pub(crate) dist_dir: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum DependencySpec {
    Simple(String),
    Detailed(DependencyDetail),
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(crate) struct DependencyDetail {
    pub(crate) version: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) git: Option<String>,
    pub(crate) rev: Option<String>,
    pub(crate) tag: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) subdir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct IrMeta {
    #[serde(default)]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) native_cache_version: u32,
    #[serde(default)]
    pub(crate) files: Vec<IrFileMeta>,
    #[serde(default)]
    pub(crate) manifest_hash: Option<String>,
    #[serde(default)]
    pub(crate) lock_hash: Option<String>,
    #[serde(default)]
    pub(crate) build_target: String,
    #[serde(default)]
    pub(crate) rustc_version: String,
    #[serde(default)]
    pub(crate) cli_version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct IrFileMeta {
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) hash: String,
}
