//! Opening a URL in the user's default browser (the app normally passes Tauri's opener instead).

use std::process::{Command, Stdio};

/// Opens `url` with the OS handler; errors are logged, never fatal (the caller also shows the URL).
pub fn open_in_browser(url: &str) {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).stdout(Stdio::null()).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
    } else {
        Command::new("xdg-open")
            .arg(url)
            .stdout(Stdio::null())
            .spawn()
    };
    if let Err(e) = result {
        tracing::warn!("could not open the browser: {e}");
    }
}
