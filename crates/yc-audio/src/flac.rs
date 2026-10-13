//! Pure-Rust FLAC encoding of a finished track (for retention: FLAC is ~½ the size of raw PCM).

use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use flacenc::bitsink::ByteSink;
use flacenc::component::BitRepr;
use flacenc::config::Encoder;
use flacenc::error::{SourceError, Verify};
use flacenc::source::{Fill, Source};
use yc_core::{Error, Result, SAMPLE_RATE};

/// Streams an s16le mono `.pcm` file into the encoder without expanding it to i32 in memory.
struct PcmFileSource {
    reader: BufReader<File>,
    bytes: Vec<u8>,
    total: usize,
}

impl Source for PcmFileSource {
    fn channels(&self) -> usize {
        1
    }
    fn bits_per_sample(&self) -> usize {
        16
    }
    fn sample_rate(&self) -> usize {
        SAMPLE_RATE as usize
    }
    fn read_samples<F: Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> std::result::Result<usize, SourceError> {
        self.bytes.resize(block_size * 2, 0);
        let mut got = 0;
        while got < self.bytes.len() {
            let n = self
                .reader
                .read(&mut self.bytes[got..])
                .map_err(SourceError::from_io_error)?;
            if n == 0 {
                break;
            }
            got += n;
        }
        let whole = got & !1;
        dest.fill_le_bytes(&self.bytes[..whole], 2)?;
        Ok(whole / 2)
    }
    fn len_hint(&self) -> Option<usize> {
        Some(self.total)
    }
}

/// FNV-1a over the little-endian bytes of a stream of 16-bit samples.
#[derive(Clone, Copy)]
struct SampleHash(u64);

impl SampleHash {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn push(&mut self, s: i16) {
        for b in s.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Count and checksum of the whole samples of a raw s16le file.
fn hash_pcm(pcm: &Path) -> Result<(u64, u64)> {
    let mut reader = BufReader::new(File::open(pcm)?);
    let (mut buf, mut hash, mut n) = (vec![0u8; 1 << 16], SampleHash::new(), 0u64);
    let mut carry = 0usize;
    loop {
        let got = reader.read(&mut buf[carry..])?;
        if got == 0 {
            break;
        }
        let total = carry + got;
        let whole = total & !1;
        for c in buf[..whole].chunks_exact(2) {
            hash.push(i16::from_le_bytes([c[0], c[1]]));
            n += 1;
        }
        buf.copy_within(whole..total, 0);
        carry = total - whole;
    }
    Ok((n, hash.0))
}

/// Decodes `flac` with the same decoder the app plays and re-transcribes with (symphonia) and
/// returns (samples, checksum) of what came out.
fn hash_flac(flac: &Path) -> Result<(u64, u64)> {
    let (mut n, mut hash) = (0u64, SampleHash::new());
    crate::decode::decode_to_16k_mono(flac, &mut |block| {
        for &f in block {
            hash.push((f * 32768.0).round().clamp(-32768.0, 32767.0) as i16);
        }
        n += block.len() as u64;
        Ok(())
    })?;
    Ok((n, hash.0))
}

/// Encodes `<pcm>` (s16le mono 16 kHz) to a FLAC file at `out` and returns its size in bytes.
///
/// The result is written to a temp file, **decoded again** and compared (sample count and
/// checksum) with the source, and only then renamed to `out`. `Ok` therefore means the FLAC holds
/// exactly the audio of the `.pcm` and can be read back by the app's decoder — callers may delete
/// the `.pcm` only after `Ok`. On any failure `out` is untouched and the temp file is removed.
pub fn encode_flac(pcm: &Path, out: &Path) -> Result<u64> {
    let len = fs::metadata(pcm)?.len();
    let source = PcmFileSource {
        reader: BufReader::new(File::open(pcm)?),
        bytes: Vec::new(),
        total: (len / 2) as usize,
    };
    let config = Encoder::default()
        .into_verified()
        .map_err(|e| Error::Invalid(format!("flac config: {e:?}")))?;
    let block = config.block_size;
    let mut stream = flacenc::encode_with_fixed_block_size(&config, source, block)
        .map_err(|e| Error::Storage(format!("flac encode: {e:?}")))?;
    // flacenc folds the short last block into `min_blocksize`. The FLAC spec says the minimum
    // excludes the last block (so a fixed-blocksize stream has min == max), and strict decoders —
    // Core Audio, symphonia — reject a file whose STREAMINFO claims a variable blocksize.
    let total = (len / 2) as usize;
    let fixed = block.min(total.max(1));
    stream
        .stream_info_mut()
        .set_block_sizes(fixed, fixed)
        .map_err(|e| Error::Storage(format!("flac streaminfo: {e:?}")))?;
    let mut sink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| Error::Storage(format!("flac write: {e:?}")))?;
    let tmp = out.with_extension("flac.tmp");
    fs::write(&tmp, sink.as_slice())?;

    let verified = (|| -> Result<()> {
        if total == 0 {
            return Ok(()); // nothing to lose
        }
        let want = hash_pcm(pcm)?;
        let got = hash_flac(&tmp)?;
        if got != want {
            tracing::error!(?want, ?got, "FLAC read-back differs from the source");
            return Err(Error::Integrity(out.to_path_buf()));
        }
        Ok(())
    })();
    if let Err(e) = verified {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, out)?;
    Ok(sink.as_slice().len() as u64)
}

/// What [`repair_flac`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repair {
    /// STREAMINFO was already correct; the file was not touched.
    AlreadyValid,
    /// The block sizes in STREAMINFO were corrected in place.
    Repaired,
}

const STREAMINFO_OFFSET: u64 = 8; // "fLaC" + 4-byte metadata block header

/// Fixes FLAC files written by the first version of [`encode_flac`], whose STREAMINFO announced a
/// variable block size (`min_blocksize` = size of the short last block) that Core Audio and
/// symphonia refuse to read. Only the 4 block-size bytes are rewritten, in place; the audio frames
/// are untouched. The result is decoded to check that its sample count matches STREAMINFO, and the
/// original bytes are restored if it does not. Files from other encoders pass through unchanged.
pub fn repair_flac(path: &Path) -> Result<Repair> {
    let mut f = fs::OpenOptions::new().read(true).write(true).open(path)?;
    let mut head = [0u8; 42];
    f.read_exact(&mut head)
        .map_err(|_| Error::Invalid(format!("{}: too short for FLAC", path.display())))?;
    if &head[..4] != b"fLaC" || head[4] & 0x7f != 0 {
        return Err(Error::Invalid(format!(
            "{}: not a FLAC file",
            path.display()
        )));
    }
    let si = &head[8..42];
    let min = u16::from_be_bytes([si[0], si[1]]);
    let max = u16::from_be_bytes([si[2], si[3]]);
    let packed = u64::from_be_bytes([
        si[10], si[11], si[12], si[13], si[14], si[15], si[16], si[17],
    ]);
    let total = packed & ((1 << 36) - 1);
    if min == max || max == 0 {
        return Ok(Repair::AlreadyValid);
    }
    // Multi-frame stream: the fixed size is the largest block. A single short frame: its length.
    let fixed = if total > u64::from(max) || total == 0 {
        max
    } else {
        total as u16
    };

    let original = [head[8], head[9], head[10], head[11]];
    let patched = [fixed.to_be_bytes(), fixed.to_be_bytes()].concat();
    let write_at = |f: &mut File, bytes: &[u8]| -> std::io::Result<()> {
        f.seek(SeekFrom::Start(STREAMINFO_OFFSET))?;
        f.write_all(bytes)?;
        f.sync_data()
    };
    write_at(&mut f, &patched)?;
    let decoded = hash_flac(path).map(|(n, _)| n);
    if decoded.as_ref().ok() != Some(&total) {
        write_at(&mut f, &original)?;
        return Err(Error::Integrity(path.to_path_buf()));
    }
    Ok(Repair::Repaired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flac_round_trip_is_lossless_and_smaller() {
        let dir = tempfile::tempdir().unwrap();
        // 3 s of two tones plus a deterministic noise burst, as s16.
        let mut seed = 12345u32;
        let samples: Vec<i16> = (0..48_000usize)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = ((seed >> 16) as f32 / 65536.0 - 0.5) * 0.02;
                let v = 0.4 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.2 * (2.0 * std::f32::consts::PI * 2500.0 * t).sin()
                    + noise;
                (v * 32767.0) as i16
            })
            .collect();
        let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let pcm_path = dir.path().join("t.pcm");
        let flac_path = dir.path().join("t.flac");
        fs::write(&pcm_path, &pcm).unwrap();

        let size = encode_flac(&pcm_path, &flac_path).unwrap();
        assert!(size < pcm.len() as u64 * 3 / 4, "{size} vs {}", pcm.len());

        let mut reader = claxon::FlacReader::open(&flac_path).unwrap();
        let info = reader.streaminfo();
        assert_eq!(
            (info.sample_rate, info.channels, info.bits_per_sample),
            (16_000, 1, 16)
        );
        let decoded: Vec<i16> = reader.samples().map(|s| s.unwrap() as i16).collect();
        assert_eq!(decoded, samples, "bit-exact round trip");
    }

    #[test]
    fn handles_non_block_multiple_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        let pcm_path = dir.path().join("t.pcm");
        let samples: Vec<i16> = (0..5000).map(|i| ((i * 7) % 2000 - 1000) as i16).collect();
        fs::write(
            &pcm_path,
            samples
                .iter()
                .flat_map(|s| s.to_le_bytes())
                .collect::<Vec<u8>>(),
        )
        .unwrap();
        let out = dir.path().join("t.flac");
        encode_flac(&pcm_path, &out).unwrap();
        let decoded: Vec<i16> = claxon::FlacReader::open(&out)
            .unwrap()
            .samples()
            .map(|s| s.unwrap() as i16)
            .collect();
        assert_eq!(decoded, samples);
    }

    /// Speech-like test signal: tone sweep + noise, `n` samples.
    fn signal(n: usize) -> Vec<i16> {
        let mut seed = 99u32;
        (0..n)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = ((seed >> 16) as f32 / 65536.0 - 0.5) * 0.05;
                let f = 200.0 + 300.0 * (t * 0.7).sin();
                ((0.35 * (2.0 * std::f32::consts::PI * f * t).sin() + noise) * 32767.0) as i16
            })
            .collect()
    }

    fn encode_signal(dir: &Path, n: usize) -> (Vec<i16>, std::path::PathBuf) {
        let samples = signal(n);
        let pcm = dir.join("t.pcm");
        fs::write(
            &pcm,
            samples
                .iter()
                .flat_map(|s| s.to_le_bytes())
                .collect::<Vec<u8>>(),
        )
        .unwrap();
        let out = dir.join("t.flac");
        encode_flac(&pcm, &out).unwrap();
        (samples, out)
    }

    fn decode_symphonia(path: &Path) -> Vec<f32> {
        let mut v = Vec::new();
        crate::decode::decode_to_16k_mono(path, &mut |b| {
            v.extend_from_slice(b);
            Ok(())
        })
        .unwrap();
        v
    }

    #[test]
    fn symphonia_decodes_long_multi_block_file_with_short_tail() {
        let dir = tempfile::tempdir().unwrap();
        let n = 4096 * 40 + 1973;
        let (samples, flac) = encode_signal(dir.path(), n);
        let got = decode_symphonia(&flac);
        assert_eq!(got.len(), n, "symphonia sample count");
        let max_err = got
            .iter()
            .zip(&samples)
            .map(|(a, b)| (a - f32::from(*b) / 32768.0).abs())
            .fold(0.0, f32::max);
        assert!(max_err < 1e-4, "{max_err}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn core_audio_reads_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let n = 4096 * 40 + 1973;
        let (_, flac) = encode_signal(dir.path(), n);
        let wav = dir.path().join("t.wav");
        let out = std::process::Command::new("afconvert")
            .args(["-f", "WAVE", "-d", "LEI16"])
            .arg(&flac)
            .arg(&wav)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "afconvert: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let info = std::process::Command::new("afinfo")
            .arg(&flac)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&info.stdout);
        assert!(text.contains(&format!("{n} valid frames")), "{text}");
        assert!(fs::metadata(&wav).unwrap().len() >= 2 * n as u64);
    }

    /// What the first version of `encode_flac` wrote: flacenc's output with its STREAMINFO as is.
    fn encode_legacy(pcm: &Path, out: &Path) {
        let len = fs::metadata(pcm).unwrap().len();
        let source = PcmFileSource {
            reader: BufReader::new(File::open(pcm).unwrap()),
            bytes: Vec::new(),
            total: (len / 2) as usize,
        };
        let config = Encoder::default().into_verified().unwrap();
        let stream =
            flacenc::encode_with_fixed_block_size(&config, source, config.block_size).unwrap();
        let mut sink = ByteSink::new();
        stream.write(&mut sink).unwrap();
        fs::write(out, sink.as_slice()).unwrap();
    }

    fn streaminfo_blocks(path: &Path) -> (u16, u16) {
        let d = fs::read(path).unwrap();
        (
            u16::from_be_bytes([d[8], d[9]]),
            u16::from_be_bytes([d[10], d[11]]),
        )
    }

    #[test]
    fn new_files_announce_a_fixed_blocksize() {
        let dir = tempfile::tempdir().unwrap();
        let (_, flac) = encode_signal(dir.path(), 4096 * 5 + 1973);
        assert_eq!(streaminfo_blocks(&flac), (4096, 4096));
        assert_eq!(repair_flac(&flac).unwrap(), Repair::AlreadyValid);
    }

    #[test]
    fn repair_makes_old_encoder_files_readable_in_place() {
        for n in [4096 * 40 + 1973, 1500] {
            let dir = tempfile::tempdir().unwrap();
            let samples = signal(n);
            let pcm = dir.path().join("t.pcm");
            fs::write(
                &pcm,
                samples
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect::<Vec<u8>>(),
            )
            .unwrap();
            let flac = dir.path().join("t.flac");
            encode_legacy(&pcm, &flac);
            let audio_before = fs::read(&flac).unwrap()[42..].to_vec();
            if n > 4096 {
                assert_eq!(
                    streaminfo_blocks(&flac).0,
                    1973,
                    "legacy file has the bad minimum"
                );
                assert!(
                    hash_flac(&flac).is_err(),
                    "symphonia rejects the legacy file"
                );
            }

            assert_eq!(repair_flac(&flac).unwrap(), Repair::Repaired);
            assert_eq!(repair_flac(&flac).unwrap(), Repair::AlreadyValid);
            let (min, max) = streaminfo_blocks(&flac);
            assert_eq!(min, max);
            assert_eq!(
                fs::read(&flac).unwrap()[42..],
                audio_before[..],
                "frames untouched"
            );
            assert_eq!(hash_flac(&flac).unwrap(), hash_pcm(&pcm).unwrap());
            let mut claxon = claxon::FlacReader::open(&flac).unwrap();
            assert_eq!(claxon.samples().count(), n);
        }
    }

    #[test]
    fn repair_rejects_non_flac() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.flac");
        fs::write(&p, vec![0u8; 100]).unwrap();
        assert!(repair_flac(&p).is_err());
    }

    /// Manual check against a real library file: `YC_REAL_FLAC=/path/to/them.flac cargo test -- --ignored`.
    /// Works on a copy; the original is never modified.
    #[test]
    #[ignore = "needs YC_REAL_FLAC"]
    fn repairs_a_real_library_file() {
        let src = std::env::var("YC_REAL_FLAC").expect("YC_REAL_FLAC");
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("real.flac");
        fs::copy(&src, &copy).unwrap();
        let before = streaminfo_blocks(&copy);
        let outcome = repair_flac(&copy).unwrap();
        let (n, _) = hash_flac(&copy).unwrap();
        eprintln!(
            "{src}: {before:?} -> {:?}, {outcome:?}, {n} samples",
            streaminfo_blocks(&copy)
        );
        #[cfg(target_os = "macos")]
        {
            let wav = dir.path().join("real.wav");
            let out = std::process::Command::new("afconvert")
                .args(["-f", "WAVE", "-d", "LEI16"])
                .arg(&copy)
                .arg(&wav)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
