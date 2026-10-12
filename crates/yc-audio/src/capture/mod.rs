//! Platform capture backends.
//!
//! | track        | macOS 14.2+                  | Windows            | Linux                        |
//! |--------------|------------------------------|--------------------|------------------------------|
//! | mic          | cpal (Core Audio)            | cpal (WASAPI)      | cpal (ALSA / PulseAudio)     |
//! | system audio | Core Audio process tap       | WASAPI loopback    | PulseAudio/PipeWire monitor  |

pub(crate) mod cpal_backend;

#[cfg(target_os = "macos")]
pub(crate) mod macos_tap;

#[cfg(target_os = "linux")]
pub(crate) mod linux_monitor;
