//! System audio on Linux: the monitor source of the default sink, read through `parec`.
//!
//! `parec` ships with PulseAudio and with `pipewire-pulse`, so this covers both sound servers
//! without linking `libpulse` at build time (no `-dev` package needed, nothing to dlopen). The
//! child's stdout is raw interleaved f32le stereo at 48 kHz; a reader thread converts it to mono and
//! feeds the ring. Dropping the guard kills `parec` and joins the thread.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;

use yc_core::{Error, Result};

use crate::supervisor::{Backend, OpenInfo, Sink};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;

pub(crate) struct MonitorBackend;

pub(crate) struct MonitorStream {
    child: Child,
    reader: Option<JoinHandle<()>>,
}

impl Drop for MonitorStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

fn default_sink() -> Option<String> {
    let out = Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .ok()?;
    let name = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !name.is_empty()).then_some(name)
}

impl Backend for MonitorBackend {
    type Guard = MonitorStream;

    fn open(&mut self, sink: Sink) -> Result<(MonitorStream, OpenInfo)> {
        let sink_name = default_sink().ok_or_else(|| {
            Error::Device(
                "cannot find the default PulseAudio/PipeWire sink (is `pactl` installed?)".into(),
            )
        })?;
        let monitor = format!("{sink_name}.monitor");
        let mut child = Command::new("parec")
            .args([
                "--raw",
                "--format=float32le",
                "--channels=2",
                "--rate=48000",
                "--latency-msec=40",
                "--device",
            ])
            .arg(&monitor)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                Error::Device(format!(
                    "cannot start `parec` (install pulseaudio-utils): {e}"
                ))
            })?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Device("parec has no stdout".into()))?;

        let reader = std::thread::Builder::new()
            .name("yc-parec-reader".into())
            .spawn(move || {
                let mut sink = sink;
                let mut raw = [0u8; 4096 * 4];
                let mut carry = 0usize;
                let mut floats = [0f32; 4096];
                loop {
                    match stdout.read(&mut raw[carry..]) {
                        Ok(0) | Err(_) => {
                            sink.mark_failed();
                            return;
                        }
                        Ok(n) => {
                            let total = carry + n;
                            let whole = total - total % (4 * CHANNELS);
                            let count = whole / 4;
                            for (o, b) in floats.iter_mut().zip(raw[..whole].chunks_exact(4)) {
                                *o = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                            }
                            sink.push_interleaved(&floats[..count], CHANNELS, |s| s);
                            raw.copy_within(whole..total, 0);
                            carry = total - whole;
                        }
                    }
                }
            })
            .map_err(Error::Io)?;

        Ok((
            MonitorStream {
                child,
                reader: Some(reader),
            },
            OpenInfo {
                device: format!("System audio ({sink_name})"),
                sample_rate: RATE,
            },
        ))
    }

    fn target(&self) -> Option<String> {
        default_sink()
    }
}
