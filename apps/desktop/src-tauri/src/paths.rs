//! Where YadaCall keeps things: app data (index, models, runtimes, calibration) and the meeting library.
//!
//! App data comes from Tauri's path resolver (`app_data_dir`), overridable with `YADACALL_DATA_DIR` for
//! development and tests. The library defaults to `~/Созвоны` when the UI language is Russian and
//! `~/YadaCall` otherwise; the user can move it in the settings.

use std::path::{Path, PathBuf};

/// Environment variable that overrides the app data directory.
pub const DATA_DIR_ENV: &str = "YADACALL_DATA_DIR";

/// Home directory of the current user.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Default library folder for a UI language.
pub fn default_library_dir(russian: bool) -> PathBuf {
    home_dir().join(if russian { "Созвоны" } else { "YadaCall" })
}

/// Whether the system language is Russian (used for first-run defaults before the user chose one).
pub fn system_is_russian() -> bool {
    sys_locale::get_locale().is_some_and(|l| l.to_ascii_lowercase().starts_with("ru"))
}

/// Expands a leading `~` or `~/`.
pub fn expand_tilde(path: &str) -> PathBuf {
    let path = path.trim();
    if path == "~" {
        return home_dir();
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return home_dir().join(rest);
    }
    PathBuf::from(path)
}

/// Shows a path with `~` for the home directory (what the settings screen displays).
pub fn collapse_home(path: &Path) -> String {
    let home = home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Resolves the app data directory: the env override, else the given default (Tauri's resolver).
pub fn data_dir(default: impl FnOnce() -> PathBuf) -> PathBuf {
    std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(default)
}

/// Percent-encodes `s` like JavaScript's `encodeURIComponent`.
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The URL the webview uses to load `path` through Tauri's asset protocol (what `convertFileSrc` returns).
pub fn asset_url(path: &Path) -> String {
    let enc = encode_component(&path.to_string_lossy());
    if cfg!(windows) {
        format!("http://asset.localhost/{enc}")
    } else {
        format!("asset://localhost/{enc}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_round_trip() {
        let p = expand_tilde("~/Созвоны");
        assert!(p.ends_with("Созвоны"));
        assert_eq!(collapse_home(&p), "~/Созвоны");
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn asset_url_encodes_like_convert_file_src() {
        let url = asset_url(Path::new("/Users/a b/Созвоны/x.flac"));
        assert!(url.ends_with("%2FUsers%2Fa%20b%2F%D0%A1%D0%BE%D0%B7%D0%B2%D0%BE%D0%BD%D1%8B%2Fx.flac"));
    }
}
