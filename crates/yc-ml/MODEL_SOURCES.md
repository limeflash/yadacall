# Model sources and licenses

The inference crate loads large recognition, speaker and embedding weights from local model
directories. These weights are downloaded separately and are not bundled with the crate.

| Component | Source | License / attribution |
| --- | --- | --- |
| Silero VAD v5.1.2 (`src/silero_vad/data/silero_vad.onnx`) | [snakers4/silero-vad, v5.1.2](https://github.com/snakers4/silero-vad/tree/v5.1.2/src/silero_vad/data) | MIT; copy at `src/silero_vad/data/LICENSE.txt` |
| GigaAM v3 RNNT / CTC models | [istupakov/gigaam-v3-onnx](https://huggingface.co/istupakov/gigaam-v3-onnx) | MIT |
| GigaAM preprocessor (`src/asr/preprocessors/data/gigaam_v3.onnx`) | [onnx-asr](https://github.com/istupakov/onnx-asr), `onnx_asr-0.12.0` package | MIT; copy at `src/asr/preprocessors/data/ONNX_ASR_LICENSE.txt` |
| Parakeet TDT 0.6B v3 and `nemo128.onnx` preprocessor | [istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) | CC BY 4.0; preserve attribution to NVIDIA NeMo and the model authors when redistributing |
| 3D-Speaker CAMPPlus | [sherpa-onnx speaker-recognition-models release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models) | Apache 2.0; 3D-Speaker authors |
| multilingual-e5-small | [intfloat/multilingual-e5-small](https://huggingface.co/intfloat/multilingual-e5-small) | MIT; Microsoft Research Asia |

The two embedded ONNX graphs are upstream artifacts: `gigaam_v3.onnx` is extracted unchanged
from the MIT-licensed `onnx-asr` 0.12.0 Python package, and `silero_vad.onnx` is downloaded
unchanged from the Silero v5.1.2 data directory. The Parakeet preprocessing graph is loaded
from the same local Hugging Face repository directory as its encoder, as required by the model
package layout.

Model cache files live outside the repository under `YC_MODELS_DIR` (or the developer model
cache). Check each upstream repository's current terms before redistributing downloaded weights.
