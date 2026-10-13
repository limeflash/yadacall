//! `yadacall devices` — what the recorder can capture on this machine.

use clap::Args as ClapArgs;
use yc_audio::DeviceInfo;

#[derive(ClapArgs)]
pub struct Args {}

pub fn run(_args: Args) -> anyhow::Result<()> {
    println!("Input devices (microphone track; pass the id or a name part to `record --mic`):");
    print_devices(yc_audio::list_input_devices()?);
    println!("\nOutput devices (system audio follows the default):");
    print_devices(yc_audio::list_output_devices()?);

    println!(
        "\nSystem audio capture: {}",
        match yc_audio::system_audio_support() {
            Ok(how) => how.to_owned(),
            Err(why) => format!("unavailable ({why})"),
        }
    );
    println!(
        "Output is built-in speakers: {}",
        fmt(yc_audio::output_is_builtin_speakers())
    );
    println!(
        "Input is Bluetooth:          {}",
        fmt(yc_audio::input_is_bluetooth())
    );
    if let Some(m) = yc_audio::resolve_default_mic(true) {
        match m.substituted_for {
            Some(headset) => println!(
                "Default microphone:          {} (instead of {headset}, which would drop it to call quality)",
                m.device
            ),
            None => println!("Default microphone:          {}", m.device),
        }
    }
    Ok(())
}

fn fmt(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

fn print_devices(devices: Vec<DeviceInfo>) {
    if devices.is_empty() {
        println!("  (none)");
    }
    for d in devices {
        println!(
            "  {} {}  [{}{}{}]\n      id: {}",
            if d.is_default { "*" } else { " " },
            d.name,
            d.transport.as_deref().unwrap_or("?"),
            d.sample_rate
                .map(|r| format!(", {r} Hz"))
                .unwrap_or_default(),
            d.channels.map(|c| format!(", {c} ch")).unwrap_or_default(),
            d.id,
        );
    }
}
