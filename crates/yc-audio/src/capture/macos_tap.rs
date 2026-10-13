//! System audio on macOS 14.2+ through a Core Audio process tap.
//!
//! # Why `cidre`
//!
//! The alternative, `objc2-core-audio`, is a raw `-sys`-style binding: every property read is an
//! `AudioObjectGetPropertyData` call with hand-built addresses and out-pointers, `CATapDescription`
//! has to be driven through `msg_send!`, and the aggregate-device dictionary is a `CFDictionary` built
//! by hand. `cidre` already wraps exactly these pieces — `TapDesc`, `Tap` (destroyed on drop),
//! `AggregateDevice` (destroyed on drop), `device_start`/`StartedDevice` (stopped on drop), property
//! getters for device name/uid/transport type — so the only `unsafe` left in this crate is the
//! `extern "C"` IOProc entry point and one pointer read of the callback's `AudioBufferList`.
//!
//! # TCC behaviour
//!
//! The first capture makes macOS show the "System Audio Recording Only" prompt for the *hosting app*
//! (the bundle that carries `NSAudioCaptureUsageDescription`; when run from a terminal it is the
//! terminal). While the prompt is open, or after it was denied, the tap keeps running but delivers
//! digital silence (exact zeros) and `AudioHardwareCreateProcessTap` does **not** fail. The recorder
//! therefore watches for "exact zeros since start" and raises
//! [`RecorderEvent::PermissionSuspected`](crate::RecorderEvent::PermissionSuspected); the same pattern
//! appears when nothing is playing, so the UI should phrase it as a hint ("no system audio yet —
//! check System Settings → Privacy & Security → Screen & System Audio Recording").

use ca::aggregate_device_keys as agg_keys;
use ca::sub_device_keys as sub_keys;
use cidre::{cat, cf, core_audio as ca, ns, os};
use yc_core::{Error, Result};

use crate::devices::macos_version_at_least;
use crate::supervisor::{Backend, OpenInfo, Sink};

pub(crate) struct TapBackend;

struct TapCtx {
    sink: Sink,
}

/// Live tap. Field order is drop order: stop IO and destroy the aggregate device first, then the
/// tap, then the callback context the (now stopped) IOProc pointed at.
pub(crate) struct TapStream {
    _started: ca::hardware::StartedDevice<ca::AggregateDevice>,
    _tap: ca::TapGuard,
    _ctx: Box<TapCtx>,
}

fn dev<E: std::fmt::Debug>(what: &str) -> impl FnOnce(E) -> Error + '_ {
    move |e| Error::Device(format!("{what}: {e:?}"))
}

extern "C" fn io_proc(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input_data: &cat::AudioBufList<1>,
    _input_time: &cat::AudioTimeStamp,
    _output_data: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    ctx: Option<&mut TapCtx>,
) -> os::Status {
    let Some(ctx) = ctx else {
        return Default::default();
    };
    if input_data.number_buffers == 0 {
        return Default::default();
    }
    let buf = &input_data.buffers[0];
    if buf.data.is_null() || buf.data_bytes_size == 0 {
        return Default::default();
    }
    let len = buf.data_bytes_size as usize / std::mem::size_of::<f32>();
    // SAFETY: Core Audio guarantees `data` points at `data_bytes_size` valid bytes for the duration
    // of this callback; the tap format is interleaved native-endian f32 (checked when opened).
    let samples = unsafe { std::slice::from_raw_parts(buf.data.cast::<f32>(), len) };
    ctx.sink
        .push_interleaved(samples, buf.number_channels.max(1) as usize, |s| s);
    Default::default()
}

impl Backend for TapBackend {
    type Guard = TapStream;

    fn open(&mut self, sink: Sink) -> Result<(TapStream, OpenInfo)> {
        if !macos_version_at_least(14, 2) {
            return Err(Error::Device(
                "system audio capture needs macOS 14.2 or newer".into(),
            ));
        }
        let out_dev = ca::System::default_output_device().map_err(dev("default output device"))?;
        let out_uid = out_dev.uid().map_err(dev("output device uid"))?;
        let out_name = out_dev
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|_| "output".into());

        // Exclude our own process so UI sounds of YadaCall never end up in "them".
        let own = match ca::Process::with_pid(std::process::id() as _) {
            Ok(p) if p.0 != ca::Obj::UNKNOWN => {
                ns::Array::from_slice_retained(&[ns::Number::with_u32(p.0.0)])
            }
            _ => ns::Array::new(),
        };
        let mut desc = ca::TapDesc::with_stereo_global_tap_excluding_processes(&own);
        desc.set_private(true);
        let tap = desc
            .create_process_tap()
            .map_err(dev("create process tap"))?;
        let asbd = tap.asbd().map_err(dev("tap format"))?;
        if !asbd.is_interleaved() {
            return Err(Error::Device(
                "process tap delivers non-interleaved audio (unsupported)".into(),
            ));
        }

        let sub_device =
            cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[out_uid.as_type_ref()]);
        let tap_uid = tap.uid().map_err(dev("tap uid"))?;
        let sub_tap =
            cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[tap_uid.as_type_ref()]);
        let dict = cf::DictionaryOf::with_keys_values(
            &[
                agg_keys::is_private(),
                agg_keys::is_stacked(),
                agg_keys::tap_auto_start(),
                agg_keys::name(),
                agg_keys::main_sub_device(),
                agg_keys::uid(),
                agg_keys::sub_device_list(),
                agg_keys::tap_list(),
            ],
            &[
                cf::Boolean::value_true().as_type_ref(),
                cf::Boolean::value_false(),
                cf::Boolean::value_true(),
                cf::str!(c"YadaCall system audio"),
                &out_uid,
                &cf::Uuid::new().to_cf_string(),
                &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
                &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
            ],
        );
        let agg = ca::AggregateDevice::with_desc(&dict).map_err(dev("create aggregate device"))?;

        let mut ctx = Box::new(TapCtx { sink });
        // The IOProc is clocked by the aggregate device, i.e. by the output device — not by what the
        // tap format claims. They disagree while a Bluetooth headset sits in HFP (tap says 48 kHz,
        // buffers arrive at 24 kHz). The ingest rate watch still arbitrates by measurement.
        let sample_rate = agg
            .nominal_sample_rate()
            .map(|r| r as u32)
            .ok()
            .filter(|r| *r > 0)
            .unwrap_or(asbd.sample_rate as u32);
        let proc_id = agg
            .create_io_proc_id(io_proc, Some(&mut *ctx))
            .map_err(dev("create IOProc"))?;
        let started =
            ca::device_start(agg, Some(proc_id)).map_err(dev("start aggregate device"))?;

        let info = OpenInfo {
            device: format!("System audio ({out_name})"),
            sample_rate,
            substituted_for: None,
        };
        Ok((
            TapStream {
                _started: started,
                _tap: tap,
                _ctx: ctx,
            },
            info,
        ))
    }

    /// `<output uid>@<nominal rate>`: a Bluetooth headset switching A2DP ↔ HFP keeps its uid but
    /// changes its rate, and the tap must be rebuilt to pick up the new format.
    fn target(&self) -> Option<String> {
        let dev = ca::System::default_output_device().ok()?;
        let uid = dev.uid().ok()?.to_string();
        let rate = dev.nominal_sample_rate().map_or(0, |r| r as u32);
        Some(format!("{uid}@{rate}"))
    }
}
