//! Dev convenience: when the Google OAuth client is not provided through the environment, read
//! `$HOME/.config/yadacall/google-oauth.env` (`KEY=VALUE` lines) and hand the values to rustc so
//! `option_env!` picks them up. Values are never printed to build output.

use std::path::PathBuf;

const KEYS: [&str; 2] = ["YADACALL_GOOGLE_CLIENT_ID", "YADACALL_GOOGLE_CLIENT_SECRET"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for key in KEYS {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return;
    };
    let file: PathBuf = PathBuf::from(home).join(".config/yadacall/google-oauth.env");
    println!("cargo:rerun-if-changed={}", file.display());
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        if KEYS.contains(&key)
            && !value.is_empty()
            && std::env::var_os(key).is_none_or(|v| v.is_empty())
        {
            println!("cargo:rustc-env={key}={value}");
        }
    }
}
