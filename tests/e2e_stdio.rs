//! End-to-end tests for `engramo-mcp stdio`: spawn the real binary and talk to it over
//! its stdin/stdout.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#;

fn stdio_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_engramo-mcp"));
    // `env_clear` keeps the developer's/CI's own ENGRAMO_* and proxy variables out.
    cmd.arg("stdio")
        .env_clear()
        .env("ENGRAMO_API_URL", "http://127.0.0.1:1")
        .env("APP_ENV", "local")
        .env("RUST_LOG", "off")
        .kill_on_drop(true);
    if let Ok(root) = std::env::var("SystemRoot") {
        cmd.env("SystemRoot", root);
    }
    if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
        // Lets a coverage run collect the child's profile instead of the child writing
        // `default.profraw` into the package root.
        cmd.env("LLVM_PROFILE_FILE", profile);
    }
    cmd
}

#[tokio::test]
async fn stdio_without_token_fails_with_setup_help() {
    let mut cmd = stdio_command();
    cmd.stdin(Stdio::null());
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .expect("engramo-mcp did not exit without a token")
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ENGRAMO_API_TOKEN"), "{stderr}");
    assert!(
        stderr.contains("Generate a token from your EngrAmo account settings"),
        "{stderr}"
    );
}

#[tokio::test]
async fn stdio_answers_initialize_and_exits_on_eof() {
    let mut child = stdio_command()
        .env("ENGRAMO_API_TOKEN", "engramo_t")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn engramo-mcp");
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();

    stdin
        .write_all(format!("{INITIALIZE}\n").as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
    let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("no initialize response")
        .unwrap()
        .expect("stdout closed before a response");
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["id"], 1, "{v}");
    assert_eq!(v["result"]["serverInfo"]["name"], "engramo-mcp", "{v}");

    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("engramo-mcp did not exit on stdin EOF")
        .unwrap();
    assert!(status.success(), "{status}");
}
