//! Typed, versioned model and runtime catalog embedded in the application.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// Schema version for `catalog/models.json`.
pub const CATALOG_SCHEMA_VERSION: u32 = 1;

/// The catalog shipped with this build.
pub const EMBEDDED_CATALOG: &str = include_str!("../../../catalog/models.json");

/// Model and runtime metadata available to the UI and CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    /// Version of the JSON shape, independent of app version.
    pub schema_version: u32,
    /// Date when API-backed sizes and hashes were last refreshed.
    #[serde(default)]
    pub generated_at: String,
    /// Downloadable inference and indexing models.
    pub models: Vec<ModelSpec>,
    /// Prebuilt llama-server sidecar packs.
    #[serde(default)]
    pub runtimes: Vec<RuntimeSpec>,
}

/// A single downloadable model and its resource cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpec {
    /// Stable identifier used in paths and settings.
    pub id: String,
    /// Pipeline role.
    pub kind: ModelKind,
    /// Short display name.
    pub name: String,
    /// Russian description.
    #[serde(default)]
    pub description_ru: String,
    /// English description.
    #[serde(default)]
    pub description_en: String,
    /// BCP-47 language tags, or `multilingual`.
    #[serde(default)]
    pub languages: Vec<String>,
    /// SPDX expression or a short upstream license name.
    pub license: String,
    /// Project or model card URL.
    pub homepage: String,
    /// Product defaults this model satisfies (for example `ru`, `light`, `quality`).
    #[serde(default)]
    pub default_tiers: Vec<String>,
    /// Files required to install the model.
    pub files: Vec<ModelFile>,
    /// Minimum supported host constraints.
    #[serde(default)]
    pub requires: Requirements,
    /// Analytical predictor constants.
    #[serde(default)]
    pub cost: ModelCost,
    /// Upstream or integration work needed before this entry can be used by an adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<String>,
}

/// Broad model category shown in the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    /// Automatic speech recognition.
    Asr,
    /// Voice activity detection.
    Vad,
    /// Speaker embedding and diarisation.
    Speaker,
    /// Text embeddings.
    Embed,
    /// Locally served chat model.
    Llm,
    /// A model runtime bundle.
    Runtime,
}

/// Remote artifact needed by a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFile {
    /// Relative path inside the model installation directory.
    pub path: String,
    /// Expected file length in bytes, from the upstream tree or release API.
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 digest.
    pub sha256: String,
    /// Ordered primary and mirror URLs.
    pub urls: Vec<String>,
    /// Hugging Face repository, present for catalog refreshes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_repo: Option<String>,
    /// GitHub release API, for release asset refreshes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_release_api: Option<String>,
    /// GitHub release asset name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_asset: Option<String>,
    /// Optional files are installed when the downstream adapter needs them.
    #[serde(default)]
    pub optional: bool,
}

/// Platform constraints for a model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirements {
    /// Minimum memory needed to load the model, including working buffers.
    #[serde(default)]
    pub min_ram_bytes: u64,
    /// Supported operating systems (`macos`, `windows`, `linux`). Empty means all.
    #[serde(default)]
    pub os: Vec<String>,
    /// Supported target architectures (`aarch64`, `x86_64`). Empty means all.
    #[serde(default)]
    pub arch: Vec<String>,
}

/// Cost constants shared by the analytical estimates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    /// Approximate parameter count for ASR or embedding models.
    #[serde(default)]
    pub params: f64,
    /// Total downloaded weight and tokenizer bytes used by the model.
    #[serde(default)]
    pub bytes: u64,
    /// ASR forward-pass estimate in GFLOP per audio second.
    #[serde(default)]
    pub gflop_per_audio_sec: f64,
    /// Embedding workload estimate in GFLOP per 512-token passage.
    #[serde(default)]
    pub gflop_per_passage: f64,
    /// Total LLM parameters.
    #[serde(default)]
    pub total_params: f64,
    /// Active parameters per generated token (MoE models can be smaller here).
    #[serde(default)]
    pub active_params: f64,
    /// Weight bytes read for one generated token, including quantization overhead.
    #[serde(default)]
    pub bytes_per_token: u64,
    /// Transformer layer count.
    #[serde(default)]
    pub n_layers: u32,
    /// Number of key/value heads per attention layer.
    #[serde(default)]
    pub kv_heads: u32,
    /// Key/value head width.
    #[serde(default)]
    pub head_dim: u32,
    /// Head width used by sliding-window attention layers.
    #[serde(default)]
    pub local_head_dim: u32,
    /// Sliding attention window for local-attention layers, if present.
    #[serde(default)]
    pub sliding_window: u32,
    /// Effective context length used by the predictor.
    #[serde(default = "default_context")]
    pub context: u32,
    /// Fraction of RAM reserved for KV cache and execution buffers.
    #[serde(default = "default_buffer_fraction")]
    pub buffer_fraction: f64,
    /// Fraction of measured/estimated hardware throughput available to this workload.
    #[serde(default = "default_efficiency")]
    pub efficiency: f64,
    /// Fraction of LLM layers using sliding-window attention (0.0..=1.0).
    #[serde(default = "default_local_layer_fraction")]
    pub local_layer_fraction: f64,
    /// Fraction of layers using full-context attention.
    #[serde(default)]
    pub global_layer_fraction: f64,
}

/// Platform-specific llama-server archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSpec {
    /// Stable pack id.
    pub id: String,
    /// Platform triple family.
    pub platform: String,
    /// Runtime backend (`metal`, `vulkan`, `cuda`, `cpu`).
    pub backend: String,
    /// GitHub release tag pinned by the catalog.
    pub release_tag: String,
    /// API asset name, used to refresh URL/size/hash.
    pub asset_name: String,
    /// Artifact URL.
    pub url: String,
    /// Expected byte length.
    pub size: u64,
    /// Expected SHA-256 from GitHub's asset digest.
    pub sha256: String,
    /// GitHub release API for this pin.
    pub release_api: String,
    /// Additional archives required by this backend, such as CUDA's runtime DLL bundle.
    #[serde(default)]
    pub extra_assets: Vec<RuntimeSupportAsset>,
}

/// Auxiliary artifact that is installed alongside a runtime archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSupportAsset {
    /// API asset name used to refresh the pin.
    pub asset_name: String,
    /// Download URL.
    pub url: String,
    /// Expected byte length.
    pub size: u64,
    /// SHA-256 digest from GitHub.
    pub sha256: String,
}

/// Parse and validate the embedded catalog.
pub fn embedded() -> Result<Catalog> {
    parse(EMBEDDED_CATALOG)
}

/// Parse catalog JSON and verify its schema and artifact metadata.
pub fn parse(json: &str) -> Result<Catalog> {
    let catalog: Catalog = serde_json::from_str(json)
        .map_err(|error| Error::Invalid(format!("invalid model catalog: {error}")))?;
    if catalog.schema_version != CATALOG_SCHEMA_VERSION {
        return Err(Error::Invalid(format!(
            "catalog schema {} is not supported (expected {})",
            catalog.schema_version, CATALOG_SCHEMA_VERSION
        )));
    }
    for model in &catalog.models {
        if model.id.is_empty() || model.files.is_empty() {
            return Err(Error::Invalid(format!(
                "catalog model `{}` must have an id and at least one file",
                model.id
            )));
        }
        for file in &model.files {
            if file.path.is_empty() || file.urls.is_empty() {
                return Err(Error::Invalid(format!(
                    "catalog model `{}` has a file with no path or URL",
                    model.id
                )));
            }
            if file.sha256.len() != 64 || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(Error::Invalid(format!(
                    "catalog file `{}:{}` has no valid SHA-256",
                    model.id, file.path
                )));
            }
        }
    }
    Ok(catalog)
}

/// Find a model by its stable catalog id.
pub fn model<'a>(catalog: &'a Catalog, id: &str) -> Result<&'a ModelSpec> {
    catalog
        .models
        .iter()
        .find(|model| model.id == id)
        .ok_or_else(|| Error::NotFound(format!("model `{id}`")))
}

/// Refresh all Hugging Face file sizes and SHA-256 digests in a catalog.
///
/// Large LFS files use the immutable `lfs.oid` reported by the tree API. Small files are fetched
/// once and hashed directly because Hugging Face does not expose LFS metadata for them.
pub async fn refresh_huggingface(catalog: &mut Catalog, client: &reqwest::Client) -> Result<()> {
    for model in &mut catalog.models {
        for file in &mut model.files {
            let Some(repo) = file.hf_repo.as_deref() else {
                if let (Some(api), Some(asset_name)) = (
                    file.github_release_api.as_deref(),
                    file.github_asset.as_deref(),
                ) {
                    let release: GithubRelease = client
                        .get(api)
                        .send()
                        .await
                        .map_err(network)?
                        .error_for_status()
                        .map_err(network)?
                        .json()
                        .await
                        .map_err(network)?;
                    let asset = release
                        .assets
                        .iter()
                        .find(|asset| asset.name == asset_name)
                        .ok_or_else(|| Error::NotFound(format!("GitHub asset `{asset_name}`")))?;
                    let digest = asset.digest.as_deref().unwrap_or_default();
                    let Some(digest) = digest.strip_prefix("sha256:") else {
                        return Err(Error::Invalid(format!(
                            "GitHub asset `{asset_name}` has no SHA-256 digest"
                        )));
                    };
                    file.size = asset.size;
                    file.sha256 = digest.to_ascii_lowercase();
                    file.urls = vec![asset.browser_download_url.clone()];
                } else if let Some(url) = file.urls.first() {
                    let bytes = client
                        .get(url)
                        .send()
                        .await
                        .map_err(network)?
                        .error_for_status()
                        .map_err(network)?
                        .bytes()
                        .await
                        .map_err(network)?;
                    file.size = bytes.len() as u64;
                    file.sha256 = hex::encode(Sha256::digest(&bytes));
                }
                continue;
            };
            let api = format!(
                "https://huggingface.co/api/models/{repo}/tree/main?recursive=true&expand=true"
            );
            let entries: Vec<HfTreeEntry> = client
                .get(api)
                .send()
                .await
                .map_err(network)?
                .error_for_status()
                .map_err(network)?
                .json()
                .await
                .map_err(network)?;
            let entry = entries
                .iter()
                .find(|entry| entry.path == file.path && entry.kind == "file")
                .ok_or_else(|| Error::NotFound(format!("{repo}/{}", file.path)))?;
            file.size = entry.size.unwrap_or(0);
            if let Some(oid) = entry.lfs.as_ref().and_then(|lfs| lfs.oid.as_deref()) {
                file.sha256 = oid.to_ascii_lowercase();
            } else {
                let url = hf_resolve_url(repo, &file.path);
                let bytes = client
                    .get(url)
                    .send()
                    .await
                    .map_err(network)?
                    .error_for_status()
                    .map_err(network)?
                    .bytes()
                    .await
                    .map_err(network)?;
                file.size = bytes.len() as u64;
                file.sha256 = hex::encode(Sha256::digest(&bytes));
            }
            file.urls = hf_urls(repo, &file.path);
        }
    }
    Ok(())
}

/// Refresh pinned runtime packs from the official llama.cpp GitHub release API.
pub async fn refresh_runtimes(catalog: &mut Catalog, client: &reqwest::Client) -> Result<()> {
    for runtime in &mut catalog.runtimes {
        let release: GithubRelease = client
            .get(&runtime.release_api)
            .send()
            .await
            .map_err(network)?
            .error_for_status()
            .map_err(network)?
            .json()
            .await
            .map_err(network)?;
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == runtime.asset_name)
            .ok_or_else(|| Error::NotFound(format!("release asset `{}`", runtime.asset_name)))?;
        let digest = asset.digest.as_deref().unwrap_or_default();
        let Some(digest) = digest.strip_prefix("sha256:") else {
            return Err(Error::Invalid(format!(
                "GitHub release asset `{}` has no SHA-256 digest",
                runtime.asset_name
            )));
        };
        runtime.release_tag = release.tag_name;
        runtime.url.clone_from(&asset.browser_download_url);
        runtime.size = asset.size;
        runtime.sha256 = digest.to_ascii_lowercase();
        for extra in &mut runtime.extra_assets {
            let asset = release
                .assets
                .iter()
                .find(|asset| asset.name == extra.asset_name)
                .ok_or_else(|| Error::NotFound(format!("release asset `{}`", extra.asset_name)))?;
            let digest = asset.digest.as_deref().unwrap_or_default();
            let Some(digest) = digest.strip_prefix("sha256:") else {
                return Err(Error::Invalid(format!(
                    "GitHub release asset `{}` has no SHA-256 digest",
                    extra.asset_name
                )));
            };
            extra.url.clone_from(&asset.browser_download_url);
            extra.size = asset.size;
            extra.sha256 = digest.to_ascii_lowercase();
        }
    }
    Ok(())
}

/// Refresh the embedded catalog's API-backed metadata and atomically write it to disk.
pub async fn refresh_file(path: &Path) -> Result<Catalog> {
    let mut catalog = embedded()?;
    let client = reqwest::Client::builder()
        .user_agent(concat!("YadaCall/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(network)?;
    refresh_huggingface(&mut catalog, &client).await?;
    refresh_runtimes(&mut catalog, &client).await?;
    catalog.generated_at = chrono::Utc::now().to_rfc3339();
    write_atomic(path, &catalog)?;
    Ok(catalog)
}

/// Save a refreshed catalog atomically at the requested path.
pub fn write_atomic(path: &Path, catalog: &Catalog) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("catalog output path needs a parent directory".into()))?;
    std::fs::create_dir_all(parent)?;
    let temp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(catalog)
        .map_err(|error| Error::Storage(format!("serialize catalog: {error}")))?;
    std::fs::write(&temp, data)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

/// Construct the primary and mirror URLs for one Hugging Face file.
pub fn hf_urls(repo: &str, path: &str) -> Vec<String> {
    let encoded_path = path
        .split('/')
        .map(|part| urlencoding::encode(part).into_owned())
        .collect::<Vec<_>>()
        .join("/");
    ["huggingface.co", "hf-mirror.com"]
        .into_iter()
        .map(|host| format!("https://{host}/{repo}/resolve/main/{encoded_path}?download=true"))
        .collect()
}

fn hf_resolve_url(repo: &str, path: &str) -> String {
    hf_urls(repo, path).remove(0)
}

fn default_context() -> u32 {
    32_768
}

fn default_buffer_fraction() -> f64 {
    0.12
}

fn default_efficiency() -> f64 {
    0.55
}

fn default_local_layer_fraction() -> f64 {
    0.0
}

fn network(error: reqwest::Error) -> Error {
    Error::Network(error.to_string())
}

#[derive(Deserialize)]
struct HfTreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    size: Option<u64>,
    lfs: Option<HfLfs>,
}

#[derive(Deserialize)]
struct HfLfs {
    oid: Option<String>,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}
