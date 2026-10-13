//! Speaker clustering and voice matching. Pure functions on embeddings — no I/O, no models.

/// Clustering knobs.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterConfig {
    /// Stop merging when the closest pair of clusters is farther apart than this (cosine
    /// distance, average linkage). Ignored when `num_speakers` is set.
    pub distance_threshold: f32,
    /// Exact maximum cluster count chosen by the user; ignored when the count came from an invite.
    pub num_speakers: Option<usize>,
    /// Upper bound on clusters when detecting automatically or from an invite.
    pub max_speakers: usize,
    /// A one-segment cluster with less total speech than this (seconds) is folded into the nearest
    /// other cluster. Clusters with at least two segments always meet the minimum evidence rule.
    pub min_cluster_secs: f32,
    /// After clustering, clusters whose centroids are at least this cosine-similar are one person
    /// and are merged (closest pair first). Ignored when `num_speakers` is set.
    pub merge_similarity: f32,
    /// A cluster with less than this share of all the speech *and* under `small_cluster_secs` is
    /// folded into its nearest cluster (see [`ClusterConfig::distinct_similarity`]).
    pub small_cluster_share: f32,
    /// Seconds of speech under which a cluster counts as small.
    pub small_cluster_secs: f32,
    /// A small cluster whose best centroid similarity is below this is clearly someone else and
    /// is kept.
    pub distinct_similarity: f32,
}

/// Default for [`ClusterConfig::merge_similarity`].
///
/// Calibrated with CAMPPlus (192-d) on a real 6-minute 1:1 call (`tests/calibration.rs`, ignored;
/// 19 remote and 36 own utterances of at least 1 s) plus macOS TTS voices:
///
/// * one remote person, utterance pairs: mean 0.56, min 0.19, max 0.88; interleaved halves of the
///   same person: centroid cosine 0.94; the pair that the old clustering had split into "two
///   Dimas" (17 + 2 utterances, 8.6 s): centroid cosine 0.567;
/// * different people (own mic vs remote), utterance pairs: mean 0.18, max 0.44; centroids 0.31
///   against the whole remote side and 0.16 against the small split-off cluster;
/// * TTS, same voice on different sentences: mean 0.90, min 0.83; different voices: mean 0.30,
///   centroids mean 0.32, lowest 0.08 (one pair of installed voices rendered identically, cosine 1.0).
///
/// So different people stay below ~0.45 (utterances) and 0.31 (centroids), while one person
/// split by the clustering lands at 0.55 or more: 0.5 separates them. The floor under which a
/// small cluster is "clearly someone else" is 0.25 (`distinct_similarity`).
pub const DEFAULT_MERGE_SIMILARITY: f32 = 0.5;

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            distance_threshold: 0.55,
            num_speakers: None,
            max_speakers: 10,
            min_cluster_secs: 8.0,
            merge_similarity: DEFAULT_MERGE_SIMILARITY,
            small_cluster_share: 0.15,
            small_cluster_secs: 20.0,
            distinct_similarity: 0.25,
        }
    }
}

/// Cosine similarity of two vectors (0 when either is the zero vector).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Mean of `vectors`, L2-normalised (empty input → empty vector).
pub fn centroid<'a>(vectors: impl IntoIterator<Item = &'a [f32]>) -> Vec<f32> {
    let mut sum: Vec<f32> = Vec::new();
    let mut n = 0usize;
    for v in vectors {
        if sum.is_empty() {
            sum = vec![0.0; v.len()];
        }
        for (s, x) in sum.iter_mut().zip(v) {
            *s += x;
        }
        n += 1;
    }
    if n == 0 {
        return sum;
    }
    normalize(&mut sum);
    sum
}

/// Scales `v` to unit length in place (zero vectors are left alone).
pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v {
            *x /= n;
        }
    }
}

/// Agglomerative clustering with average linkage on cosine distance.
///
/// Returns one label per input, numbered `0..k` in order of first appearance (so the first speaker
/// to talk is always speaker 0). `durations_secs` (same length) feeds the tiny-cluster fold; pass
/// an empty slice to disable it.
pub fn cluster(embeddings: &[Vec<f32>], durations_secs: &[f32], cfg: &ClusterConfig) -> Vec<usize> {
    let n = embeddings.len();
    if n == 0 {
        return Vec::new();
    }
    let mut dist = vec![0.0f32; n * n];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = 1.0 - cosine(&embeddings[i], &embeddings[j]);
            dist[i * n + j] = d;
            dist[j * n + i] = d;
        }
    }
    let mut members: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut active: Vec<bool> = vec![true; n];
    let mut count = n;
    let target = cfg.num_speakers.map(|k| k.max(1));

    // Nearest active neighbour of every active cluster.
    let nearest = |i: usize, dist: &[f32], active: &[bool]| -> Option<(usize, f32)> {
        (0..n)
            .filter(|&j| j != i && active[j])
            .map(|j| (j, dist[i * n + j]))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    };
    let mut nn: Vec<Option<(usize, f32)>> = (0..n).map(|i| nearest(i, &dist, &active)).collect();

    while count > 1 {
        // Closest pair overall.
        let Some((i, (j, d))) = (0..n)
            .filter(|&i| active[i])
            .filter_map(|i| nn[i].map(|x| (i, x)))
            .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
        else {
            break;
        };
        let must_merge = if let Some(k) = target {
            count > k
        } else {
            count > cfg.max_speakers.max(1) || d <= cfg.distance_threshold
        };
        if !must_merge {
            break;
        }
        // Merge j into i (Lance–Williams update for average linkage).
        let (ni, nj) = (members[i].len() as f32, members[j].len() as f32);
        for k in 0..n {
            if !active[k] || k == i || k == j {
                continue;
            }
            let d = (ni * dist[i * n + k] + nj * dist[j * n + k]) / (ni + nj);
            dist[i * n + k] = d;
            dist[k * n + i] = d;
        }
        let moved = std::mem::take(&mut members[j]);
        members[i].extend(moved);
        active[j] = false;
        count -= 1;
        nn[j] = None;
        for k in 0..n {
            if !active[k] {
                continue;
            }
            let stale = k == i || nn[k].is_some_and(|(t, _)| t == i || t == j);
            if stale {
                nn[k] = nearest(k, &dist, &active);
            } else if let Some((_, best)) = nn[k]
                && dist[k * n + i] < best
            {
                nn[k] = Some((i, dist[k * n + i]));
            }
        }
    }

    let mut labels = vec![0usize; n];
    for (c, m) in members.iter().enumerate() {
        for &i in m {
            labels[i] = c;
        }
    }
    if !durations_secs.is_empty() {
        fold_tiny(
            &mut labels,
            embeddings,
            durations_secs,
            cfg.min_cluster_secs,
        );
    }
    if cfg.num_speakers.is_none() {
        merge_close(&mut labels, embeddings, cfg.merge_similarity);
        if !durations_secs.is_empty() {
            fold_small(&mut labels, embeddings, durations_secs, cfg);
        }
    }
    relabel_by_first_appearance(&labels)
}

/// Distinct labels in ascending order.
fn distinct(labels: &[usize]) -> Vec<usize> {
    let mut v = labels.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

/// Centroid of the embeddings carrying `label`.
fn label_centroid(labels: &[usize], embeddings: &[Vec<f32>], label: usize) -> Vec<f32> {
    centroid(
        (0..labels.len())
            .filter(|&i| labels[i] == label)
            .map(|i| embeddings[i].as_slice()),
    )
}

/// Merges the closest pair of clusters (by centroid cosine) again and again while that pair is at
/// least `min_similarity` alike. Average linkage over single utterances is stricter than centroid
/// similarity, so one person who sounded different on a few utterances ends up as several clusters
/// that this pass rejoins.
pub fn merge_close(labels: &mut [usize], embeddings: &[Vec<f32>], min_similarity: f32) {
    loop {
        let ids = distinct(labels);
        if ids.len() < 2 {
            return;
        }
        let cents: Vec<Vec<f32>> = ids
            .iter()
            .map(|&c| label_centroid(labels, embeddings, c))
            .collect();
        let mut best: Option<(usize, usize, f32)> = None;
        for a in 0..ids.len() {
            for b in (a + 1)..ids.len() {
                let s = cosine(&cents[a], &cents[b]);
                if best.is_none_or(|(_, _, top)| s > top) {
                    best = Some((a, b, s));
                }
            }
        }
        let Some((a, b, s)) = best else { return };
        if s < min_similarity {
            return;
        }
        let (keep, gone) = (ids[a], ids[b]);
        for l in labels.iter_mut() {
            if *l == gone {
                *l = keep;
            }
        }
    }
}

/// Folds each small cluster (under `small_cluster_share` of all speech and under
/// `small_cluster_secs`) into its nearest cluster, unless even the nearest one is below
/// `distinct_similarity` — then it is plausibly another person who barely spoke.
fn fold_small(
    labels: &mut [usize],
    embeddings: &[Vec<f32>],
    durations: &[f32],
    cfg: &ClusterConfig,
) {
    let secs_of = |labels: &[usize], c: usize| -> f32 {
        (0..labels.len())
            .filter(|&i| labels[i] == c)
            .map(|i| durations.get(i).copied().unwrap_or(0.0))
            .sum()
    };
    let total: f32 = (0..labels.len())
        .map(|i| durations.get(i).copied().unwrap_or(0.0))
        .sum();
    let mut kept: Vec<usize> = Vec::new(); // small but distinct: never reconsidered
    loop {
        let ids = distinct(labels);
        if ids.len() < 2 {
            return;
        }
        let smallest = ids
            .iter()
            .copied()
            .filter(|c| !kept.contains(c))
            .map(|c| (c, secs_of(labels, c)))
            .filter(|&(_, secs)| {
                secs < cfg.small_cluster_secs && secs < cfg.small_cluster_share * total
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((small, _)) = smallest else { return };
        let small_c = label_centroid(labels, embeddings, small);
        let nearest = ids
            .iter()
            .copied()
            .filter(|&c| c != small)
            .map(|c| (c, cosine(&small_c, &label_centroid(labels, embeddings, c))))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match nearest {
            Some((into, sim)) if sim >= cfg.distinct_similarity => {
                for l in labels.iter_mut() {
                    if *l == small {
                        *l = into;
                    }
                }
            }
            _ => kept.push(small),
        }
    }
}

/// Finds a cluster that should be absorbed because it matches a saved voice that a *different*
/// cluster already holds (voices are matched one-to-one, so the second hit stays unmatched).
/// Returns `(absorbed, into)` cluster indices when such a pair exists and the two clusters are
/// not clearly different people (`distinct_similarity`).
pub fn same_voice_merge(
    centroids: &[Vec<f32>],
    known: &[KnownVoice],
    min_voice_similarity: f32,
    distinct_similarity: f32,
) -> Option<(usize, usize)> {
    let matched = match_voices(centroids, known, min_voice_similarity);
    let mut best: Option<(usize, usize, f32)> = None;
    for (c, cen) in centroids.iter().enumerate() {
        if matched[c].is_some() {
            continue;
        }
        for (holder, voice) in matched.iter().enumerate() {
            let Some(v) = *voice else { continue };
            let k = &known[v];
            if k.centroid.len() != cen.len() {
                continue;
            }
            let s = cosine(cen, &k.centroid);
            if s >= min_voice_similarity
                && cosine(cen, &centroids[holder]) >= distinct_similarity
                && best.is_none_or(|(_, _, top)| s > top)
            {
                best = Some((c, holder, s));
            }
        }
    }
    best.map(|(c, holder, _)| (c, holder))
}

/// Renumbers labels `0..k` by order of first appearance.
pub fn relabel_by_first_appearance(labels: &[usize]) -> Vec<usize> {
    let mut map: Vec<(usize, usize)> = Vec::new();
    labels
        .iter()
        .map(|&l| match map.iter().find(|(old, _)| *old == l) {
            Some((_, new)) => *new,
            None => {
                let new = map.len();
                map.push((l, new));
                new
            }
        })
        .collect()
}

fn fold_tiny(labels: &mut [usize], embeddings: &[Vec<f32>], durations: &[f32], min_secs: f32) {
    loop {
        let ids: Vec<usize> = {
            let mut v: Vec<usize> = labels.to_vec();
            v.sort_unstable();
            v.dedup();
            v
        };
        if ids.len() < 2 {
            return;
        }
        let tiny = ids.iter().copied().find(|&c| {
            let idx: Vec<usize> = (0..labels.len()).filter(|&i| labels[i] == c).collect();
            let secs: f32 = idx
                .iter()
                .map(|&i| durations.get(i).copied().unwrap_or(0.0))
                .sum();
            idx.len() < 2 && secs < min_secs
        });
        let Some(tiny) = tiny else { return };
        let tiny_c = centroid(
            (0..labels.len())
                .filter(|&i| labels[i] == tiny)
                .map(|i| embeddings[i].as_slice()),
        );
        let best = ids
            .iter()
            .copied()
            .filter(|&c| c != tiny)
            .map(|c| {
                let cc = centroid(
                    (0..labels.len())
                        .filter(|&i| labels[i] == c)
                        .map(|i| embeddings[i].as_slice()),
                );
                (c, cosine(&tiny_c, &cc))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| c);
        let Some(best) = best else { return };
        for l in labels.iter_mut() {
            if *l == tiny {
                *l = best;
            }
        }
    }
}

/// A saved voice profile.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownVoice {
    pub id: String,
    pub name: String,
    /// Email addresses learned from calendar-backed speaker renames.
    pub emails: Vec<String>,
    pub centroid: Vec<f32>,
}

/// Matches cluster centroids to saved voices: the best similarities first, each voice and each
/// cluster used at most once, nothing below `min_similarity`. Returns the voice index per cluster.
pub fn match_voices(
    centroids: &[Vec<f32>],
    known: &[KnownVoice],
    min_similarity: f32,
) -> Vec<Option<usize>> {
    let mut pairs: Vec<(usize, usize, f32)> = Vec::new();
    for (c, cen) in centroids.iter().enumerate() {
        for (v, voice) in known.iter().enumerate() {
            if voice.centroid.len() != cen.len() || cen.is_empty() {
                continue;
            }
            let s = cosine(cen, &voice.centroid);
            if s >= min_similarity {
                pairs.push((c, v, s));
            }
        }
    }
    pairs.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut out = vec![None; centroids.len()];
    let mut used = vec![false; known.len()];
    for (c, v, _) in pairs {
        if out[c].is_none() && !used[v] {
            out[c] = Some(v);
            used[v] = true;
        }
    }
    out
}

/// Gives every segment without an embedding the label of the temporally nearest segment that has
/// one. `spans` are `(start_ms, end_ms)`; with no labelled segment at all everything is label 0.
pub fn assign_unlabelled(spans: &[(u64, u64)], labels: &[Option<usize>]) -> Vec<usize> {
    let labelled: Vec<usize> = (0..spans.len()).filter(|&i| labels[i].is_some()).collect();
    (0..spans.len())
        .map(|i| {
            if let Some(l) = labels[i] {
                return l;
            }
            labelled
                .iter()
                .min_by_key(|&&j| gap(spans[i], spans[j]))
                .and_then(|&j| labels[j])
                .unwrap_or(0)
        })
        .collect()
}

fn gap(a: (u64, u64), b: (u64, u64)) -> u64 {
    if a.1 <= b.0 {
        b.0 - a.1
    } else {
        a.0.saturating_sub(b.1)
    }
}

/// Packs an embedding as little-endian f32 bytes (the format of `voices.centroid`).
pub fn to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Inverse of [`to_bytes`]; a trailing partial float is ignored.
pub fn from_bytes(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-noise in [-1, 1].
    fn noise(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
    }

    /// `n` unit vectors near direction `base` with the given jitter.
    fn blob(base: [f32; 4], n: usize, jitter: f32, seed: &mut u32) -> Vec<Vec<f32>> {
        (0..n)
            .map(|_| {
                let mut v: Vec<f32> = base.iter().map(|x| x + jitter * noise(seed)).collect();
                normalize(&mut v);
                v
            })
            .collect()
    }

    fn three_speakers() -> (Vec<Vec<f32>>, Vec<usize>) {
        let mut seed = 7;
        let mut all = Vec::new();
        let mut truth = Vec::new();
        // Interleave speakers: 0,1,2,0,1,2,… so first-appearance order is 0,1,2.
        let a = blob([1.0, 0.0, 0.0, 0.0], 8, 0.15, &mut seed);
        let b = blob([0.0, 1.0, 0.0, 0.0], 8, 0.15, &mut seed);
        let c = blob([0.0, 0.0, 1.0, 0.2], 8, 0.15, &mut seed);
        for i in 0..8 {
            for (k, set) in [&a, &b, &c].iter().enumerate() {
                all.push(set[i].clone());
                truth.push(k);
            }
        }
        (all, truth)
    }

    #[test]
    fn finds_three_speakers_by_threshold() {
        let (emb, truth) = three_speakers();
        let labels = cluster(&emb, &[], &ClusterConfig::default());
        assert_eq!(labels, truth);
    }

    #[test]
    fn honours_expected_speaker_count() {
        let (emb, _) = three_speakers();
        let two = cluster(
            &emb,
            &[],
            &ClusterConfig {
                num_speakers: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(two.iter().copied().max(), Some(1));
        let one = cluster(
            &emb,
            &[],
            &ClusterConfig {
                num_speakers: Some(1),
                ..Default::default()
            },
        );
        assert!(one.iter().all(|&l| l == 0));
        // More speakers than the data supports: asking for 3 yields the true 3 even if the
        // threshold would have merged them.
        let loose = cluster(
            &emb,
            &[],
            &ClusterConfig {
                distance_threshold: 2.0,
                num_speakers: Some(3),
                ..Default::default()
            },
        );
        assert_eq!(loose.iter().copied().max(), Some(2));
    }

    #[test]
    fn invite_count_caps_clusters_but_explicit_count_is_exact() {
        let embeddings = vec![
            vec![1.0, 0.0],
            vec![0.99, 0.01],
            vec![0.0, 1.0],
            vec![0.01, 0.99],
        ];
        let durations = [4.0; 4];
        let invite_bound = cluster(
            &embeddings,
            &durations,
            &ClusterConfig {
                distance_threshold: 2.0,
                max_speakers: 2,
                ..Default::default()
            },
        );
        assert!(invite_bound.iter().all(|&label| label == 0));

        let explicit_count = cluster(
            &embeddings,
            &durations,
            &ClusterConfig {
                distance_threshold: 2.0,
                num_speakers: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(explicit_count.iter().copied().max(), Some(1));
    }

    #[test]
    fn one_segment_clusters_need_eight_seconds_of_speech() {
        let embeddings = vec![vec![1.0, 0.0], vec![0.99, 0.01], vec![0.0, 1.0]];
        let folded = cluster(
            &embeddings,
            &[4.0, 4.0, 7.9],
            &ClusterConfig {
                distance_threshold: 0.1,
                ..Default::default()
            },
        );
        assert_eq!(folded.iter().copied().max(), Some(0));

        let retained = cluster(
            &embeddings,
            &[4.0, 4.0, 8.0],
            &ClusterConfig {
                distance_threshold: 0.1,
                ..Default::default()
            },
        );
        assert_eq!(retained.iter().copied().max(), Some(1));
    }

    #[test]
    fn max_speakers_caps_detection() {
        let (emb, _) = three_speakers();
        let labels = cluster(
            &emb,
            &[],
            &ClusterConfig {
                distance_threshold: 0.01,
                max_speakers: 2,
                ..Default::default()
            },
        );
        assert_eq!(labels.iter().copied().max(), Some(1));
    }

    #[test]
    fn tiny_outlier_cluster_is_folded() {
        let (mut emb, _) = three_speakers();
        let mut dur = vec![4.0; emb.len()];
        emb.push(vec![0.0, 0.0, 0.0, 1.0]); // a lone, short outlier
        dur.push(1.0);
        let with_fold = cluster(&emb, &dur, &ClusterConfig::default());
        assert_eq!(with_fold.iter().copied().max(), Some(2), "outlier absorbed");
        let without = cluster(&emb, &[], &ClusterConfig::default());
        assert_eq!(without.iter().copied().max(), Some(3));
    }

    /// Unit vector at `degrees` in the first plane of a 4-d space, with a little jitter.
    fn at(degrees: f32, seed: &mut u32) -> Vec<f32> {
        let r = degrees.to_radians();
        let mut v = vec![
            r.cos() + 0.02 * noise(seed),
            r.sin() + 0.02 * noise(seed),
            0.02 * noise(seed),
            0.02 * noise(seed),
        ];
        normalize(&mut v);
        v
    }

    /// 17 utterances at 0 degrees and 2 at 50 degrees (centroid cosine ~0.64).
    fn split_person() -> (Vec<Vec<f32>>, Vec<f32>) {
        let mut seed = 3;
        let mut emb: Vec<Vec<f32>> = (0..17).map(|_| at(0.0, &mut seed)).collect();
        emb.extend((0..2).map(|_| at(50.0, &mut seed)));
        let mut secs = vec![5.0; 17];
        secs.extend([4.3, 4.3]);
        (emb, secs)
    }

    fn strict() -> ClusterConfig {
        // Nothing merges in the agglomerative step, as with the person split in the field data.
        ClusterConfig {
            distance_threshold: 0.05,
            merge_similarity: 2.0,
            small_cluster_secs: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn one_person_split_in_two_is_rejoined_by_centroid_similarity() {
        let (emb, secs) = split_person();
        let before = cluster(&emb, &secs, &strict());
        assert_eq!(before.iter().copied().max(), Some(1), "premise: split");
        let merged = cluster(
            &emb,
            &secs,
            &ClusterConfig {
                merge_similarity: 0.5,
                small_cluster_secs: 0.0,
                ..strict()
            },
        );
        assert!(merged.iter().all(|&l| l == 0));
        // A stricter bar keeps them apart.
        let kept = cluster(
            &emb,
            &secs,
            &ClusterConfig {
                merge_similarity: 0.9,
                small_cluster_secs: 0.0,
                ..strict()
            },
        );
        assert_eq!(kept.iter().copied().max(), Some(1));
    }

    #[test]
    fn small_cluster_is_folded_unless_clearly_another_person() {
        let (emb, secs) = split_person();
        let folded = cluster(
            &emb,
            &secs,
            &ClusterConfig {
                merge_similarity: 0.9,
                small_cluster_secs: 20.0,
                ..strict()
            },
        );
        assert!(folded.iter().all(|&l| l == 0), "8.6 s of 94 s is small");

        // A short cluster at 90 degrees (cosine ~0 to the big one) is someone else.
        let mut seed = 5;
        let mut emb: Vec<Vec<f32>> = (0..17).map(|_| at(0.0, &mut seed)).collect();
        emb.extend((0..2).map(|_| at(90.0, &mut seed)));
        let kept = cluster(
            &emb,
            &secs,
            &ClusterConfig {
                small_cluster_secs: 20.0,
                ..strict()
            },
        );
        assert_eq!(kept.iter().copied().max(), Some(1));

        // A big second cluster is never "small", whatever its similarity.
        let big = vec![5.0; emb.len()];
        let mut two_big = emb.clone();
        two_big.truncate(19);
        let labels = cluster(
            &two_big,
            &big,
            &ClusterConfig {
                merge_similarity: 0.9,
                small_cluster_share: 0.01,
                small_cluster_secs: 20.0,
                ..strict()
            },
        );
        assert_eq!(labels.iter().copied().max(), Some(1));
    }

    #[test]
    fn explicit_speaker_count_skips_the_merge_pass() {
        let (emb, secs) = split_person();
        let labels = cluster(
            &emb,
            &secs,
            &ClusterConfig {
                num_speakers: Some(2),
                merge_similarity: 0.1,
                small_cluster_secs: 20.0,
                ..Default::default()
            },
        );
        assert_eq!(labels.iter().copied().max(), Some(1));
    }

    #[test]
    fn a_second_cluster_matching_a_taken_voice_is_absorbed() {
        let known = vec![KnownVoice {
            id: "dima".into(),
            name: "Дима".into(),
            emails: Vec::new(),
            centroid: vec![1.0, 0.0],
        }];
        // Cluster 0 is Dima; cluster 1 also matches the profile but loses the one-to-one match.
        let cents = vec![vec![0.99, 0.05], vec![0.8, 0.6], vec![0.0, 1.0]];
        assert_eq!(same_voice_merge(&cents, &known, 0.7, 0.25), Some((1, 0)));
        // Clearly different from the holder: not merged.
        let far = vec![vec![1.0, 0.0], vec![0.72, 0.69]];
        assert_eq!(same_voice_merge(&far, &known, 0.7, 0.9), None);
        assert_eq!(same_voice_merge(&cents[2..], &known, 0.7, 0.25), None);
    }

    #[test]
    fn empty_and_single_inputs() {
        assert!(cluster(&[], &[], &ClusterConfig::default()).is_empty());
        assert_eq!(
            cluster(&[vec![1.0, 0.0]], &[1.0], &ClusterConfig::default()),
            vec![0]
        );
    }

    #[test]
    fn voice_matching_is_one_to_one_and_thresholded() {
        let known = vec![
            KnownVoice {
                id: "a".into(),
                name: "Anna".into(),
                emails: Vec::new(),
                centroid: vec![1.0, 0.0],
            },
            KnownVoice {
                id: "b".into(),
                name: "Boris".into(),
                emails: Vec::new(),
                centroid: vec![0.0, 1.0],
            },
        ];
        let cents = vec![vec![0.99, 0.1], vec![0.9, 0.2], vec![-1.0, 0.0]];
        let m = match_voices(&cents, &known, 0.7);
        assert_eq!(
            m,
            vec![Some(0), None, None],
            "Anna taken by the closer cluster"
        );
        let m = match_voices(&[vec![0.1, 0.99]], &known, 0.7);
        assert_eq!(m, vec![Some(1)]);
        assert_eq!(
            match_voices(&[vec![1.0, 0.0, 0.0]], &known, 0.0),
            vec![None]
        );
    }

    #[test]
    fn unlabelled_follow_nearest_neighbour_in_time() {
        let spans = [(0, 1000), (1100, 1400), (5000, 6000), (6100, 6300)];
        let labels = [Some(0), None, Some(1), None];
        assert_eq!(assign_unlabelled(&spans, &labels), vec![0, 0, 1, 1]);
        assert_eq!(assign_unlabelled(&spans[..2], &[None, None]), vec![0, 0]);
    }

    #[test]
    fn bytes_round_trip_and_centroid() {
        let v = vec![0.5, -1.25, 3.0];
        assert_eq!(from_bytes(&to_bytes(&v)), v);
        let c = centroid([[1.0f32, 0.0].as_slice(), [0.0, 1.0].as_slice()]);
        assert!((c[0] - c[1]).abs() < 1e-6 && (cosine(&c, &c) - 1.0).abs() < 1e-6);
        assert!(centroid(std::iter::empty::<&[f32]>()).is_empty());
    }
}
