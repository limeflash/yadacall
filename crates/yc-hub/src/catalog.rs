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
    /// Normalized model languages (`ru`, `en`, or `multilingual`).
    #[serde(default)]
    pub languages: Vec<String>,
    /// Short, human-readable model vendor.
    #[serde(default)]
    pub vendor: String,
    /// Model family used for grouping and search.
    #[serde(default)]
    pub family: String,
    /// Machine-readable traits from the model-entry contract vocabulary.
    #[serde(default)]
    pub tags: Vec<String>,
    /// SPDX expression or a short upstream license name.
    pub license: String,
    /// Link to the applicable license text or terms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_url: Option<String>,
    /// Whether the license has custom or attribution terms that merit a UI notice.
    #[serde(default)]
    pub license_custom: bool,
    /// Project or model card URL.
    pub homepage: String,
    /// Relative quality within this model kind; higher is better.
    #[serde(default)]
    pub quality_rank: i32,
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
    /// Adapter family used to instantiate ASR catalog entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<ModelEngine>,
    /// Optional local filenames for an ASR engine's model artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_files: Option<EngineFileNames>,
    /// Per-model options for the local LLM runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmOptions>,
    /// Upstream or integration work needed before this entry can be used by an adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<String>,
}

/// Adapter family stored in an ASR catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelEngine {
    /// Parakeet TDT encoder and joint decoder.
    #[serde(rename = "parakeet-tdt")]
    ParakeetTdt,
    /// GigaAM v3 transducer.
    #[serde(rename = "gigaam-rnnt")]
    GigaAmRnnt,
    /// GigaAM CTC model, including the multilingual release.
    #[serde(rename = "gigaam-ctc")]
    GigaAmCtc,
}

/// Optional local artifact names consumed by an ASR engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineFileNames {
    /// ONNX preprocessor, when the engine uses a downloaded graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preprocessor: Option<String>,
    /// Encoder graph for transducer engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    /// Predictor/decoder graph for RNNT engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder: Option<String>,
    /// Joint graph for RNNT engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint: Option<String>,
    /// Combined decoder/joint graph for Parakeet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder_joint: Option<String>,
    /// CTC or other single-model graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Token vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<String>,
}

/// Runtime options attached to an LLM catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmOptions {
    /// Whether llama.cpp should disable model reasoning tokens.
    pub reasoning: ReasoningMode,
    /// Model-card sampling values for local summary and RAG requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<LlmSampling>,
}

/// Reasoning behavior supported by the pinned llama.cpp runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningMode {
    /// Disable model reasoning/thinking for concise summaries and answers.
    Off,
    /// Leave the model/template default unchanged.
    Default,
}

/// Optional sampling values supported by llama-server's OpenAI-compatible endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmSampling {
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top-k sampling limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Minimum probability cutoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    /// Presence penalty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
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
    /// Remote Hugging Face path when it differs from the installed local filename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_path: Option<String>,
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
    /// Weight bytes read for one generated token, including quantization overhead. For the
    /// Gemma 4 26B-A4B and GigaChat 3.1 Lightning entries this is downloaded weight bytes ×
    /// active parameters / total parameters; the MoE weights are assumed to be proportionally
    /// distributed across experts.
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
    /// Attention layout used to estimate context-dependent cache memory.
    #[serde(default)]
    pub attention: AttentionCost,
}

/// Attention layout used by the KV-cache memory estimate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AttentionCost {
    /// Conventional K/V cache, using the layer and head fields on [`ModelCost`].
    #[default]
    Standard,
    /// Multi-head latent attention. llama.cpp caches one compressed latent plus its RoPE key
    /// dimensions; its MLA value view reuses the latent, so it is counted once.
    /// Verified in `src/models/deepseek2.cpp`: `Kcur` concatenates `kv_cmpr` and `k_pe`, `Vcur`
    /// views `kv_cmpr`, and the graph uses the MLA K-only cache input.
    Mla {
        /// Compressed key/value latent width per layer.
        #[serde(default)]
        kv_lora_rank: u32,
        /// RoPE key width stored alongside the latent per layer.
        #[serde(default)]
        qk_rope_head_dim: u32,
    },
    /// Hybrid recurrent and full-attention layout. The recurrent state is fixed in size while
    /// full-attention cache grows with context.
    Hybrid {
        /// Number of layers with a context-growing full-attention cache.
        #[serde(default)]
        full_attention_layers: u32,
        /// Total fixed recurrent-state memory for linear-attention layers.
        /// For Qwen3.5 this is `linear_layers × 4 × (value_heads × key_dim × value_dim +
        /// (2 × key_heads × key_dim + value_heads × value_dim) × convolution_kernel)` bytes:
        /// f32 recurrent state plus f32 Q/K/V convolution history, from `config.json` dimensions.
        #[serde(default)]
        linear_state_bytes: u64,
    },
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
            let remote_path = file.hf_path.as_deref().unwrap_or(&file.path);
            let entry = entries
                .iter()
                .find(|entry| entry.path == remote_path && entry.kind == "file")
                .ok_or_else(|| Error::NotFound(format!("{repo}/{remote_path}")))?;
            file.size = entry.size.unwrap_or(0);
            if let Some(oid) = entry.lfs.as_ref().and_then(|lfs| lfs.oid.as_deref()) {
                file.sha256 = oid.to_ascii_lowercase();
            } else {
                let url = hf_resolve_url(repo, remote_path);
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
            file.urls = hf_urls(repo, remote_path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_ids_metadata_files_and_llm_costs_are_complete() {
        let catalog = embedded().expect("embedded catalog");
        let mut ids = HashSet::new();
        let allowed_tags = [
            "ru-tuned",
            "moe",
            "english-only",
            "reasoning-off",
            "streaming",
            "tiny",
        ];
        for model in &catalog.models {
            assert!(ids.insert(&model.id), "duplicate model id {}", model.id);
            assert!(
                !model.vendor.trim().is_empty(),
                "{} has no vendor",
                model.id
            );
            assert!(
                !model.family.trim().is_empty(),
                "{} has no family",
                model.id
            );
            assert!(
                !model.license.trim().is_empty(),
                "{} has no license",
                model.id
            );
            assert!(model.quality_rank > 0, "{} has no qualityRank", model.id);
            assert!(
                model
                    .languages
                    .iter()
                    .all(|language| { matches!(language.as_str(), "ru" | "en" | "multilingual") })
            );
            assert!(
                model
                    .tags
                    .iter()
                    .all(|tag| allowed_tags.contains(&tag.as_str()))
            );
            for file in &model.files {
                assert!(file.size > 0, "{}:{} has no size", model.id, file.path);
                assert!(
                    file.sha256.len() == 64
                        && file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
                    "{}:{} has an invalid SHA-256",
                    model.id,
                    file.path
                );
                assert!(
                    file.urls.iter().any(|url| url.contains("huggingface.co/")),
                    "{}:{} has no Hugging Face URL",
                    model.id,
                    file.path
                );
            }
            if model.kind == ModelKind::Asr {
                assert!(model.engine.is_some(), "{} has no ASR engine", model.id);
            }
            if model.kind == ModelKind::Llm {
                assert!(
                    model.cost.total_params > 0.0,
                    "{} has no totalParams",
                    model.id
                );
                assert!(
                    model.cost.active_params > 0.0,
                    "{} has no activeParams",
                    model.id
                );
                assert!(
                    model.cost.bytes_per_token > 0,
                    "{} has no bytesPerToken",
                    model.id
                );
                assert!(model.cost.n_layers > 0, "{} has no layer count", model.id);
                assert!(model.cost.context > 0, "{} has no context", model.id);
                assert!(model.llm.is_some(), "{} has no llm options", model.id);
                match &model.cost.attention {
                    AttentionCost::Standard => {
                        assert!(model.cost.kv_heads > 0 && model.cost.head_dim > 0);
                    }
                    AttentionCost::Mla {
                        kv_lora_rank,
                        qk_rope_head_dim,
                    } => {
                        assert!(*kv_lora_rank > 0 && *qk_rope_head_dim > 0);
                    }
                    AttentionCost::Hybrid {
                        full_attention_layers,
                        linear_state_bytes,
                    } => {
                        assert!(*full_attention_layers > 0);
                        assert!(*linear_state_bytes > 0);
                        assert!(model.cost.kv_heads > 0 && model.cost.head_dim > 0);
                    }
                }
            }
        }
    }
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
