//! Linux microphone-holder detection through PulseAudio/PipeWire-Pulse source outputs.

use std::process::Command;

use super::{CallSources, MicProbe, MicUser, TabAccessError, titles};

/// Best-effort source-output inspection using the PulseAudio-compatible `pactl` CLI.
#[derive(Debug, Default)]
pub struct LinuxSources;

impl CallSources for LinuxSources {
    fn microphone_users(&self) -> MicProbe {
        let output = match Command::new("pactl")
            .args(["-f", "json", "list", "source-outputs"])
            .output()
        {
            Ok(output) if output.status.success() => output,
            _ => return MicProbe::Unknown,
        };
        let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) else {
            return MicProbe::Unknown;
        };
        MicProbe::Known(rows.iter().filter_map(mic_user_from_row).collect())
    }

    fn call_tabs(&self, _browser: &MicUser) -> Result<Vec<titles::CallTab>, TabAccessError> {
        Err(TabAccessError::Unavailable)
    }
}

fn mic_user_from_row(row: &serde_json::Value) -> Option<MicUser> {
    let properties = row.get("properties")?.as_object()?;
    let property = |name: &str| properties.get(name).and_then(serde_json::Value::as_str);
    let process_name = property("application.name")
        .or_else(|| property("application.process.binary"))
        .map(str::to_owned)
        .or_else(|| property("application.process.id").map(|pid| format!("process {pid}")))?;
    let pid = property("application.process.id").and_then(|pid| pid.parse().ok());
    let bundle_id = property("application.id")
        .or_else(|| property("application.process.binary"))
        .map(str::to_owned);
    Some(MicUser {
        process_name,
        bundle_id,
        pid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pulse_source_output_process_details() {
        let row = serde_json::json!({
            "properties": {
                "application.name": "Brave",
                "application.process.binary": "brave",
                "application.process.id": "812",
                "application.id": "com.brave.Browser"
            }
        });
        assert_eq!(
            mic_user_from_row(&row),
            Some(MicUser {
                process_name: "Brave".into(),
                bundle_id: Some("com.brave.Browser".into()),
                pid: Some(812),
            })
        );
    }
}
