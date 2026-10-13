//! Calibration of the speaker-merge thresholds on real audio. Ignored by default.
//!
//! Needs `YC_MODELS_DIR` (the dev model cache) and `YC_CALIB_DIR` holding `transcript.json`,
//! `them.f32` / `mic.f32` (raw 16 kHz mono f32le of the two tracks) and `tts/*.f32` (utterances
//! named `<voice>_<n>.f32`). Prints similarity statistics and the clusters the default
//! configuration produces; asserts that the single remote person ends up as one speaker.

use std::collections::BTreeMap;
use std::path::PathBuf;

use yc_core::ports::SpeakerEmbedder;
use yc_engine::diarize::{ClusterConfig, centroid, cluster, cosine};
use yc_ml::{Backend, CampPlusEmbedder};

fn read_f32(path: &std::path::Path) -> Vec<f32> {
    std::fs::read(path)
        .expect("read raw audio")
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn stats(label: &str, values: &[f32]) {
    if values.is_empty() {
        return;
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let min = values.iter().copied().fold(f32::MAX, f32::min);
    let max = values.iter().copied().fold(f32::MIN, f32::max);
    eprintln!(
        "{label}: n={} mean={mean:.3} min={min:.3} max={max:.3}",
        values.len()
    );
}

struct Utt {
    track: String,
    old_speaker: String,
    secs: f32,
    emb: Vec<f32>,
}

#[test]
#[ignore = "needs YC_MODELS_DIR, YC_CALIB_DIR and the CAMPPlus model"]
fn merge_thresholds_on_real_audio() {
    let models = PathBuf::from(std::env::var_os("YC_MODELS_DIR").expect("YC_MODELS_DIR"));
    let calib = PathBuf::from(std::env::var_os("YC_CALIB_DIR").expect("YC_CALIB_DIR"));
    let mut embedder = CampPlusEmbedder::load(
        &models.join("speaker-campplus-zh-en-16k"),
        Backend::Cpu,
    )
    .expect("load CAMPPlus");

    // ---- the real meeting ----
    let transcript: serde_json::Value =
        serde_json::from_slice(&std::fs::read(calib.join("transcript.json")).unwrap()).unwrap();
    let (mic_pcm, them_pcm) = (read_f32(&calib.join("mic.f32")), read_f32(&calib.join("them.f32")));
    let (mic_offset, them_offset) = (1307u64, 2907u64);
    let mut utts: Vec<Utt> = Vec::new();
    for seg in transcript["segments"].as_array().unwrap() {
        let (start, end) = (seg["startMs"].as_u64().unwrap(), seg["endMs"].as_u64().unwrap());
        let track = seg["track"].as_str().unwrap();
        let (pcm, offset) = if track == "mic" {
            (&mic_pcm, mic_offset)
        } else {
            (&them_pcm, them_offset)
        };
        if end - start < 1000 {
            continue;
        }
        let from = ((start.saturating_sub(offset)) * 16) as usize;
        let to = (from + ((end - start).min(15_000) * 16) as usize).min(pcm.len());
        if to <= from + 8000 {
            continue;
        }
        utts.push(Utt {
            track: track.to_owned(),
            old_speaker: seg["speaker"].as_str().unwrap().to_owned(),
            secs: (end - start) as f32 / 1000.0,
            emb: embedder.embed(&pcm[from..to]).expect("embed"),
        });
    }
    let them: Vec<&Utt> = utts.iter().filter(|u| u.track == "system").collect();
    let mic: Vec<&Utt> = utts.iter().filter(|u| u.track == "mic").collect();
    eprintln!("utterances: them={} mic={}", them.len(), mic.len());

    let cen = |sel: &[&Utt]| centroid(sel.iter().map(|u| u.emb.as_slice()));
    let (s1, s2): (Vec<&Utt>, Vec<&Utt>) = (
        them.iter().copied().filter(|u| u.old_speaker == "s1").collect(),
        them.iter().copied().filter(|u| u.old_speaker == "s2").collect(),
    );
    eprintln!(
        "OLD split: s1 n={} s2 n={}  centroid cos(s1,s2)={:.3}  s2 secs={:.1}",
        s1.len(),
        s2.len(),
        cosine(&cen(&s1), &cen(&s2)),
        s2.iter().map(|u| u.secs).sum::<f32>()
    );
    let mic_c = cen(&mic);
    eprintln!(
        "DIFFERENT PEOPLE (me vs them) centroid cos: vs all={:.3} vs s1={:.3} vs s2={:.3}",
        cosine(&mic_c, &cen(&them)),
        cosine(&mic_c, &cen(&s1)),
        cosine(&mic_c, &cen(&s2))
    );
    // Utterance-level distributions.
    let mut within = Vec::new();
    for (i, a) in them.iter().enumerate() {
        for b in &them[i + 1..] {
            within.push(cosine(&a.emb, &b.emb));
        }
    }
    let across: Vec<f32> = mic
        .iter()
        .flat_map(|m| them.iter().map(|t| cosine(&m.emb, &t.emb)))
        .collect();
    stats("utterance pairs, same person (them)", &within);
    stats("utterance pairs, different people (me/them)", &across);
    // Same-person cross-validation: random halves of `them` centroids.
    let (even, odd): (Vec<&Utt>, Vec<&Utt>) = {
        let mut e = Vec::new();
        let mut o = Vec::new();
        for (i, u) in them.iter().enumerate() {
            if i % 2 == 0 { e.push(*u) } else { o.push(*u) }
        }
        (e, o)
    };
    eprintln!(
        "same person, interleaved halves centroid cos={:.3}",
        cosine(&cen(&even), &cen(&odd))
    );

    // Clustering of the remote track: previous behaviour vs the new defaults.
    let vectors: Vec<Vec<f32>> = them.iter().map(|u| u.emb.clone()).collect();
    let secs: Vec<f32> = them.iter().map(|u| u.secs).collect();
    let old = ClusterConfig {
        merge_similarity: 2.0,
        small_cluster_secs: 0.0,
        ..Default::default()
    };
    let describe = |labels: &[usize]| {
        let mut by: BTreeMap<usize, (usize, f32)> = BTreeMap::new();
        for (l, s) in labels.iter().zip(&secs) {
            let e = by.entry(*l).or_default();
            e.0 += 1;
            e.1 += s;
        }
        format!("{by:?}")
    };
    let before = cluster(&vectors, &secs, &old);
    let after = cluster(&vectors, &secs, &ClusterConfig::default());
    eprintln!("clusters before merge pass: {}", describe(&before));
    eprintln!("clusters with defaults:     {}", describe(&after));
    for m in [0.4f32, 0.45, 0.5, 0.55, 0.6, 0.65] {
        let labels = cluster(
            &vectors,
            &secs,
            &ClusterConfig {
                merge_similarity: m,
                ..Default::default()
            },
        );
        eprintln!(
            "  merge_similarity={m}: {} cluster(s)",
            labels.iter().max().map_or(0, |x| x + 1)
        );
    }
    // Two different people in one track: the remote speech plus my mic speech, as one pool.
    let pool: Vec<Vec<f32>> = utts.iter().map(|u| u.emb.clone()).collect();
    let pool_secs: Vec<f32> = utts.iter().map(|u| u.secs).collect();
    let pooled = cluster(&pool, &pool_secs, &ClusterConfig::default());
    let mut table: BTreeMap<(String, usize), usize> = BTreeMap::new();
    for (u, l) in utts.iter().zip(&pooled) {
        *table.entry((u.track.clone(), *l)).or_default() += 1;
    }
    eprintln!("pooled me+them with defaults (track, cluster) -> n: {table:?}");
    assert_eq!(
        after.iter().max().copied(),
        Some(0),
        "one remote person must be one speaker"
    );

    // ---- TTS voices ----
    let mut by_voice: BTreeMap<String, Vec<Vec<f32>>> = BTreeMap::new();
    let mut entries: Vec<_> = std::fs::read_dir(calib.join("tts"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "f32"))
        .collect();
    entries.sort();
    for path in entries {
        let voice = path.file_stem().unwrap().to_string_lossy();
        let voice = voice.rsplit_once('_').unwrap().0.to_owned();
        let emb = embedder.embed(&read_f32(&path)).expect("embed tts");
        by_voice.entry(voice).or_default().push(emb);
    }
    let mut same = Vec::new();
    let mut diff = Vec::new();
    let voices: Vec<(&String, &Vec<Vec<f32>>)> = by_voice.iter().collect();
    for (i, (_, a)) in voices.iter().enumerate() {
        for (x, ea) in a.iter().enumerate() {
            for eb in &a[x + 1..] {
                same.push(cosine(ea, eb));
            }
            for (_, b) in &voices[i + 1..] {
                for eb in b.iter() {
                    diff.push(cosine(ea, eb));
                }
            }
        }
    }
    stats("TTS utterance pairs, same voice", &same);
    stats("TTS utterance pairs, different voice", &diff);
    let cents: Vec<(&String, Vec<f32>)> = voices
        .iter()
        .map(|(n, v)| (*n, centroid(v.iter().map(|e| e.as_slice()))))
        .collect();
    let mut dc = Vec::new();
    for (i, (_, a)) in cents.iter().enumerate() {
        for (_, b) in &cents[i + 1..] {
            dc.push(cosine(a, b));
        }
    }
    stats("TTS voice centroids, different voices", &dc);
    // Real "them" vs TTS voices.
    let them_c = cen(&them);
    let vs_tts: Vec<f32> = cents.iter().map(|(_, c)| cosine(&them_c, c)).collect();
    stats("real remote speaker vs TTS voice centroids", &vs_tts);
}
