//! Naming speakers and learning voices from it.

use std::sync::Arc;

use yc_core::meeting::{MeetingId, Participant};
use yc_core::{Error, Result};
use yc_store::StoreVoice;

use crate::diarize::{from_bytes, normalize, to_bytes};
use crate::engine::Engine;
use crate::stages::SpeakerProfiles;

impl Engine {
    /// Names the speaker `key` (`me`, `them`, `s1`, …) in a meeting.
    ///
    /// When voice recognition is on and the speaker was diarised, the voice is saved to (or merged
    /// into) the voice library so the person is recognised in later meetings.
    pub async fn rename_speaker(
        self: &Arc<Self>,
        id: MeetingId,
        key: String,
        name: String,
    ) -> Result<()> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(Error::Invalid("a speaker name cannot be empty".into()));
        }
        self.blocking(move |e| e.rename_speaker_blocking(id, &key, &name))
            .await?;
        self.schedule_reindex(id); // the chunk text carries speaker names
        Ok(())
    }

    fn rename_speaker_blocking(&self, id: MeetingId, key: &str, name: &str) -> Result<()> {
        let folder = self.library_root().join(self.store.get_meeting(id)?.folder);
        let profiles = SpeakerProfiles::read(&folder);
        let learn = self.settings().recognise_voices;
        self.mutate_meeting(id, |rec| {
            let participants = &mut rec.meeting.participants;
            let pos = match participants.iter().position(|p| p.key == key) {
                Some(p) => p,
                None => {
                    participants.push(Participant {
                        key: key.to_owned(),
                        name: name.to_owned(),
                        voice_id: None,
                    });
                    participants.len() - 1
                }
            };
            participants[pos].name = name.to_owned();
            if learn && let Some(profile) = profiles.speakers.get(key) {
                let linked = participants[pos].voice_id.clone();
                let voice_id =
                    self.learn_voice(linked, name, &profile.centroid, profile.samples)?;
                participants[pos].voice_id = Some(voice_id);
            }
            Ok(((), true))
        })
    }

    /// Creates, renames or merges a saved voice; returns its id.
    fn learn_voice(
        &self,
        linked: Option<String>,
        name: &str,
        centroid: &[f32],
        samples: u64,
    ) -> Result<String> {
        let voices = self.store.list_voices()?;
        let now = chrono::Utc::now().to_rfc3339();
        // Already linked to this meeting's speaker: only the name may have changed.
        if let Some(existing) = linked.and_then(|l| voices.iter().find(|v| v.id == l)) {
            self.store.upsert_voice(&StoreVoice {
                name: name.to_owned(),
                updated_at: now,
                ..existing.clone()
            })?;
            return Ok(existing.id.clone());
        }
        // Same person named in another meeting: fold this evidence into their voice.
        if let Some(existing) = voices
            .iter()
            .find(|v| v.name.to_lowercase() == name.to_lowercase())
        {
            let old = from_bytes(&existing.centroid);
            let merged = if old.len() == centroid.len() {
                let mut m: Vec<f32> = old
                    .iter()
                    .zip(centroid)
                    .map(|(a, b)| a * existing.samples as f32 + b * samples as f32)
                    .collect();
                normalize(&mut m);
                m
            } else {
                centroid.to_vec()
            };
            self.store.upsert_voice(&StoreVoice {
                id: existing.id.clone(),
                name: existing.name.clone(),
                centroid: to_bytes(&merged),
                samples: existing.samples + samples,
                updated_at: now,
            })?;
            return Ok(existing.id.clone());
        }
        let id = uuid::Uuid::now_v7().to_string();
        self.store.upsert_voice(&StoreVoice {
            id: id.clone(),
            name: name.to_owned(),
            centroid: to_bytes(centroid),
            samples,
            updated_at: now,
        })?;
        Ok(id)
    }

    /// Saved voices (id, name, number of utterances behind the profile).
    pub async fn list_voices(self: &Arc<Self>) -> Result<Vec<(String, String, u64)>> {
        self.blocking(|e| {
            Ok(e.store
                .list_voices()?
                .into_iter()
                .map(|v| (v.id, v.name, v.samples))
                .collect())
        })
        .await
    }
}
