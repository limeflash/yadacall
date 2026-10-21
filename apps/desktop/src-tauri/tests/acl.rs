//! Every command the app registers must be in the build manifest and granted to the main window,
//! otherwise the release build rejects it at runtime with "Command … not allowed by ACL"
//! (issue #3: the folder and bookmark commands shipped without capability entries).

use std::{collections::BTreeSet, path::Path};

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Command names registered with `generate_handler!` (`commands::name` paths in lib.rs).
fn registered_commands() -> BTreeSet<String> {
    let lib = read("src/lib.rs");
    lib.match_indices("commands::")
        .map(|(at, prefix)| {
            lib[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|name| {
            !name.is_empty() && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        })
        .collect()
}

/// The `COMMANDS` list in build.rs (each becomes an `allow-<name>` permission).
fn manifest_commands() -> BTreeSet<String> {
    let build = read("build.rs");
    let list = build
        .split("const COMMANDS")
        .nth(1)
        .and_then(|rest| rest.split("];").next())
        .expect("COMMANDS list in build.rs");
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

fn granted(capability: &str) -> BTreeSet<String> {
    let json: serde_json::Value = serde_json::from_str(&read(capability)).expect("capability JSON");
    json["permissions"]
        .as_array()
        .expect("permissions array")
        .iter()
        .filter_map(|p| p.as_str())
        .filter_map(|p| p.strip_prefix("allow-"))
        .map(|p| p.replace('-', "_"))
        .collect()
}

#[test]
fn every_registered_command_is_in_the_build_manifest() {
    let missing: Vec<_> = registered_commands()
        .difference(&manifest_commands())
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "add to COMMANDS in build.rs: {missing:?}"
    );
}

#[test]
fn the_main_window_is_granted_every_command() {
    let missing: Vec<_> = manifest_commands()
        .difference(&granted("capabilities/main.json"))
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "grant in capabilities/main.json as allow-<name>: {missing:?}"
    );
}
