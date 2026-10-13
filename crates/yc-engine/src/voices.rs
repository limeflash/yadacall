//! Naming speakers and learning voices from it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use yc_core::meeting::{MeetingId, Participant};
use yc_core::{Error, Result};
use yc_store::StoreVoice;

use crate::diarize::{from_bytes, normalize, to_bytes};
use crate::engine::Engine;
use crate::participants::hint_email_for_name;
use crate::stages::{SpeakerProfiles, is_diarised_key};

const VOICE_EMAILS_FILE: &str = "voice-emails.json";

/// Email associations are engine metadata alongside the store's voice-centroid table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct VoiceEmailIndex(BTreeMap<String, Vec<String>>);

impl VoiceEmailIndex {
    pub(crate) fn read(library: &Path) -> Self {
        std::fs::read(library.join(VOICE_EMAILS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub(crate) fn emails(&self, voice_id: &str) -> Vec<String> {
        self.0.get(voice_id).cloned().unwrap_or_default()
    }

    fn add(&mut self, voice_id: &str, email: &str) {
        let emails = self.0.entry(voice_id.to_owned()).or_default();
        if !emails
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(email.trim()))
        {
            emails.push(email.trim().to_owned());
            emails.sort_by_key(|value| value.to_lowercase());
        }
    }

    fn write(&self, library: &Path) -> Result<()> {
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|error| Error::Storage(error.to_string()))?;
        crate::meta::write_atomic(&library.join(VOICE_EMAILS_FILE), &bytes)
    }
}

/// What [`Engine::rename_speaker`] did beyond renaming.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameOutcome {
    /// Set when the new name was already used by another speaker of the meeting: the renamed
    /// speaker was merged into that one ("Объединил с Дима").
    pub merged_into: Option<MergedSpeaker>,
}

/// The speaker that absorbed a renamed one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedSpeaker {
    /// Speaker key that now carries all the segments.
    pub key: String,
    /// Its display name.
    pub name: String,
}

fn same_name(a: &str, b: &str) -> bool {
    let fold = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    fold(a) == fold(b)
}

impl Engine {
    /// Names the speaker `key` (`me`, `them`, `s1`, …) in a meeting.
    ///
    /// When voice recognition is on and the speaker was diarised, the voice is saved to (or merged
    /// into) the voice library so the person is recognised in later meetings. Naming a diarised
    /// speaker like another diarised speaker of the same meeting means the clustering split one
    /// person: the two are merged (segments re-keyed, voice centroid and voice profile combined)
    /// and the result says into whom.
    pub async fn rename_speaker(
        self: &Arc<Self>,
        id: MeetingId,
        key: String,
        name: String,
    ) -> Result<RenameOutcome> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(Error::Invalid("a speaker name cannot be empty".into()));
        }
        let outcome = self
            .blocking(move |e| e.rename_speaker_blocking(id, &key, &name))
            .await?;
        self.schedule_reindex(id); // the chunk text carries speaker names
        Ok(outcome)
    }

    fn rename_speaker_blocking(
        &self,
        id: MeetingId,
        key: &str,
        name: &str,
    ) -> Result<RenameOutcome> {
        let rec = self.store.get_meeting(id)?;
        let library = self.library_root();
        let folder = library.join(rec.folder);
        let mut profiles = SpeakerProfiles::read(&folder);
        let learn = self.settings().recognise_voices;
        let email = hint_email_for_name(name, &rec.meeting.participant_hints);
        let (learned, merged_into) = self.mutate_meeting(id, |rec| {
            let participants = &mut rec.meeting.participants;
            // The same name on another diarised speaker: it is one person.
            let twin = if is_diarised_key(key) {
                participants
                    .iter()
                    .find(|p| p.key != key && is_diarised_key(&p.key) && same_name(&p.name, name))
                    .map(|p| (p.key.clone(), p.name.clone(), p.voice_id.clone()))
            } else {
                None
            };
            if let Some((twin_key, twin_name, twin_voice)) = twin {
                let gone_voice = participants
                    .iter()
                    .find(|p| p.key == key)
                    .and_then(|p| p.voice_id.clone());
                participants.retain(|p| p.key != key);
                for segment in &mut rec.transcript.segments {
                    if segment.speaker == key {
                        segment.speaker = twin_key.clone();
                    }
                }
                let gone = profiles.speakers.remove(key);
                if let Some(gone) = &gone
                    && let Some(kept) = profiles.speakers.get_mut(&twin_key)
                {
                    let mut c: Vec<f32> = if kept.centroid.len() == gone.centroid.len() {
                        kept.centroid
                            .iter()
                            .zip(&gone.centroid)
                            .map(|(a, b)| a * kept.samples as f32 + b * gone.samples as f32)
                            .collect()
                    } else {
                        kept.centroid.clone()
                    };
                    normalize(&mut c);
                    kept.centroid = c;
                    kept.samples += gone.samples;
                }
                let mut learned = twin_voice.clone();
                if learn && let Some(kept) = profiles.speakers.get(&twin_key) {
                    learned = match (&twin_voice, &gone_voice) {
                        // Neither was learned yet: one voice from the combined evidence.
                        (None, linked) => Some(self.learn_voice(
                            linked.clone(),
                            &twin_name,
                            &kept.centroid,
                            kept.samples,
                        )?),
                        // The absorbed speaker's evidence is not in the voice yet.
                        (Some(voice), None) => {
                            if let Some(gone) = &gone {
                                self.fold_into_voice(voice, &gone.centroid, gone.samples)?;
                            }
                            Some(voice.clone())
                        }
                        (Some(voice), Some(_)) => Some(voice.clone()),
                    };
                    if let Some(p) = participants.iter_mut().find(|p| p.key == twin_key) {
                        p.voice_id = learned.clone();
                    }
                }
                let merged = MergedSpeaker {
                    key: twin_key,
                    name: twin_name,
                };
                return Ok(((learned, Some(merged)), true));
            }
            let pos = match participants.iter().position(|p| p.key == key) {
                Some(p) => p,
                None => {
                    participants.push(Participant {
                        key: key.to_owned(),
                        name: name.to_owned(),
                        voice_id: None,
                        suggestions: Vec::new(),
                    });
                    participants.len() - 1
                }
            };
            participants[pos].name = name.to_owned();
            participants[pos].suggestions.clear();
            let mut learned = None;
            if learn && let Some(profile) = profiles.speakers.get(key) {
                let linked = participants[pos].voice_id.clone();
                let voice_id =
                    self.learn_voice(linked, name, &profile.centroid, profile.samples)?;
                participants[pos].voice_id = Some(voice_id);
                learned = participants[pos].voice_id.clone();
            }
            Ok(((learned, None), true))
        })?;
        if merged_into.is_some() {
            profiles.write(&folder)?;
        }
        if let (Some(voice_id), Some(email)) = (learned, email) {
            self.remember_voice_email(&voice_id, &email)?;
        }
        Ok(RenameOutcome { merged_into })
    }

    fn remember_voice_email(&self, voice_id: &str, email: &str) -> Result<()> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| Error::Storage("write lock poisoned".into()))?;
        let library = self.library_root();
        let mut contacts = VoiceEmailIndex::read(&library);
        contacts.add(voice_id, email);
        contacts.write(&library)
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
            self.fold_into_voice(&existing.id, centroid, samples)?;
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

    /// Adds `samples` utterances with the given centroid to a saved voice's profile.
    fn fold_into_voice(&self, voice_id: &str, centroid: &[f32], samples: u64) -> Result<()> {
        let voices = self.store.list_voices()?;
        let Some(existing) = voices.iter().find(|v| v.id == voice_id) else {
            return Ok(());
        };
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
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
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
