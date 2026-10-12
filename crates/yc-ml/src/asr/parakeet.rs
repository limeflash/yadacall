use std::path::Path;

use ort::value::Tensor;
use yc_core::{
    Error, Result,
    ports::{ModelInfo, SpeechRecognizer},
};

use crate::{Backend, backend::AdaptiveSession};

use super::common::{
    MAX_CHUNK_MS, Preprocessor, TokenTiming, Vocabulary, argmax, backend_summary, create_session,
    model_error, recognized_from_tokens, validate_audio,
};

const ENCODER_DIMS: usize = 1024;
const PREDICTOR_DIMS: usize = 640;
const STATE_COUNT: usize = 2;
const FRAME_MS: u64 = 80;
const MAX_SYMBOLS_PER_STEP: usize = 10;

/// Parakeet TDT v3 multilingual ONNX recognizer.
pub struct ParakeetTdt {
    preprocessor: Preprocessor,
    encoder: AdaptiveSession,
    decoder_joint: AdaptiveSession,
    vocabulary: Vocabulary,
    info: ModelInfo,
}

impl ParakeetTdt {
    /// Loads the int8 Parakeet TDT model files from a Hugging Face repository directory.
    pub fn load(model_dir: &Path, backend: Backend) -> Result<Self> {
        let preprocessor = Preprocessor::parakeet(model_dir, backend)?;
        let encoder = create_session(
            model_dir,
            "parakeet-tdt-v3-encoder",
            "encoder-model.int8.onnx",
            backend,
        )?;
        let decoder_joint = create_session(
            model_dir,
            "parakeet-tdt-v3-decoder-joint",
            "decoder_joint-model.int8.onnx",
            backend,
        )?;
        let vocab_path = super::required_file(model_dir, "vocab.txt")?;
        let vocabulary = Vocabulary::load(&vocab_path)?;
        let info = ModelInfo {
            id: "parakeet-tdt-0.6b-v3-int8".to_owned(),
            backend: encoder.backend().to_string(),
        };
        Ok(Self {
            preprocessor,
            encoder,
            decoder_joint,
            vocabulary,
            info,
        })
    }

    fn encode(&mut self, features: &[f32], frames: usize) -> Result<(Vec<f32>, usize)> {
        let data = features.to_vec();
        let (encoded, encoded_len) = self.encoder.run_mapped(|session| {
            let audio = Tensor::from_array(([1_usize, 128, frames], data.clone()))
                .map_err(|error| model_error("parakeet-tdt-v3-encoder", error))?;
            let lengths = Tensor::from_array(([1_usize], vec![frames as i64]))
                .map_err(|error| model_error("parakeet-tdt-v3-encoder", error))?;
            let outputs = session
                .run(ort::inputs!["audio_signal" => audio, "length" => lengths])
                .map_err(|error| model_error("parakeet-tdt-v3-encoder", error))?;
            let (shape, encoded) = outputs["outputs"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("parakeet-tdt-v3-encoder", error))?;
            let (_, encoded_lengths) = outputs["encoded_lengths"]
                .try_extract_tensor::<i64>()
                .map_err(|error| model_error("parakeet-tdt-v3-encoder", error))?;
            if shape.len() != 3 || shape[1] as usize != ENCODER_DIMS {
                return Err(Error::Model {
                    model: "parakeet-tdt-v3-encoder".to_owned(),
                    message: format!(
                        "expected encoder output [1, {ENCODER_DIMS}, time], got {shape:?}"
                    ),
                });
            }
            let encoded_frames = shape[2] as usize;
            let output_length = encoded_lengths
                .first()
                .copied()
                .map(|length| length.max(0) as usize)
                .ok_or_else(|| Error::Model {
                    model: "parakeet-tdt-v3-encoder".to_owned(),
                    message: "encoder returned no output length".to_owned(),
                })?
                .min(encoded_frames);
            let mut time_major = vec![0.0; encoded_frames * ENCODER_DIMS];
            for frame in 0..encoded_frames {
                for dimension in 0..ENCODER_DIMS {
                    time_major[frame * ENCODER_DIMS + dimension] =
                        encoded[dimension * encoded_frames + frame];
                }
            }
            Ok((time_major, output_length))
        })?;
        self.info.backend = self.encoder.backend().to_string();
        Ok((encoded, encoded_len))
    }

    fn decode_step(
        &mut self,
        frame: &[f32],
        token: usize,
        state1: &[f32],
        state2: &[f32],
    ) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        let frame = frame.to_vec();
        let state1_data = state1.to_vec();
        let state2_data = state2.to_vec();
        self.decoder_joint.run_mapped(|session| {
            let encoded = Tensor::from_array(([1_usize, ENCODER_DIMS, 1], frame.clone()))
                .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            let token = Tensor::from_array(([1_usize, 1], vec![token as i32]))
                .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            let token_length = Tensor::from_array(([1_usize], vec![1_i32]))
                .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            let state1 =
                Tensor::from_array(([STATE_COUNT, 1, PREDICTOR_DIMS], state1_data.clone()))
                    .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            let state2 =
                Tensor::from_array(([STATE_COUNT, 1, PREDICTOR_DIMS], state2_data.clone()))
                    .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            let outputs = session
                .run(ort::inputs![
                    "encoder_outputs" => encoded,
                    "targets" => token,
                    "target_length" => token_length,
                    "input_states_1" => state1,
                    "input_states_2" => state2
                ])
                .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            let (_, decoder_output) = outputs["outputs"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            let (_, next_state1) = outputs["output_states_1"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            let (_, next_state2) = outputs["output_states_2"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error("parakeet-tdt-v3-decoder-joint", error))?;
            Ok((
                decoder_output.to_vec(),
                next_state1.to_vec(),
                next_state2.to_vec(),
            ))
        })
    }
}

impl SpeechRecognizer for ParakeetTdt {
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
        if features.channels != 128 {
            return Err(Error::Model {
                model: "parakeet-tdt-v3-preprocessor".to_owned(),
                message: format!("expected 128 mel bins, got {}", features.channels),
            });
        }
        let (encoded, encoded_len) = self.encode(&features.data, features.frames)?;
        let blank_id = self.vocabulary.blank_id;
        let mut state1 = vec![0.0; STATE_COUNT * PREDICTOR_DIMS];
        let mut state2 = vec![0.0; STATE_COUNT * PREDICTOR_DIMS];
        let mut tokens = Vec::new();
        let mut frame_index = 0;
        let mut last_token = blank_id;
        while frame_index < encoded_len {
            let frame = &encoded[frame_index * ENCODER_DIMS..(frame_index + 1) * ENCODER_DIMS];
            let mut symbols = 0;
            loop {
                let (output, next_state1, next_state2) =
                    self.decode_step(frame, last_token, &state1, &state2)?;
                let vocab_size = self.vocabulary.tokens.len();
                if output.len() < vocab_size + 5 {
                    return Err(Error::Model {
                        model: "parakeet-tdt-v3-decoder-joint".to_owned(),
                        message: format!(
                            "expected {vocab_size} token and 5 duration logits, got {}",
                            output.len()
                        ),
                    });
                }
                let (token, _) = argmax(&output[..vocab_size])?;
                let (duration, _) = argmax(&output[vocab_size..vocab_size + 5])?;
                if token != blank_id {
                    tokens.push(TokenTiming {
                        id: token,
                        frame: frame_index,
                        end_frame: frame_index + duration.max(1),
                    });
                    state1 = next_state1;
                    state2 = next_state2;
                    last_token = token;
                    symbols += 1;
                }
                if duration > 0 {
                    frame_index = (frame_index + duration).min(encoded_len);
                    break;
                }
                if token == blank_id || symbols >= MAX_SYMBOLS_PER_STEP {
                    frame_index += 1;
                    break;
                }
            }
        }
        let recognized = recognized_from_tokens(&tokens, &self.vocabulary, FRAME_MS, pcm.len())?;
        self.info.backend = backend_summary([
            self.preprocessor.backend(),
            self.encoder.backend(),
            self.decoder_joint.backend(),
        ]);
        Ok(recognized)
    }
}
