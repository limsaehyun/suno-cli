mod common;

use common::suno;

#[test]
fn stdio_mcp_lists_tools_and_previews_without_auth() {
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"suno_generate","arguments":{"title":"Preview","tags":"ambient","instrumental":true,"dry_run":true}}}"#,
    ]
    .join("\n");
    let output = suno().arg("mcp").write_stdin(input).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "suno-cli");
    assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 3);
    assert_eq!(responses[2]["result"]["isError"], false);
    assert!(
        responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("chirp-hawk")
    );
}

#[test]
fn stdio_mcp_requires_explicit_paid_confirmation() {
    let input = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"suno_generate","arguments":{"title":"Nope","tags":"ambient","dry_run":false}}}"#;
    let output = suno().arg("mcp").write_stdin(input).output().unwrap();
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("confirm_spend=true")
    );
}

#[test]
fn stdio_mcp_rejects_unknown_arguments() {
    let input = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"suno_generate","arguments":{"title":"Preview","tags":"ambient","dry_run":true,"unexpected":"value"}}}"#;
    let output = suno().arg("mcp").write_stdin(input).output().unwrap();
    assert!(output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown field")
    );
}
