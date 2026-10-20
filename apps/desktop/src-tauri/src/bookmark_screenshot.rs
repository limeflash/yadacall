//! Fast, best-effort capture for live bookmarks. Work is always run off the recorder thread.

use std::io::Cursor;

use device_query::DeviceQuery;
use serde::{Deserialize, Serialize};
use xcap::{
    Monitor, Window,
    image::{self, DynamicImage, ImageEncoder, imageops::FilterType},
};

/// Which visual to attach to a live bookmark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BookmarkScreenshot {
    Off,
    #[default]
    Window,
    Screen,
}

/// Capture adapter seam; tests can supply an in-memory fake.
pub trait ScreenshotCapture: Send + Sync {
    /// Captures and encodes a JPEG, or returns `None` when capture is unavailable.
    fn capture(&self, mode: BookmarkScreenshot, own_pid: u32) -> Result<Option<Vec<u8>>, String>;
}

/// Calls a capture adapter; exposed so bookmark workflows can be exercised with a fake adapter.
pub fn capture_with(
    capture: &dyn ScreenshotCapture,
    mode: BookmarkScreenshot,
    own_pid: u32,
) -> Result<Option<Vec<u8>>, String> {
    capture.capture(mode, own_pid)
}

/// Platform implementation backed by xcap.
#[derive(Debug, Default)]
pub struct XcapCapture;

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowInfo {
    pid: u32,
    z: i32,
    visible: bool,
}

fn choose_window(windows: &[WindowInfo], own_pid: u32) -> Option<usize> {
    windows
        .iter()
        .enumerate()
        .filter(|(_, w)| w.visible && w.pid != own_pid)
        .max_by_key(|(_, w)| w.z)
        .map(|(index, _)| index)
}

fn encode_jpeg(mut image: image::RgbaImage) -> Result<Vec<u8>, String> {
    let (width, height) = image.dimensions();
    let longest = width.max(height);
    if longest > 1920 {
        let width = ((u64::from(width) * 1920) / u64::from(longest)).max(1) as u32;
        let height = ((u64::from(height) * 1920) / u64::from(longest)).max(1) as u32;
        image = image::imageops::resize(&image, width, height, FilterType::Triangle);
    }
    let rgb = DynamicImage::ImageRgba8(image).to_rgb8();
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut bytes), 80)
        .write_image(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

impl ScreenshotCapture for XcapCapture {
    fn capture(&self, mode: BookmarkScreenshot, own_pid: u32) -> Result<Option<Vec<u8>>, String> {
        if mode == BookmarkScreenshot::Off {
            return Ok(None);
        }
        let windows = Window::all().map_err(|e| e.to_string())?;
        let infos: Vec<_> = windows
            .iter()
            .map(|w| WindowInfo {
                pid: w.pid().unwrap_or_default(),
                z: w.z().unwrap_or(i32::MAX),
                visible: !w.is_minimized().unwrap_or(true),
            })
            .collect();
        let target = choose_window(&infos, own_pid).map(|i| &windows[i]);
        let image = match (mode, target) {
            (BookmarkScreenshot::Window, Some(window)) => {
                window.capture_image().map_err(|e| e.to_string())?
            }
            (BookmarkScreenshot::Window, None) => return Ok(None),
            (BookmarkScreenshot::Screen, Some(window)) => window
                .current_monitor()
                .and_then(|m| m.capture_image())
                .map_err(|e| e.to_string())?,
            (BookmarkScreenshot::Screen, None) => {
                let cursor =
                    device_query::DeviceState::checked_new().map(|state| state.get_mouse().coords);
                let Some((x, y)) = cursor else {
                    return Ok(None);
                };
                Monitor::from_point(x, y)
                    .and_then(|m| m.capture_image())
                    .map_err(|e| e.to_string())?
            }
            (BookmarkScreenshot::Off, _) => return Ok(None),
        };
        encode_jpeg(image).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcap::image::GenericImageView;

    #[test]
    fn frontmost_non_yadacall_window_is_selected() {
        let windows = [
            WindowInfo {
                pid: 8,
                z: 2,
                visible: true,
            },
            WindowInfo {
                pid: 9,
                z: 0,
                visible: true,
            },
            WindowInfo {
                pid: 10,
                z: 1,
                visible: true,
            },
        ];
        assert_eq!(choose_window(&windows, 8), Some(2));
        assert_eq!(choose_window(&windows[..1], 8), None);
    }

    #[test]
    fn jpeg_is_resized_and_encoded() {
        let bytes = encode_jpeg(image::RgbaImage::new(2400, 1200)).unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!(image.dimensions(), (1920, 960));
        assert!(bytes.starts_with(&[0xff, 0xd8]));
    }

    #[test]
    fn capture_adapter_can_be_faked() {
        struct Fake;
        impl ScreenshotCapture for Fake {
            fn capture(
                &self,
                mode: BookmarkScreenshot,
                _own_pid: u32,
            ) -> Result<Option<Vec<u8>>, String> {
                assert_eq!(mode, BookmarkScreenshot::Window);
                Ok(Some(b"fake".to_vec()))
            }
        }
        assert_eq!(
            capture_with(&Fake, BookmarkScreenshot::Window, 1).unwrap(),
            Some(b"fake".to_vec())
        );
    }
}
