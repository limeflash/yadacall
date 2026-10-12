mod helpers;

use std::{path::PathBuf, time::Instant};

use yc_core::ports::{EmbedKind, SpeakerEmbedder, TextEmbedder};
use yc_ml::{
    AsrEngine, AsrKind, Backend, CampPlusEmbedder, OnnxTextEmbedder, Pooling, SileroVad,
    TextEmbeddingConfig,
};

fn models_dir() -> PathBuf {
    std::env::var_os("YC_MODELS_DIR")
        .map(PathBuf::from)
        .expect("set YC_MODELS_DIR to the shared developer model cache")
}

#[test]
fn silero_streaming_vad_finds_voiced_regions_between_silence_gaps() {
    use yc_core::ports::VoiceActivityDetector;

    let mut pcm = vec![0.0; 11_200];
    helpers::append_with_silence(
        &mut pcm,
        &helpers::render_voiced_signal(1.7, 142.0, 0),
        12_800,
    );
    helpers::append_with_silence(
        &mut pcm,
        &helpers::render_voiced_signal(1.7, 142.0, 2),
        11_200,
    );

    let mut vad =
        SileroVad::load(Backend::Cpu, &std::env::temp_dir()).expect("load bundled Silero VAD");
    let mut regions = Vec::new();
    for chunk in pcm.chunks(257) {
        regions.extend(vad.push(chunk).expect("process VAD audio"));
    }
    regions.extend(vad.flush());

    assert_eq!(regions.len(), 2, "regions: {regions:?}");
    assert!(
        regions[0].start < 30_000 && regions[0].end < 55_000,
        "first region is outside the first speech interval: {regions:?}"
    );
    assert!(regions[1].start > 35_000 && regions[1].end > 70_000);
}

#[test]
#[ignore = "requires the downloaded CAMPPlus model; uses synthetic voiced signals because macOS TTS is unavailable in the sandbox"]
fn campplus_embedding_is_stable_for_one_voice_and_separates_another() {
    let root = models_dir();
    let model_dir = root.join("k2-fsa__sherpa-onnx__speaker-recongition-models");
    let mut embedder = CampPlusEmbedder::load(&model_dir, Backend::Cpu).expect("load CAMPPlus");
    let same_voice_first = helpers::render_voiced_signal(2.0, 132.0, 0);
    let same_voice_second = helpers::render_voiced_signal(2.0, 132.0, 3);
    let different_voice = helpers::render_voiced_signal(2.0, 205.0, 1);
    let first = embedder
        .embed(&same_voice_first)
        .expect("embed first voice");
    let second = embedder
        .embed(&same_voice_second)
        .expect("embed same voice, different text");
    let other = embedder.embed(&different_voice).expect("embed other voice");
    let same_similarity = helpers::cosine(&first, &second);
    let different_similarity = helpers::cosine(&first, &other);
    eprintln!("CAMPPlus cosine: same={same_similarity:.4}, different={different_similarity:.4}");
    assert!(same_similarity > different_similarity);
    assert!((first.iter().map(|value| value * value).sum::<f32>().sqrt() - 1.0).abs() < 1e-4);
}

#[test]
#[ignore = "requires downloaded GigaAM v3 RNNT weights"]
fn gigaam_rnnt_runs_on_cpu() {
    let root = models_dir();
    let model_dir = root.join("istupakov__gigaam-v3-onnx");
    let mut recognizer =
        AsrEngine::load(AsrKind::GigaAm, &model_dir, Backend::Cpu).expect("load GigaAM");
    let pcm = helpers::render_voiced_signal(1.8, 142.0, 0);
    let output = recognizer
        .recognize(&pcm, "ru")
        .expect("recognize voiced audio");
    eprintln!("GigaAM CPU output: {output:?}");
}

#[test]
#[ignore = "requires downloaded Parakeet TDT v3 int8 weights"]
fn parakeet_tdt_runs_on_cpu() {
    let root = models_dir();
    let model_dir = root.join("istupakov__parakeet-tdt-0.6b-v3-onnx");
    let mut recognizer =
        AsrEngine::load(AsrKind::Parakeet, &model_dir, Backend::Cpu).expect("load Parakeet");
    let pcm = helpers::render_voiced_signal(1.8, 142.0, 0);
    let output = recognizer
        .recognize(&pcm, "ru")
        .expect("recognize voiced audio");
    eprintln!("Parakeet CPU output: {output:?}");
}

#[test]
#[ignore = "requires multilingual-e5-small qint8 and O4 ONNX weights"]
fn e5_qint8_and_o4_have_the_same_russian_retrieval_ranking() {
    let onnx_dir = models_dir().join("intfloat__multilingual-e5-small/onnx");
    let tokenizer_path = onnx_dir.join("tokenizer.json");
    let queries = ["Как изменить адрес доставки?", "Что обсуждали на встрече?"];
    let passages = [
        "Измените адрес доставки в настройках аккаунта до отправки заказа.",
        "Встреча команды прошла во вторник и закончилась обсуждением бюджета.",
        "Для возврата товара заполните форму на сайте магазина.",
    ];
    let mut rankings = Vec::new();
    for filename in ["model_qint8_avx512_vnni.onnx", "model_O4.onnx"] {
        let config = TextEmbeddingConfig {
            onnx_path: onnx_dir.join(filename),
            tokenizer_path: tokenizer_path.clone(),
            query_prefix: "query: ".to_owned(),
            passage_prefix: "passage: ".to_owned(),
            pooling: Pooling::Mean,
            max_tokens: 512,
            dims: 384,
        };
        let mut embedder =
            OnnxTextEmbedder::load(config, Backend::Cpu, &onnx_dir).expect("load E5 model");
        let query = embedder
            .embed(&[queries[0]], EmbedKind::Query)
            .expect("embed query");
        let started = Instant::now();
        let passage_vectors = embedder
            .embed(&passages, EmbedKind::Passage)
            .expect("embed passages");
        let elapsed = started.elapsed().as_secs_f64();
        let scores = passage_vectors
            .iter()
            .map(|vector| helpers::cosine(&query[0], vector))
            .collect::<Vec<_>>();
        let ranking = (0..scores.len())
            .max_by(|left, right| scores[*left].total_cmp(&scores[*right]))
            .expect("passage ranking");
        eprintln!(
            "E5 {filename}: passages/s={:.2}, scores={scores:?}",
            passages.len() as f64 / elapsed
        );
        rankings.push(ranking);
    }
    assert_eq!(rankings, vec![0, 0]);
}
