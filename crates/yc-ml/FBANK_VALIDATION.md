# CAMPPlus FBank validation

`KaldiFbank` uses 16 kHz audio, 25 ms frames, 10 ms shifts, 80 triangular mel bins, Povey
windowing, 0.97 pre-emphasis, per-frame DC removal, no dither, snipped edges, log power and
per-utterance feature-wise mean normalization. A one-second signal produces 98 frames, matching
Kaldi's snipped-edge framing rule. Unit tests check that count, finite feature values, and zero
temporal mean per bin.

I could not make a numerical comparison with `torchaudio.compliance.kaldi.fbank` in the current
build environment: `import torchaudio` fails with `ModuleNotFoundError`. The numeric reference
comparison remains unverified; the tests currently cover framing and normalization only.
