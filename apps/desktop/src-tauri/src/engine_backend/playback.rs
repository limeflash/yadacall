//! Audio for the transcript player: the meeting's FLAC served through Tauri's asset protocol. With
//! both tracks on disk they are mixed once into a cached FLAC under the app data directory.

use std::path::{Path, PathBuf};

use yc_core::meeting::TrackKind;
use yc_core::{Error, Result};

use super::EngineBackend;
use crate::paths::asset_url;

/// A source track and where it starts on the meeting clock.
struct Source {
    path: PathBuf,
    offset_ms: u64,
}

impl EngineBackend {
    /// Directory holding the cached mixes (inside the asset-protocol scope).
    pub fn playback_dir(&self) -> PathBuf {
        self.data_dir.join("playback")
    }

    /// Folders the webview may load audio from: the library and the mix cache.
    pub fn asset_roots(&self) -> Vec<PathBuf> {
        vec![self.engine().library_root(), self.playback_dir()]
    }

    /// `asset://` URL of the meeting's playable audio, if any is still on disk.
    pub(super) async fn audio_url(&self, detail: &yc_engine::MeetingDetail) -> Option<String> {
        let audio_dir = self
            .engine()
            .library_root()
            .join(&detail.folder)
            .join("audio");
        let mut sources = Vec::new();
        for kind in TrackKind::ALL {
            let path = audio_dir.join(format!("{}.flac", kind.file_stem()));
            if path.is_file() {
                let offset_ms = detail
                    .meeting
                    .tracks
                    .iter()
                    .find(|t| t.kind == kind)
                    .map_or(0, |t| t.offset_ms);
                sources.push(Source { path, offset_ms });
            }
        }
        match sources.len() {
            0 => {
                let _ = std::fs::remove_file(self.mix_path(detail));
                None
            }
            1 => Some(asset_url(&sources[0].path)),
            _ => {
                let out = self.mix_path(detail);
                let fresh = std::fs::metadata(&out).and_then(|m| m.modified()).ok();
                let newest_source = sources
                    .iter()
                    .filter_map(|s| std::fs::metadata(&s.path).and_then(|m| m.modified()).ok())
                    .max();
                let up_to_date = match (fresh, newest_source) {
                    (Some(f), Some(n)) => f >= n,
                    (Some(_), None) => true,
                    _ => false,
                };
                if !up_to_date {
                    let target = out.clone();
                    let mixed = tokio::task::spawn_blocking(move || mix_to_flac(&sources, &target))
                        .await
                        .map_err(|e| Error::Storage(e.to_string()))
                        .and_then(|r| r);
                    if let Err(e) = mixed {
                        tracing::warn!("could not mix the audio of {}: {e}", detail.meeting.id);
                        return None;
                    }
                }
                Some(asset_url(&out))
            }
        }
    }

    fn mix_path(&self, detail: &yc_engine::MeetingDetail) -> PathBuf {
        self.playback_dir()
            .join(format!("{}.flac", detail.meeting.id))
    }
}

/// Sums the tracks (each placed at its offset) into one 16 kHz mono FLAC.
fn mix_to_flac(sources: &[Source], out: &Path) -> Result<()> {
    let mut mix: Vec<i16> = Vec::new();
    for source in sources {
        let mut pos = (source.offset_ms as usize).saturating_mul(16);
        yc_audio::decode_to_16k_mono(&source.path, &mut |block| {
            if mix.len() < pos + block.len() {
                mix.resize(pos + block.len(), 0);
            }
            for s in block {
                // 0.75 per track keeps two simultaneous voices from clipping.
                let v = (s.clamp(-1.0, 1.0) * 0.75 * f32::from(i16::MAX)) as i16;
                mix[pos] = mix[pos].saturating_add(v);
                pos += 1;
            }
            Ok(())
        })?;
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pcm = out.with_extension("pcm.tmp");
    let bytes: Vec<u8> = mix.iter().flat_map(|s| s.to_le_bytes()).collect();
    std::fs::write(&pcm, bytes)?;
    let encoded = yc_audio::encode_flac(&pcm, out);
    let _ = std::fs::remove_file(&pcm);
    encoded.map(|_| ())
}
