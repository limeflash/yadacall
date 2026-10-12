//! cpal-based capture: microphones everywhere, and WASAPI loopback on Windows (cpal turns an input
//! stream built on a *render* device into a loopback capture).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use yc_core::error::PermissionKind;
use yc_core::{Error, Result};

use crate::config::DeviceSel;
use crate::supervisor::{Backend, OpenInfo, Sink};

/// What the backend records.
pub(crate) enum Source {
    /// A microphone / input device.
    Input(DeviceSel),
    /// Whatever the default output device is playing (Windows).
    #[cfg(target_os = "windows")]
    Loopback,
}

pub(crate) struct CpalBackend {
    host: cpal::Host,
    source: Source,
}

impl CpalBackend {
    pub(crate) fn new(source: Source) -> Self {
        Self {
            host: cpal::default_host(),
            source,
        }
    }

    fn resolve(&self) -> Result<cpal::Device> {
        match &self.source {
            Source::Input(DeviceSel::Default) => {
                self.host.default_input_device().ok_or_else(|| {
                    Error::Device("no default input device (is a microphone connected?)".into())
                })
            }
            Source::Input(DeviceSel::Named(wanted)) => find_input(&self.host, wanted),
            #[cfg(target_os = "windows")]
            Source::Loopback => self
                .host
                .default_output_device()
                .ok_or_else(|| Error::Device("no default output device to capture".into())),
        }
    }

    fn permission(&self) -> PermissionKind {
        match self.source {
            Source::Input(_) => PermissionKind::Microphone,
            #[cfg(target_os = "windows")]
            Source::Loopback => PermissionKind::SystemAudio,
        }
    }
}

fn find_input(host: &cpal::Host, wanted: &str) -> Result<cpal::Device> {
    let devices: Vec<cpal::Device> = host
        .input_devices()
        .map_err(|e| Error::Device(e.to_string()))?
        .collect();
    let name_of = |d: &cpal::Device| {
        d.description()
            .map(|x| x.name().to_owned())
            .unwrap_or_default()
    };
    let by = |pred: &dyn Fn(&cpal::Device) -> bool| devices.iter().position(pred);
    let lower = wanted.to_lowercase();
    let idx = by(&|d| d.id().map(|i| i.to_string() == wanted).unwrap_or(false))
        .or_else(|| by(&|d| name_of(d) == wanted))
        .or_else(|| by(&|d| name_of(d).to_lowercase().contains(&lower)));
    match idx {
        Some(i) => Ok(devices.into_iter().nth(i).expect("index from position()")),
        None => Err(Error::NotFound(format!(
            "input device `{wanted}` (run `yadacall devices`)"
        ))),
    }
}

fn map_err(e: cpal::Error, permission: PermissionKind) -> Error {
    match e.kind() {
        cpal::ErrorKind::PermissionDenied => Error::Permission(permission),
        _ => Error::Device(e.to_string()),
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut sink: Sink,
    permission: PermissionKind,
) -> Result<cpal::Stream>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let failed = sink_failed_handle(&sink);
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| sink.push_interleaved(data, channels, f32::from_sample),
            move |e: cpal::Error| match e.kind() {
                // Informational: the stream keeps running.
                cpal::ErrorKind::DeviceChanged
                | cpal::ErrorKind::Xrun
                | cpal::ErrorKind::RealtimeDenied => {
                    tracing::debug!("cpal: {e}");
                }
                _ => {
                    tracing::warn!("cpal stream error: {e}");
                    failed();
                }
            },
            None,
        )
        .map_err(|e| map_err(e, permission))
}

fn sink_failed_handle(sink: &Sink) -> impl Fn() + Send + 'static {
    let shared = sink.shared_handle();
    move || {
        shared
            .failed
            .store(true, std::sync::atomic::Ordering::Relaxed)
    }
}

impl Backend for CpalBackend {
    type Guard = cpal::Stream;

    fn open(&mut self, sink: Sink) -> Result<(cpal::Stream, OpenInfo)> {
        let permission = self.permission();
        let device = self.resolve()?;
        let supported = if device.supports_input() {
            device.default_input_config()
        } else {
            device.default_output_config()
        }
        .map_err(|e| map_err(e, permission))?;
        let name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "unknown device".into());
        let sample_rate = supported.sample_rate();
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, sink, permission)?,
            SampleFormat::I16 => build::<i16>(&device, config, sink, permission)?,
            SampleFormat::I32 => build::<i32>(&device, config, sink, permission)?,
            SampleFormat::U16 => build::<u16>(&device, config, sink, permission)?,
            other => return Err(Error::Device(format!("unsupported sample format {other}"))),
        };
        stream.play().map_err(|e| map_err(e, permission))?;
        let label = match self.source {
            Source::Input(_) => name,
            #[cfg(target_os = "windows")]
            Source::Loopback => format!("System audio ({name})"),
        };
        Ok((
            stream,
            OpenInfo {
                device: label,
                sample_rate,
            },
        ))
    }

    fn target(&self) -> Option<String> {
        match &self.source {
            Source::Input(DeviceSel::Default) => self
                .host
                .default_input_device()
                .and_then(|d| d.id().ok())
                .map(|i| i.to_string()),
            Source::Input(DeviceSel::Named(n)) => Some(n.clone()),
            #[cfg(target_os = "windows")]
            Source::Loopback => self
                .host
                .default_output_device()
                .and_then(|d| d.id().ok())
                .map(|i| i.to_string()),
        }
    }

    #[cfg(target_os = "windows")]
    fn stall_detect(&self) -> bool {
        // WASAPI loopback delivers nothing while nothing plays.
        !matches!(self.source, Source::Loopback)
    }

    #[cfg(target_os = "windows")]
    fn idle_fill(&self) -> bool {
        matches!(self.source, Source::Loopback)
    }
}
