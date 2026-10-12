//! Generic BERT/XLM-R ONNX text embeddings.

use std::{path::PathBuf, time::Instant};

use ort::value::Tensor;
use tokenizers::{PaddingParams, Tokenizer, TruncationDirection};
use yc_core::{
    Error, Result,
    ports::{EmbedKind, ModelInfo, TextEmbedder},
};

use crate::{
    Backend,
    backend::{AdaptiveSession, model_cache_dir},
};

/// Pooling operation applied to the encoder's token vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pooling {
    /// Average token vectors selected by `attention_mask`.
    Mean,
    /// Take the first token vector, normally `[CLS]` or `<s>`.
    Cls,
}

/// Paths and model-specific behavior for a generic text embedding ONNX model.
#[derive(Debug, Clone)]
pub struct TextEmbeddingConfig {
    /// ONNX graph path.
    pub onnx_path: PathBuf,
    /// HuggingFace `tokenizer.json` path.
    pub tokenizer_path: PathBuf,
    /// Prefix added to queries.
    pub query_prefix: String,
    /// Prefix added to passages.
    pub passage_prefix: String,
    /// Token pooling rule.
    pub pooling: Pooling,
    /// Maximum encoded sequence length, including special tokens.
    pub max_tokens: usize,
    /// Expected output vector size.
    pub dims: usize,
}

/// Configurable multilingual encoder implementing YadaCall's text embedding port.
pub struct OnnxTextEmbedder {
    session: AdaptiveSession,
    tokenizer: Tokenizer,
    config: TextEmbeddingConfig,
    info: ModelInfo,
}

impl OnnxTextEmbedder {
    /// Loads an encoder and tokenizer from local model files.
    pub fn load(
        config: TextEmbeddingConfig,
        backend: Backend,
        model_dir: &std::path::Path,
    ) -> Result<Self> {
        if config.max_tokens == 0 || config.dims == 0 {
            return Err(Error::Invalid(
                "text embedding max_tokens and dims must be positive".to_owned(),
            ));
        }
        let tokenizer =
            Tokenizer::from_file(&config.tokenizer_path).map_err(|error| Error::Model {
                model: "text-embedder".to_owned(),
                message: format!("failed to load tokenizer: {error}"),
            })?;
        let model_id = config
            .onnx_path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("text-embedder")
            .to_owned();
        let session = AdaptiveSession::from_file(
            &config.onnx_path,
            backend,
            &model_cache_dir(model_dir, &model_id),
        )?;
        let info = ModelInfo {
            id: model_id,
            backend: session.backend().to_string(),
        };
        Ok(Self {
            session,
            tokenizer,
            config,
            info,
        })
    }

    fn encode_batch(&mut self, texts: &[&str], kind: EmbedKind) -> Result<Vec<Vec<f32>>> {
        let prefix = match kind {
            EmbedKind::Query => &self.config.query_prefix,
            EmbedKind::Passage => &self.config.passage_prefix,
        };
        let with_prefix = texts
            .iter()
            .map(|text| format!("{prefix}{text}"))
            .collect::<Vec<_>>();
        let mut encodings = self
            .tokenizer
            .encode_batch(with_prefix, true)
            .map_err(|error| Error::Model {
                model: self.info.id.clone(),
                message: format!("tokenization failed: {error}"),
            })?;
        for encoding in &mut encodings {
            encoding.truncate(self.config.max_tokens, 0, TruncationDirection::Right);
        }
        let sequence_len = encodings
            .iter()
            .map(|encoding| encoding.len())
            .max()
            .unwrap_or(0);
        if sequence_len == 0 {
            return Ok(vec![Vec::new(); texts.len()]);
        }
        let (pad_id, pad_type_id) = padding_tokens(&self.tokenizer)?;
        let batch_size = encodings.len();
        let mut input_ids = Vec::with_capacity(batch_size * sequence_len);
        let mut attention_mask = Vec::with_capacity(batch_size * sequence_len);
        let mut token_type_ids = Vec::with_capacity(batch_size * sequence_len);
        for encoding in &encodings {
            let ids = encoding.get_ids();
            let masks = encoding.get_attention_mask();
            let type_ids = encoding.get_type_ids();
            input_ids.extend(ids.iter().map(|id| i64::from(*id)));
            attention_mask.extend(masks.iter().map(|mask| i64::from(*mask)));
            token_type_ids.extend(type_ids.iter().map(|id| i64::from(*id)));
            let padding = sequence_len - ids.len();
            input_ids.extend(std::iter::repeat_n(i64::from(pad_id), padding));
            attention_mask.extend(std::iter::repeat_n(0_i64, padding));
            token_type_ids.extend(std::iter::repeat_n(i64::from(pad_type_id), padding));
        }
        let input_names = self.session_input_names()?;
        let output_names = self.session.output_names();
        let output_name = output_names
            .iter()
            .find(|name| name.as_str() == "last_hidden_state")
            .or_else(|| output_names.first())
            .cloned()
            .ok_or_else(|| Error::Model {
                model: self.info.id.clone(),
                message: "encoder declares no output tensors".to_owned(),
            })?;
        let model_id = self.info.id.clone();
        let ids_for_run = input_ids.clone();
        let mask_for_run = attention_mask.clone();
        let type_ids_for_run = token_type_ids.clone();
        let expected_dims = self.config.dims;
        let pooling = self.config.pooling;
        let result_name = output_name.clone();
        let started = Instant::now();
        let vectors = self.session.run_mapped(|session| {
            let ids = Tensor::from_array(([batch_size, sequence_len], ids_for_run.clone()))
                .map_err(|error| model_error(&model_id, error))?;
            let mask = Tensor::from_array(([batch_size, sequence_len], mask_for_run.clone()))
                .map_err(|error| model_error(&model_id, error))?;
            let mut inputs = vec![
                ("input_ids", ids.into_dyn()),
                ("attention_mask", mask.into_dyn()),
            ];
            if input_names.iter().any(|name| name == "token_type_ids") {
                let types =
                    Tensor::from_array(([batch_size, sequence_len], type_ids_for_run.clone()))
                        .map_err(|error| model_error(&model_id, error))?;
                inputs.push(("token_type_ids", types.into_dyn()));
            }
            let outputs = session
                .run(inputs)
                .map_err(|error| model_error(&model_id, error))?;
            let (shape, hidden) = outputs[result_name.as_str()]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error(&model_id, error))?;
            if shape.len() != 3 || shape[0] != batch_size as i64 || shape[1] != sequence_len as i64
            {
                return Err(Error::Model {
                    model: model_id.clone(),
                    message: format!(
                        "expected hidden states [batch, sequence, dims], got {shape:?}"
                    ),
                });
            }
            let actual_dims = shape[2] as usize;
            if actual_dims != expected_dims {
                return Err(Error::Model {
                    model: model_id.clone(),
                    message: format!(
                        "configured dims {expected_dims}, encoder returned {actual_dims}"
                    ),
                });
            }
            let mut batch_vectors = Vec::with_capacity(batch_size);
            for batch in 0..batch_size {
                let mut vector = vec![0.0_f32; actual_dims];
                let mut token_count = 0.0_f32;
                let start_token = match pooling {
                    Pooling::Cls => 0,
                    Pooling::Mean => 0,
                };
                let end_token = match pooling {
                    Pooling::Cls => 1,
                    Pooling::Mean => sequence_len,
                };
                for token in start_token..end_token {
                    let mask_index = batch * sequence_len + token;
                    let weight = match pooling {
                        Pooling::Cls => 1.0,
                        Pooling::Mean => (mask_for_run[mask_index] != 0) as u8 as f32,
                    };
                    if weight == 0.0 {
                        continue;
                    }
                    token_count += weight;
                    let offset = (batch * sequence_len + token) * actual_dims;
                    for (dimension, value) in vector.iter_mut().enumerate() {
                        *value += hidden[offset + dimension] * weight;
                    }
                }
                if token_count > 0.0 {
                    for value in &mut vector {
                        *value /= token_count;
                    }
                }
                normalize(&mut vector);
                batch_vectors.push(vector);
            }
            Ok(batch_vectors)
        })?;
        self.info.backend = self.session.backend().to_string();
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            count = texts.len(),
            "embedded text batch"
        );
        Ok(vectors)
    }

    fn session_input_names(&self) -> Result<Vec<String>> {
        let names = self.session.input_names();
        if !names.iter().any(|name| name == "input_ids")
            || !names.iter().any(|name| name == "attention_mask")
        {
            return Err(Error::Model {
                model: self.info.id.clone(),
                message: format!(
                    "expected BERT input_ids and attention_mask inputs, found {names:?}"
                ),
            });
        }
        Ok(names)
    }
}

impl TextEmbedder for OnnxTextEmbedder {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn dims(&self) -> usize {
        self.config.dims
    }

    fn embed(&mut self, texts: &[&str], kind: EmbedKind) -> Result<Vec<Vec<f32>>> {
        let mut result = Vec::with_capacity(texts.len());
        for batch in texts.chunks(32) {
            result.extend(self.encode_batch(batch, kind)?);
        }
        Ok(result)
    }
}

fn padding_tokens(tokenizer: &Tokenizer) -> Result<(u32, u32)> {
    if let Some(PaddingParams {
        pad_id,
        pad_type_id,
        ..
    }) = tokenizer.get_padding()
    {
        return Ok((*pad_id, *pad_type_id));
    }
    let pad_id = ["<pad>", "[PAD]"]
        .into_iter()
        .find_map(|token| tokenizer.token_to_id(token))
        .ok_or_else(|| Error::Invalid("tokenizer has no padding token".to_owned()))?;
    Ok((pad_id, 0))
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        vector.iter_mut().for_each(|value| *value /= norm);
    }
}

fn model_error(model: &str, error: impl std::fmt::Display) -> Error {
    Error::Model {
        model: model.to_owned(),
        message: error.to_string(),
    }
}
