//! 3D-Speaker CAMPPlus voice embedding adapter.

use std::path::Path;

use ort::value::Tensor;
use yc_core::{
    Error, Result,
    ports::{ModelInfo, SpeakerEmbedder},
};

use crate::{
    Backend,
    backend::{AdaptiveSession, model_cache_dir},
    fbank::KaldiFbank,
};

const MODEL_FILE: &str = "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx";

/// CAMPPlus ONNX model wrapper with Kaldi-compatible 80-bin FBank input.
pub struct CampPlusEmbedder {
    session: AdaptiveSession,
    info: ModelInfo,
    model_path: String,
    fbank: KaldiFbank,
    dims: usize,
}

impl CampPlusEmbedder {
    /// Loads the model from the sherpa-onnx release directory.
    pub fn load(model_dir: &Path, backend: Backend) -> Result<Self> {
        let path = locate_model(model_dir)?;
        let session = AdaptiveSession::from_file(
            &path,
            backend,
            &model_cache_dir(model_dir, "speaker-campplus"),
        )?;
        let info = ModelInfo {
            id: "speaker-campplus-3dspeaker".to_owned(),
            backend: session.backend().to_string(),
        };
        Ok(Self {
            session,
            info,
            model_path: path.display().to_string(),
            fbank: KaldiFbank::new(),
            dims: 192,
        })
    }
}

impl SpeakerEmbedder for CampPlusEmbedder {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(&mut self, pcm: &[f32]) -> Result<Vec<f32>> {
        let features = self.fbank.compute(pcm);
        if features.is_empty() {
            return Err(Error::Invalid(
                "speaker embedding audio must be at least 25 ms".to_owned(),
            ));
        }
        let frame_count = features.len();
        let feature_data = features.into_iter().flatten().collect::<Vec<_>>();
        let model_path = self.model_path.clone();
        let mut input_names = Vec::new();
        let mut output_name = String::new();
        let embedding = self.session.run_mapped(|session| {
            if input_names.is_empty() {
                input_names = session
                    .inputs()
                    .iter()
                    .map(|input| input.name().to_owned())
                    .collect();
                output_name = session
                    .outputs()
                    .first()
                    .map(|output| output.name().to_owned())
                    .ok_or_else(|| Error::Model {
                        model: "speaker-campplus".to_owned(),
                        message: "model declares no output tensors".to_owned(),
                    })?;
            }
            let feature_shape = feature_shape(session, &input_names[0], frame_count)?;
            let input_data = if feature_shape.0 == [1, 80, frame_count] {
                transpose_features(&feature_data, frame_count)
            } else {
                feature_data.clone()
            };
            let feature_tensor = Tensor::from_array((feature_shape.0, input_data))
                .map_err(|error| model_error(&model_path, error))?;
            let mut named_inputs = vec![(input_names[0].as_str(), feature_tensor.into_dyn())];
            let length_tensor = Tensor::from_array(([1_usize], vec![frame_count as i64]))
                .map_err(|error| model_error(&model_path, error))?;
            let has_lengths = input_names.len() > 1;
            if has_lengths {
                named_inputs.push((input_names[1].as_str(), length_tensor.into_dyn()));
            }
            let outputs = session
                .run(named_inputs)
                .map_err(|error| model_error(&model_path, error))?;
            let (_, embedding) = outputs[output_name.as_str()]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error(&model_path, error))?;
            Ok(embedding.to_vec())
        })?;
        self.info.backend = self.session.backend().to_string();
        let normalized = l2_normalize(embedding);
        self.dims = normalized.len();
        Ok(normalized)
    }
}

fn locate_model(model_dir: &Path) -> Result<std::path::PathBuf> {
    let candidates = [
        model_dir.join(MODEL_FILE),
        model_dir.join("speaker-campplus").join(MODEL_FILE),
        model_dir
            .join("speaker-recongition-models")
            .join(MODEL_FILE),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| Error::ModelMissing(MODEL_FILE.to_owned()))
}

fn feature_shape(
    session: &ort::session::Session,
    name: &str,
    frames: usize,
) -> Result<([usize; 3], bool)> {
    let outlet = session
        .inputs()
        .iter()
        .find(|input| input.name() == name)
        .ok_or_else(|| Error::Model {
            model: "speaker-campplus".to_owned(),
            message: format!("missing input `{name}`"),
        })?;
    let shape = outlet.dtype().tensor_shape().ok_or_else(|| Error::Model {
        model: "speaker-campplus".to_owned(),
        message: format!("input `{name}` is not a tensor"),
    })?;
    if shape.len() != 3 {
        return Err(Error::Model {
            model: "speaker-campplus".to_owned(),
            message: format!("expected a 3D fbank input, got shape {shape:?}"),
        });
    }
    let channel_first = shape[1] == 80;
    if channel_first {
        Ok(([1, 80, frames], true))
    } else {
        Ok(([1, frames, 80], false))
    }
}

fn transpose_features(features: &[f32], frames: usize) -> Vec<f32> {
    let mut transposed = vec![0.0; features.len()];
    for frame in 0..frames {
        for bin in 0..80 {
            transposed[bin * frames + frame] = features[frame * 80 + bin];
        }
    }
    transposed
}

fn l2_normalize(mut embedding: Vec<f32>) -> Vec<f32> {
    let norm = embedding
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if norm > 0.0 {
        for value in &mut embedding {
            *value /= norm;
        }
    }
    embedding
}

fn model_error(model: &str, error: impl std::fmt::Display) -> Error {
    Error::Model {
        model: model.to_owned(),
        message: error.to_string(),
    }
}
