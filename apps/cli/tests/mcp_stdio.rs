use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use yc_store::Store;

#[test]
fn mcp_cli_initializes_lists_tools_and_calls_list_meetings() {
    let temp = tempfile::tempdir().expect("temp directory");
    let data = temp.path().join("data");
    let library = temp.path().join("library");
    let store = Store::open(&library, data.join("index.sqlite")).expect("fixture store");
    store
        .set_setting("settings", &json!({"libraryDir":library.to_string_lossy()}))
        .expect("saved library path");
    drop(store);

    let mut child = Command::new(env!("CARGO_BIN_EXE_yadacall"))
        .args(["mcp"])
        .env("YADACALL_DATA_DIR", &data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start CLI MCP server");
    let mut input = child.stdin.take().expect("stdin");
    let output = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(output);

    write_request(
        &mut input,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
    );
    let initialized = read_response(&mut reader);
    assert_eq!(initialized["id"], 1);
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .expect("notify initialized");
    write_request(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    let tools = read_response(&mut reader);
    assert!(
        tools["result"]["tools"]
            .as_array()
            .is_some_and(|xs| xs.iter().any(|t| t["name"] == "list_meetings"))
    );
    write_request(
        &mut input,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_meetings","arguments":{}}}),
    );
    let call = read_response(&mut reader);
    assert_eq!(call["id"], 3);
    assert!(call["result"]["content"].as_array().is_some());
    drop(input);
    let _ = child.kill();
    child.wait().expect("wait for stopped MCP server");
}

fn write_request(input: &mut impl Write, value: Value) {
    writeln!(input, "{value}").expect("write MCP request");
}

fn read_response(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(
            reader.read_line(&mut line).expect("read MCP response"),
            0,
            "server closed stdout"
        );
        if let Ok(value) = serde_json::from_str::<Value>(&line)
            && value.get("id").is_some()
        {
            return value;
        }
    }
}
