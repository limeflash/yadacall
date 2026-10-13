use std::path::Path;

use ort::value::Tensor;
use yc_core::{
    Error, Result,
    transcript::{Recognized, Word},
};

use crate::{
    Backend,
    backend::{AdaptiveSession, model_cache_dir},
};

use super::required_file;

pub const SAMPLE_RATE: usize = 16_000;
pub const MAX_CHUNK_MS: u64 = 20_000;
const GIGAAM_PREPROCESSOR: &[u8] = include_bytes!("preprocessors/data/gigaam_v3.onnx");

pub struct Preprocessor {
    session: AdaptiveSession,
    name: String,
}

impl Preprocessor {
    pub fn gigaam(dir: &Path, backend: Backend) -> Result<Self> {
        Self::embedded(GIGAAM_PREPROCESSOR, "gigaam-v3-preprocessor", dir, backend)
    }

    pub fn parakeet(dir: &Path, backend: Backend, filename: &str) -> Result<Self> {
        let path = required_file(dir, filename)?;
        let session = AdaptiveSession::from_file(
            &path,
            backend,
            &model_cache_dir(dir, "parakeet-v3-preprocessor"),
        )?;
        Ok(Self {
            session,
            name: "parakeet-v3-preprocessor".to_owned(),
        })
    }

    fn embedded(bytes: &[u8], name: &str, dir: &Path, backend: Backend) -> Result<Self> {
        let session = AdaptiveSession::from_memory(bytes, backend, &model_cache_dir(dir, name))?;
        Ok(Self {
            session,
            name: name.to_owned(),
        })
    }

    pub fn compute(&mut self, pcm: &[f32]) -> Result<Features> {
        if pcm.is_empty() {
            return Err(Error::Invalid(
                "ASR input audio must not be empty".to_owned(),
            ));
        }
        let samples = pcm.to_vec();
        let length = pcm.len() as i64;
        let name = self.name.clone();
        let features = self.session.run_mapped(|session| {
            let waveform = Tensor::from_array(([1_usize, samples.len()], samples.clone()))
                .map_err(|error| model_error(&name, error))?;
            let waveform_length = Tensor::from_array(([1_usize], vec![length]))
                .map_err(|error| model_error(&name, error))?;
            let outputs = session
                .run(ort::inputs!["waveforms" => waveform, "waveforms_lens" => waveform_length])
                .map_err(|error| model_error(&name, error))?;
            let (shape, data) = outputs["features"]
                .try_extract_tensor::<f32>()
                .map_err(|error| model_error(&name, error))?;
            if shape.len() != 3 || shape[0] != 1 {
                return Err(Error::Model {
                    model: name.clone(),
                    message: format!(
                        "expected preprocessor features [1, mel_bins, time], got {shape:?}"
                    ),
                });
            }
            let (_, lengths) = outputs["features_lens"]
                .try_extract_tensor::<i64>()
                .map_err(|error| model_error(&name, error))?;
            let feature_frames = usize::try_from(*lengths.first().ok_or_else(|| Error::Model {
                model: name.clone(),
                message: "preprocessor returned no feature length".to_owned(),
            })?)
            .map_err(|_| Error::Model {
                model: name.clone(),
                message: "negative feature length".to_owned(),
            })?;
            Ok(Features {
                channels: shape[1] as usize,
                data: data.to_vec(),
                frames: feature_frames,
            })
        })?;
        Ok(features)
    }

    pub fn backend(&self) -> Backend {
        self.session.backend()
    }
}

pub struct Features {
    pub channels: usize,
    pub data: Vec<f32>,
    pub frames: usize,
}

pub struct Vocabulary {
    pub tokens: Vec<String>,
    pub blank_id: usize,
}

impl Vocabulary {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut rows = Vec::new();
        for (line_number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let (token, id) = line.rsplit_once(' ').ok_or_else(|| {
                Error::Invalid(format!(
                    "vocabulary line {} has no token id",
                    line_number + 1
                ))
            })?;
            let id = id.trim().parse::<usize>().map_err(|_| {
                Error::Invalid(format!(
                    "vocabulary line {} has an invalid token id",
                    line_number + 1
                ))
            })?;
            rows.push((id, token.replace('\u{2581}', " ")));
        }
        let size = rows.iter().map(|(id, _)| id + 1).max().unwrap_or(0);
        if size == 0 {
            return Err(Error::Invalid(format!(
                "vocabulary `{}` is empty",
                path.display()
            )));
        }
        let mut tokens = vec![String::new(); size];
        let mut blank_id = None;
        for (id, token) in rows {
            if token == "<blk>" || token == "<blank>" {
                blank_id = Some(id);
            }
            tokens[id] = token;
        }
        let blank_id = blank_id.unwrap_or(size - 1);
        Ok(Self { tokens, blank_id })
    }

    pub fn token(&self, id: usize) -> Result<&str> {
        self.tokens
            .get(id)
            .map(String::as_str)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| Error::Model {
                model: "asr-vocabulary".to_owned(),
                message: format!("token id {id} is missing"),
            })
    }
}

#[derive(Debug, Clone)]
pub struct TokenTiming {
    pub id: usize,
    pub frame: usize,
    pub end_frame: usize,
}

pub fn recognized_from_tokens(
    tokens: &[TokenTiming],
    vocabulary: &Vocabulary,
    frame_ms: u64,
    audio_samples: usize,
) -> Result<Vec<Recognized>> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let fragments = tokens
        .iter()
        .map(|timing| vocabulary.token(timing.id))
        .collect::<Result<Vec<_>>>()?;
    let text = fragments
        .concat()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let audio_ms = (audio_samples as u64).saturating_mul(1000) / SAMPLE_RATE as u64;
    let mut words = Vec::new();
    let mut current_word: Option<Word> = None;
    for (token, fragment) in tokens.iter().zip(fragments) {
        if fragment.chars().next().is_some_and(char::is_whitespace)
            && let Some(word) = current_word.take()
        {
            words.push(word);
        }
        let fragment = fragment.trim();
        if fragment.is_empty() {
            continue;
        }
        let start_ms = (token.frame as u64).saturating_mul(frame_ms).min(audio_ms);
        let end_ms = (token.end_frame as u64)
            .saturating_mul(frame_ms)
            .max((token.frame as u64).saturating_mul(frame_ms) + 1)
            .min(audio_ms);
        if let Some(word) = &mut current_word {
            word.text.push_str(fragment);
            word.end_ms = end_ms.max(word.end_ms);
        } else {
            current_word = Some(Word {
                start_ms,
                end_ms,
                text: fragment.to_owned(),
            });
        }
    }
    if let Some(word) = current_word {
        words.push(word);
    }
    let start_ms = words.first().map(|word| word.start_ms).unwrap_or(0);
    let end_ms = words
        .last()
        .map(|word| word.end_ms)
        .unwrap_or(start_ms)
        .max(start_ms);
    Ok(vec![Recognized {
        start_ms,
        end_ms,
        text,
        words,
        confidence: None,
    }])
}

pub fn create_session(
    dir: &Path,
    name: &str,
    filename: &str,
    backend: Backend,
) -> Result<AdaptiveSession> {
    let path = required_file(dir, filename)?;
    AdaptiveSession::from_file(&path, backend, &model_cache_dir(dir, name))
}

pub fn validate_audio(pcm: &[f32]) -> Result<()> {
    if pcm.is_empty() {
        return Err(Error::Invalid(
            "ASR input audio must not be empty".to_owned(),
        ));
    }
    if pcm.len() > SAMPLE_RATE * 25 {
        return Err(Error::Invalid(
            "ASR model input exceeds its 25-second limit".to_owned(),
        ));
    }
    Ok(())
}

pub fn ensure_vocab_logits(logits: &[f32], vocab_size: usize) -> Result<()> {
    if logits.len() < vocab_size {
        return Err(Error::Model {
            model: "asr-decoder".to_owned(),
            message: format!(
                "decoder produced {} logits for {vocab_size} vocabulary entries",
                logits.len()
            ),
        });
    }
    Ok(())
}

pub fn argmax(values: &[f32]) -> Result<(usize, f32)> {
    values
        .iter()
        .copied()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .ok_or_else(|| Error::Model {
            model: "asr-decoder".to_owned(),
            message: "decoder returned no logits".to_owned(),
        })
}

pub fn model_error(model: &str, error: impl std::fmt::Display) -> Error {
    Error::Model {
        model: model.to_owned(),
        message: error.to_string(),
    }
}

pub fn backend_summary(backends: impl IntoIterator<Item = Backend>) -> String {
    let mut backends = backends.into_iter();
    let Some(first) = backends.next() else {
        return "unknown".to_owned();
    };
    if backends.all(|backend| backend == first) {
        first.to_string()
    } else {
        "mixed".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognized_tokens_are_grouped_into_word_timings() {
        let vocabulary = Vocabulary {
            tokens: vec![
                "<unk>".to_owned(),
                " hello".to_owned(),
                " wor".to_owned(),
                "ld".to_owned(),
                ".".to_owned(),
                " ".to_owned(),
            ],
            blank_id: 0,
        };
        let recognized = recognized_from_tokens(
            &[
                TokenTiming {
                    id: 1,
                    frame: 0,
                    end_frame: 2,
                },
                TokenTiming {
                    id: 2,
                    frame: 3,
                    end_frame: 4,
                },
                TokenTiming {
                    id: 3,
                    frame: 4,
                    end_frame: 5,
                },
                TokenTiming {
                    id: 4,
                    frame: 5,
                    end_frame: 6,
                },
            ],
            &vocabulary,
            40,
            16_000,
        )
        .expect("valid vocabulary");

        assert_eq!(recognized[0].text, "hello world.");
        assert_eq!(recognized[0].words.len(), 2);
        assert_eq!(recognized[0].words[0].text, "hello");
        assert_eq!(recognized[0].words[0].start_ms, 0);
        assert_eq!(recognized[0].words[0].end_ms, 80);
        assert_eq!(recognized[0].words[1].text, "world.");
        assert_eq!(recognized[0].words[1].start_ms, 120);
        assert_eq!(recognized[0].words[1].end_ms, 240);
    }
}
