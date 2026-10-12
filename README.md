# YadaCall

Записывает созвоны в две дорожки — «я» и «собеседник», расшифровывает их **прямо на компьютере и прямо во время звонка**, собирает итоги и задачи и отвечает на вопросы по всем встречам. Без облака, аккаунтов и телеметрии. macOS, Windows, Linux.

Records calls as two tracks (you / the other side), transcribes them **on-device while the call is happening**, builds summaries and action items, and answers questions across all your meetings. No cloud, no accounts, no telemetry.

- Russian-first speech recognition (GigaAM v3), 25 languages via Parakeet v3
- Local summaries with Gemma 4 (llama.cpp), or your own Claude / OpenAI-compatible / Ollama endpoint
- Shows whether each model will run well on *your* machine before you download it
- Plain files: every meeting is a folder with `transcript.md` + `transcript.json`

Status: early development. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

License: MIT.
