//! `yc-ml` behind the engine's model-factory ports. Models are looked up in the `yc-hub` install
//! location; the execution provider is [`Backend::Auto`] (CPU on macOS, which measured fastest).

use std::sync::Arc;

use yc_core::ports::{SpeakerEmbedder, SpeechRecognizer, TextEmbedder, VoiceActivityDetector};
use yc_core::{Error, Result};
use yc_engine::ports::{AsrFactory, SpeakerEmbedderFactory, TextEmbedderFactory, VadFactory};
use yc_hub::catalog::{ModelEngine, ModelSpec};
use yc_ml::{
    AsrCatalogEngine, AsrEngine, AsrFileNames, Backend, CampPlusEmbedder, OnnxTextEmbedder,
    Pooling, SileroVad, TextEmbeddingConfig, asr_config as shared_asr_config,
};

use super::SharedPrefs;
use super::hub::{EMBED_E5_SMALL, ModelHub, SPEAKER_CAMPPLUS};
use super::read_prefs;

fn asr_catalog_engine(engine: ModelEngine) -> AsrCatalogEngine {
    match engine {
        ModelEngine::GigaAmRnnt => AsrCatalogEngine::GigaAmRnnt,
        ModelEngine::GigaAmCtc => AsrCatalogEngine::GigaAmCtc,
        ModelEngine::ParakeetTdt => AsrCatalogEngine::ParakeetTdt,
    }
}

/// Converts catalog metadata to the shared `yc-ml` ASR loader configuration.
pub fn asr_config(spec: &ModelSpec) -> Result<(yc_ml::AsrKind, yc_ml::AsrModelConfig)> {
    let engine = spec
        .engine
        .ok_or_else(|| Error::Invalid(format!("no engine configured for `{}`", spec.id)))?;
    let catalog_files = spec.engine_files.as_ref();
    Ok(shared_asr_config(
        asr_catalog_engine(engine),
        spec.id.clone(),
        AsrFileNames {
            preprocessor: catalog_files.and_then(|files| files.preprocessor.clone()),
            encoder: catalog_files.and_then(|files| files.encoder.clone()),
            decoder: catalog_files.and_then(|files| files.decoder.clone()),
            joint: catalog_files.and_then(|files| files.joint.clone()),
            decoder_joint: catalog_files.and_then(|files| files.decoder_joint.clone()),
            model: catalog_files.and_then(|files| files.model.clone()),
            vocabulary: catalog_files.and_then(|files| files.vocabulary.clone()),
        },
    ))
}

/// Loads GigaAM v3 for `ru` and Parakeet v3 for everything else, unless the user chose a model.
pub struct Asr {
    hub: Arc<ModelHub>,
    prefs: SharedPrefs,
}

impl Asr {
    pub fn new(hub: Arc<ModelHub>, prefs: SharedPrefs) -> Self {
        Self { hub, prefs }
    }
}

impl AsrFactory for Asr {
    fn load(&self, language: &str) -> Result<Box<dyn SpeechRecognizer>> {
        let chosen = read_prefs(&self.prefs).asr;
        let spec = self.hub.pick_asr(language, chosen.as_deref())?;
        let (kind, config) = asr_config(spec)?;
        AsrEngine::load_configured(kind, &self.hub.dir(spec)?, Backend::Auto, config)
    }
}

/// Silero VAD (bundled in `yc-ml`, nothing to install).
pub struct Vad {
    cache_dir: std::path::PathBuf,
}

impl Vad {
    pub fn new(hub: &ModelHub) -> Self {
        Self {
            cache_dir: hub.root().join("models"),
        }
    }
}

impl VadFactory for Vad {
    fn create(&self) -> Result<Box<dyn VoiceActivityDetector>> {
        Ok(Box::new(SileroVad::load(Backend::Auto, &self.cache_dir)?))
    }
}

/// CAM++ speaker embeddings for diarisation; absent until the model is installed.
pub struct Speaker {
    hub: Arc<ModelHub>,
}

impl Speaker {
    pub fn new(hub: Arc<ModelHub>) -> Self {
        Self { hub }
    }
}

impl SpeakerEmbedderFactory for Speaker {
    fn is_available(&self) -> bool {
        self.hub
            .spec(SPEAKER_CAMPPLUS)
            .is_ok_and(|s| self.hub.is_installed(s))
    }

    fn create(&self) -> Result<Option<Box<dyn SpeakerEmbedder>>> {
        let spec = self.hub.spec(SPEAKER_CAMPPLUS)?;
        if !self.hub.is_installed(spec) {
            return Ok(None);
        }
        let embedder = CampPlusEmbedder::load(&self.hub.dir(spec)?, Backend::Auto)?;
        Ok(Some(Box::new(embedder)))
    }
}

/// The text-embedding model for semantic search; absent until installed.
pub struct TextEmbed {
    hub: Arc<ModelHub>,
    prefs: SharedPrefs,
}

impl TextEmbed {
    pub fn new(hub: Arc<ModelHub>, prefs: SharedPrefs) -> Self {
        Self { hub, prefs }
    }

    fn active_id(&self) -> String {
        read_prefs(&self.prefs)
            .text_embedding
            .unwrap_or_else(|| EMBED_E5_SMALL.to_owned())
    }
}

/// Loads an embedding model of the catalog (multilingual E5 small is the one supported today).
pub fn load_text_embedder(hub: &ModelHub, id: &str) -> Result<Box<dyn TextEmbedder>> {
    let spec = hub.spec(id)?;
    let dir = hub.dir(spec)?;
    let config = match id {
        EMBED_E5_SMALL => TextEmbeddingConfig {
            onnx_path: dir.join("onnx/model_int8.onnx"),
            tokenizer_path: dir.join("tokenizer.json"),
            query_prefix: "query: ".to_owned(),
            passage_prefix: "passage: ".to_owned(),
            pooling: Pooling::Mean,
            max_tokens: 512,
            dims: 384,
        },
        other => return Err(Error::Invalid(format!("no embedder for `{other}`"))),
    };
    let embedder: OnnxTextEmbedder = OnnxTextEmbedder::load(config, Backend::Auto, &dir)?;
    Ok(Box::new(embedder))
}

impl TextEmbedderFactory for TextEmbed {
    fn is_available(&self) -> bool {
        self.hub
            .spec(&self.active_id())
            .is_ok_and(|s| self.hub.is_installed(s))
    }

    fn create(&self) -> Result<Option<Box<dyn TextEmbedder>>> {
        let id = self.active_id();
        let installed = self.hub.spec(&id).is_ok_and(|s| self.hub.is_installed(s));
        if !installed {
            return Ok(None);
        }
        load_text_embedder(&self.hub, &id).map(Some)
    }
}
