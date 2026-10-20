// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--mcp") {
        let allow_write = args.iter().any(|a| a == "--allow-write");
        let data_dir = std::env::var_os("YC_DATA_DIR")
            .or_else(|| std::env::var_os("YADACALL_DATA_DIR"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(default_data_dir);
        let result = tokio::runtime::Runtime::new().and_then(|runtime| {
            runtime
                .block_on(yc_mcp::serve(Some(data_dir), None, allow_write))
                .map_err(std::io::Error::other)
        });
        if let Err(error) = result {
            eprintln!("YadaCall MCP server failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    yadacall_desktop_lib::run()
}

fn default_data_dir() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Library/Application Support/app.yadacall.desktop")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("app.yadacall.desktop")
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_default()
                    .join(".local/share")
            })
            .join("app.yadacall.desktop")
    }
}
