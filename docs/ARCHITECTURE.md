# YadaCall — architecture

YadaCall records calls as two tracks ("me" = microphone, "them" = system audio), transcribes them
**on-device while the call is still going**, builds summaries / tasks, and answers questions across
all meetings. Everything runs locally by default; cloud LLMs are opt-in and only ever receive text.

Design goals, in priority order:

1. **Never lose a recording.** Raw audio hits disk in small appends; a crash loses ≤ 2 s.
2. **Instant results.** Live transcription during the call means the transcript is ready seconds
   after "Stop", not minutes. The post-call pass only finalises and refines.
3. **Light.** No ffmpeg, no Python, no C++ toolchain in the build. Installer stays small; models and
   the LLM runtime are downloaded on demand, after the app has shown whether they will run well.
4. **Clean boundaries.** Domain logic is pure Rust with no I/O; adapters (audio, ONNX, SQLite, HTTP)
   sit behind small traits; the Tauri layer is a thin shell.
5. **Cross-platform.** macOS 14.2+ (Apple Silicon) first, Windows 10/11 and Linux from the same code.

## Workspace

```
crates/
  yc-core      domain types, port traits, errors, events. No heavy deps. Everything depends on it.
  yc-text      pure transcript logic: track alignment, turn building, echo-duplicate filter,
               hallucination filters, Markdown/JSON rendering. 100% unit-testable.
  yc-audio     capture (mic via cpal; system audio via Core Audio process tap / WASAPI loopback /
               PulseAudio monitor), resampling to 16 kHz mono, crash-safe writer, level meters,
               silence watchdog.
  yc-ml        ONNX Runtime (`ort`) adapters: Silero VAD, ASR engines (GigaAM v3, Parakeet TDT v3),
               speaker embeddings, text embeddings. Featurisers and decoders live here.
  yc-store     SQLite index (rusqlite, bundled): meetings, segments, FTS5, sqlite-vec vectors,
               jobs, settings. On-disk meeting folders are the source of truth; the index is rebuildable.
  yc-hub       model catalog, hardware probe, performance predictor, resumable downloader with
               mirrors + sha256, runtime packs (llama.cpp server builds).
  yc-llm       chat providers (OpenAI-compatible: local llama-server / LM Studio / OpenRouter;
               Ollama native API for the local daemon and Ollama Cloud; Anthropic), llama-server supervisor, prompts, summary JSON schema, RAG answering.
  yc-engine    orchestration: recording sessions, live transcription, job pipeline state machine,
               event bus. The only crate that wires adapters together.
apps/
  desktop      Tauri 2 shell (src-tauri) + Svelte 5 UI (ui/). Commands are one-liners into yc-engine.
  cli          `yadacall` CLI: transcribe a file, run benchmarks, inspect hardware, manage models,
               serve MCP over stdio. Same engine, no UI — used heavily in tests and CI.
catalog/
  models.json  model + runtime catalog (embedded at build time, refreshable from GitHub).
```

Dependency rule: `core ← text ← {audio, ml, store, hub, llm} ← engine ← apps`. Adapters never
depend on each other; only `yc-engine` composes them.

## Data flow

```
 mic (cpal) ─┐                                       ┌─> recording/<id>/mic.pcm   (raw s16le 16k mono)
             ├─ resample 16k mono ─ level meter ─────┤
 system tap ─┘   (per track)        silence watch    └─> recording/<id>/them.pcm
                       │
                       ▼
             streaming Silero VAD (per track)
                       │ closed speech region (≤ 20 s)
                       ▼
             ASR worker pool (GigaAM / Parakeet via ort) ──> live.jsonl (append-only)
                       │                                 └─> EngineEvent::LiveSegment → UI
                       ▼   on Stop
     finalize: flush VAD, wait for queue → yc-text merge (align, turns, echo filter, hallucination
     filter) → transcript.json + transcript.md → index (FTS5) → [diarize "them" if group call]
     → chunk + embed (sqlite-vec) → summary (LLM, optional) → notify "Transcript ready"
```

Every post-call stage is a **job** row in SQLite (`jobs` table: kind, meeting_id, state, attempts,
error, timings). Jobs are idempotent and resume after a crash or restart. Audio is deleted by the
retention policy only after the transcript stage succeeded.

## On-disk layout

```
<library>/                           default ~/YadaCall (user-configurable)
  2026-10-12 14-30 Синк по онбордингу/
    meeting.json        id, title, started_at, duration, track clock offsets, bookmarks, participants
    transcript.json     segments: start, end, speaker, track, text (+ words when available)
    transcript.md       human/Claude-readable dialogue with timestamps and YAML front matter
    summary.json        structured summary (about, decisions, tasks, open questions), if made
    audio/mic.flac, audio/them.flac   kept per retention policy (none / 7 / 30 days / forever)
<app data>/
  index.sqlite          rebuildable index + jobs + settings + vectors
  models/<model-id>/…   downloaded models, verified by sha256
  runtimes/<pack-id>/…  downloaded llama.cpp server build for this platform/GPU
  recording/<id>/…      in-progress raw tracks + live.jsonl (moved into the library on finalise)
```

No colons in names (Windows). The library can be synced/backed up as plain files.

## Models and runtimes

- **ASR**: GigaAM v3 e2e-RNNT (Russian, punctuation + casing, best Russian WER) and Parakeet TDT
  0.6B v3 (25 European languages incl. Russian/English) via ONNX Runtime with CoreML / DirectML /
  CUDA execution providers where available, CPU otherwise. Featurisers are ONNX graphs too, so the
  Rust side only runs sessions and a greedy transducer decode loop.
- **VAD**: Silero VAD (ONNX, ~2 MB, bundled).
- **Speaker embeddings**: small ONNX model (~28 MB) for group-call diarisation and recognising known
  voices across meetings (voice centroids).
- **Text embeddings**: small multilingual ONNX encoder for semantic search; hybrid with FTS5 BM25 via
  reciprocal-rank fusion.
- **LLM**: Gemma 4 GGUF served by an official llama.cpp `llama-server` build that is downloaded as a
  *runtime pack* for the current platform/GPU (Metal / Vulkan / CUDA / CPU) and talked to over the
  OpenAI-compatible API on a random localhost port. The same client code drives LM Studio, OpenRouter
  and any OpenAI-compatible endpoint; Ollama (the local daemon, whose `:cloud` models run under the user's
  subscription, and Ollama Cloud with a key) uses its native `/api/chat` with `format` = JSON schema;
  Anthropic has its own small client. Summaries can be switched off entirely (Ask then lists passages).

Why a sidecar instead of linking llama.cpp: no C++ build, new model architectures arrive by updating
a download (not the app), GPU backends match the official builds, and a crash in inference cannot take
the recorder down.

## "Will it run?" before downloading

`yc-hub` probes the machine (CPU, cores, ISA, RAM, Apple chip / GPU + VRAM, NPU, disk) and runs two
sub-second micro-benchmarks (memory bandwidth, f32 GEMM). Each catalog entry carries cost constants
(weights bytes, GFLOP per audio second, KV-cache geometry). The predictor turns these into a verdict
per model — `great / ok / slow / won't run` — with a concrete estimate ("≈ 40× realtime",
"≈ 25 tok/s, uses 6.1 GB"). After a model is downloaded the app runs a real 10-second benchmark and
stores the measured factor, which recalibrates predictions for the other models on that backend.

Downloads are resumable (HTTP Range), verified (sha256), atomic (temp file + rename), with mirror
fallback (Hugging Face → hf-mirror → GitHub Releases), disk-space pre-check, pause/cancel, and
progress streamed to the UI over a Tauri channel.

## Privacy

No telemetry, no accounts, no install registration. Network is used only for model/runtime downloads,
update checks, and — when the user enables it — sending transcript text to a chosen cloud LLM. The UI
always states what left the device ("6 text excerpts sent to Claude, no audio").
