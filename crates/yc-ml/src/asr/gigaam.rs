use std::path::Path;

use ort::value::Tensor;
use yc_core::{
    Error, Result,
    ports::{ModelInfo, SpeechRecognizer},
};

use crate::{Backend, backend::AdaptiveSession};

use super::{
    AsrModelConfig,
    common::{
        Features, MAX_CHUNK_MS, Preprocessor, TokenTiming, Vocabulary, argmax, backend_summary,
        create_session, ensure_vocab_logits, model_error, recognized_from_tokens, validate_audio,
    },
};

const PREDICTOR_DIMS: usize = 320;
const ENCODER_DIMS: usize = 768;
const FRAME_MS: u64 = 40;
const MAX_SYMBOLS_PER_STEP: usize = 3;

struct PredictorState {
    output: Vec<f32>,
    hidden: Vec<f32>,
    cell: Vec<f32>,
}

/// GigaAM v3 E2E RNNT recognizer.
pub struct GigaAmRnnt {
    preprocessor: Preprocessor,
    encoder: AdaptiveSession,
    decoder: AdaptiveSession,
    joint: AdaptiveSession,
    vocabulary: Vocabulary,
    info: ModelInfo,
}

impl GigaAmRnnt {
    pub fn load_configured(
        model_dir: &Path,
        backend: Backend,
        config: AsrModelConfig,
    ) -> Result<Self> {
        let preprocessor = Preprocessor::gigaam(model_dir, backend)?;
        let encoder = create_session(
            model_dir,
            "gigaam-v3-e2e-rnnt-encoder",
            config
                .files
                .encoder
                .as_deref()
                .unwrap_or("v3_e2e_rnnt_encoder.int8.onnx"),
            backend,
        )?;
        let decoder = create_session(
            model_dir,
            "gigaam-v3-e2e-rnnt-decoder",
            config
                .files
                .decoder
                .as_deref()
                .unwrap_or("v3_e2e_rnnt_decoder.int8.onnx"),
            backend,
        )?;
        let joint = create_session(
            model_dir,
            "gigaam-v3-e2e-rnnt-joint",
            config
                .files
                .joint
                .as_deref()
                .unwrap_or("v3_e2e_rnnt_joint.int8.onnx"),
            backend,
        )?;
        let vocab_path = super::required_file(
            model_dir,
            config
                .files
                .vocabulary
                .as_deref()
                .unwrap_or("v3_e2e_rnnt_vocab.txt"),
        )?;
        let vocabulary = Vocabulary::load(&vocab_path)?;
        let info = ModelInfo {
            id: config.id,
            backend: encoder.backend().to_string(),
        };
        Ok(Self {
            preprocessor,
            encoder,
            decoder,
            joint,
            vocabulary,
            info,
        })
    }

    fn encode(&mut self, features: &Features) -> Result<(Vec<f32>, usize)> {
        if features.channels != 64 {
            return Err(Error::Model {
                model: "gigaam-v3-e2e-rnnt-preprocessor".to_owned(),
                message: format!("expected 64 mel bins, got {}", features.channels),
            });
        }
        let data = features.data.clone();
        let frames = features.frames;
        let encoder = self.encoder.run_mapped(|session| {
            let audio = Tensor::from_array(([1_usize, features.channels, frames], data.clone()))
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-encoder", error))?;
            let lengths = Tensor::from_array(([1_usize], vec![frames as i64]))
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-encoder", error))?;
            let outputs = session
                .run(ort::inputs!["audio_signal" => audio, "length" => lengths])
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-encoder", error))?;
            let (shape, encoded) = outputs["encoded"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-encoder", error))?;
            let (_, encoded_lengths) = outputs["encoded_len"]
                .try_extract_tensor::<i32>()
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-encoder", error))?;
            if shape.len() != 3 || shape[1] as usize != ENCODER_DIMS {
                return Err(Error::Model {
                    model: "gigaam-v3-e2e-rnnt-encoder".to_owned(),
                    message: format!("expected encoded [1, {ENCODER_DIMS}, time], got {shape:?}"),
                });
            }
            let encoded_frames = shape[2] as usize;
            let encoded_length = encoded_lengths
                .first()
                .copied()
                .map(|length| length.max(0) as usize)
                .ok_or_else(|| Error::Model {
                    model: "gigaam-v3-e2e-rnnt-encoder".to_owned(),
                    message: "encoder returned no output length".to_owned(),
                })?
                .min(encoded_frames);
            let mut transposed = vec![0.0; encoded_frames * ENCODER_DIMS];
            for frame in 0..encoded_frames {
                for dimension in 0..ENCODER_DIMS {
                    transposed[frame * ENCODER_DIMS + dimension] =
                        encoded[dimension * encoded_frames + frame];
                }
            }
            Ok((transposed, encoded_length))
        })?;
        self.info.backend = self.encoder.backend().to_string();
        Ok(encoder)
    }

    fn decoder_step(
        &mut self,
        token: usize,
        hidden: &[f32],
        cell: &[f32],
    ) -> Result<PredictorState> {
        let hidden_data = hidden.to_vec();
        let cell_data = cell.to_vec();
        self.decoder.run_mapped(|session| {
            let token_tensor = Tensor::from_array(([1_usize, 1], vec![token as i64]))
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-decoder", error))?;
            let hidden_tensor =
                Tensor::from_array(([1_usize, 1, PREDICTOR_DIMS], hidden_data.clone()))
                    .map_err(|error| model_error("gigaam-v3-e2e-rnnt-decoder", error))?;
            let cell_tensor = Tensor::from_array(([1_usize, 1, PREDICTOR_DIMS], cell_data.clone()))
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-decoder", error))?;
            let outputs = session
                .run(
                    ort::inputs!["x" => token_tensor, "h.1" => hidden_tensor, "c.1" => cell_tensor],
                )
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-decoder", error))?;
            let (_, predictor) = outputs["dec"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-decoder", error))?;
            let (_, hidden) = outputs["h"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-decoder", error))?;
            let (_, cell) = outputs["c"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-decoder", error))?;
            Ok(PredictorState {
                output: predictor.to_vec(),
                hidden: hidden.to_vec(),
                cell: cell.to_vec(),
            })
        })
    }

    fn joint_step(&mut self, encoder: &[f32], predictor: &[f32]) -> Result<Vec<f32>> {
        let encoder_data = encoder.to_vec();
        let predictor_data = predictor.to_vec();
        self.joint.run_mapped(|session| {
            let encoded = Tensor::from_array(([1_usize, ENCODER_DIMS, 1], encoder_data.clone()))
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-joint", error))?;
            let decoded =
                Tensor::from_array(([1_usize, PREDICTOR_DIMS, 1], predictor_data.clone()))
                    .map_err(|error| model_error("gigaam-v3-e2e-rnnt-joint", error))?;
            let outputs = session
                .run(ort::inputs!["enc" => encoded, "dec" => decoded])
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-joint", error))?;
            let (_, joint) = outputs["joint"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("gigaam-v3-e2e-rnnt-joint", error))?;
            Ok(joint.to_vec())
        })
    }
}

impl SpeechRecognizer for GigaAmRnnt {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn max_chunk_ms(&self) -> u64 {
        MAX_CHUNK_MS
    }

    fn recognize(
        &mut self,
        pcm: &[f32],
        _language: &str,
    ) -> Result<Vec<yc_core::transcript::Recognized>> {
        validate_audio(pcm)?;
        let features = self.preprocessor.compute(pcm)?;
        let (encoded, encoded_len) = self.encode(&features)?;
        let blank_id = self.vocabulary.blank_id;
        let mut predictor =
            self.decoder_step(blank_id, &[0.0; PREDICTOR_DIMS], &[0.0; PREDICTOR_DIMS])?;
        let mut tokens = Vec::new();
        for frame in 0..encoded_len {
            let encoder_frame = &encoded[frame * ENCODER_DIMS..(frame + 1) * ENCODER_DIMS];
            let mut symbols = 0;
            while symbols < MAX_SYMBOLS_PER_STEP {
                let logits = self.joint_step(encoder_frame, &predictor.output)?;
                ensure_vocab_logits(&logits, self.vocabulary.tokens.len())?;
                let (token, _) = argmax(&logits[..self.vocabulary.tokens.len()])?;
                if token == blank_id {
                    break;
                }
                tokens.push(TokenTiming {
                    id: token,
                    frame,
                    end_frame: frame + 1,
                });
                symbols += 1;
                predictor = self.decoder_step(token, &predictor.hidden, &predictor.cell)?;
            }
        }
        let recognized = recognized_from_tokens(&tokens, &self.vocabulary, FRAME_MS, pcm.len())?;
        self.info.backend = backend_summary([
            self.preprocessor.backend(),
            self.encoder.backend(),
            self.decoder.backend(),
            self.joint.backend(),
        ]);
        Ok(recognized)
    }
}

/// GigaAM v3 E2E CTC recognizer.
pub struct GigaAmCtc {
    preprocessor: Preprocessor,
    model: AdaptiveSession,
    vocabulary: Vocabulary,
    info: ModelInfo,
}

impl GigaAmCtc {
    pub fn load_configured(
        model_dir: &Path,
        backend: Backend,
        config: AsrModelConfig,
    ) -> Result<Self> {
        let preprocessor = Preprocessor::gigaam(model_dir, backend)?;
        let model = create_session(
            model_dir,
            "gigaam-v3-e2e-ctc",
            config
                .files
                .model
                .as_deref()
                .unwrap_or("v3_e2e_ctc.int8.onnx"),
            backend,
        )?;
        let vocab_path = super::required_file(
            model_dir,
            config
                .files
                .vocabulary
                .as_deref()
                .unwrap_or("v3_e2e_ctc_vocab.txt"),
        )?;
        let vocabulary = Vocabulary::load(&vocab_path)?;
        let info = ModelInfo {
            id: config.id,
            backend: model.backend().to_string(),
        };
        Ok(Self {
            preprocessor,
            model,
            vocabulary,
            info,
        })
    }
}

impl SpeechRecognizer for GigaAmCtc {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn max_chunk_ms(&self) -> u64 {
        MAX_CHUNK_MS
    }

    fn recognize(
        &mut self,
        pcm: &[f32],
        _language: &str,
    ) -> Result<Vec<yc_core::transcript::Recognized>> {
        validate_audio(pcm)?;
        let features = self.preprocessor.compute(pcm)?;
        if features.channels != 64 {
            return Err(Error::Model {
                model: "gigaam-v3-e2e-ctc-preprocessor".to_owned(),
                message: format!("expected 64 mel bins, got {}", features.channels),
            });
        }
        let feature_data = features.data.clone();
        let frames = features.frames;
        let vocab_size = self.vocabulary.tokens.len();
        let outputs = self.model.run_mapped(|session| {
            let audio =
                Tensor::from_array(([1_usize, features.channels, frames], feature_data.clone()))
                    .map_err(|error| model_error("gigaam-v3-e2e-ctc", error))?;
            let lengths = Tensor::from_array(([1_usize], vec![frames as i64]))
                .map_err(|error| model_error("gigaam-v3-e2e-ctc", error))?;
            let outputs = session
                .run(ort::inputs!["features" => audio, "feature_lengths" => lengths])
                .map_err(|error| model_error("gigaam-v3-e2e-ctc", error))?;
            let (shape, logits) = outputs["log_probs"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("gigaam-v3-e2e-ctc", error))?;
            Ok((
                shape
                    .iter()
                    .map(|dimension| *dimension as usize)
                    .collect::<Vec<_>>(),
                logits.to_vec(),
            ))
        })?;
        self.info.backend = self.model.backend().to_string();
        if outputs.0.len() != 3 || outputs.0[0] != 1 || outputs.0[2] < vocab_size {
            return Err(Error::Model {
                model: "gigaam-v3-e2e-ctc".to_owned(),
                message: format!(
                    "expected log_probs [1,time,{vocab_size}], got {:?}",
                    outputs.0
                ),
            });
        }
        let time = outputs.0[1].min((features.frames.saturating_sub(1)) / 4 + 1);
        let vocabulary_size = outputs.0[2];
        let mut tokens: Vec<TokenTiming> = Vec::new();
        let mut previous = self.vocabulary.blank_id;
        for frame in 0..time {
            let start = frame * vocabulary_size;
            let (token, _) = argmax(&outputs.1[start..start + vocab_size])?;
            if token != self.vocabulary.blank_id && token != previous {
                tokens.push(TokenTiming {
                    id: token,
                    frame,
                    end_frame: frame + 1,
                });
            } else if token == previous
                && token != self.vocabulary.blank_id
                && let Some(last) = tokens.last_mut()
            {
                last.end_frame = frame + 1;
            }
            previous = token;
        }
        let recognized = recognized_from_tokens(&tokens, &self.vocabulary, FRAME_MS, pcm.len())?;
        self.info.backend = backend_summary([self.preprocessor.backend(), self.model.backend()]);
        Ok(recognized)
    }
}
