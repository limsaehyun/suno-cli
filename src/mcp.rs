use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, Notify, oneshot};

use crate::errors::CliError;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerateArgs {
    title: String,
    tags: String,
    #[serde(default)]
    lyrics: Option<String>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    variety: Option<u64>,
    #[serde(default)]
    wait: bool,
    #[serde(default = "default_true")]
    dry_run: bool,
    #[serde(default)]
    confirm_spend: bool,
}

fn default_model() -> String {
    "v6".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()}
    })
}

fn tools() -> Value {
    json!({"tools": [
        {
            "name": "suno_models",
            "description": "List Suno models available to the authenticated account.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "suno_credits",
            "description": "Read the authenticated account's plan and remaining credits.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "suno_generate",
            "description": "Preview or submit a Suno V6 generation. Paid submission requires confirm_spend=true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "maxLength": 80},
                    "tags": {"type": "string", "maxLength": 1000},
                    "lyrics": {"type": "string", "maxLength": 5000},
                    "instrumental": {"type": "boolean", "default": false},
                    "model": {"type": "string", "enum": ["v6", "v6-wild", "v6-mini"], "default": "v6"},
                    "duration": {"type": "integer", "minimum": 10, "maximum": 360},
                    "variety": {"type": "integer", "minimum": 0, "maximum": 4},
                    "wait": {"type": "boolean", "default": false},
                    "dry_run": {"type": "boolean", "default": true},
                    "confirm_spend": {"type": "boolean", "default": false}
                },
                "required": ["title", "tags"],
                "additionalProperties": false
            }
        }
    ]})
}

fn invalid_arguments(message: impl Into<String>) -> Value {
    json!({
        "content": [{"type": "text", "text": format!("Invalid arguments: {}", message.into())}],
        "isError": true
    })
}

async fn invoke_cli(args: &[String]) -> Result<(bool, String), CliError> {
    let exe = std::env::current_exe()?;
    let mut command = tokio::process::Command::new(exe);
    command.arg("--json").args(args).kill_on_drop(true);
    let output = command.output().await?;
    let body = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    };
    Ok((output.status.success(), body))
}

async fn call_tool(name: &str, args: Value) -> Result<Value, CliError> {
    let argv = match name {
        "suno_models" => {
            if let Err(error) = serde_json::from_value::<EmptyArgs>(args) {
                return Ok(invalid_arguments(error.to_string()));
            }
            vec!["models".to_string()]
        }
        "suno_credits" => {
            if let Err(error) = serde_json::from_value::<EmptyArgs>(args) {
                return Ok(invalid_arguments(error.to_string()));
            }
            vec!["credits".to_string()]
        }
        "suno_generate" => {
            let args = match serde_json::from_value::<GenerateArgs>(args) {
                Ok(args) => args,
                Err(error) => return Ok(invalid_arguments(error.to_string())),
            };
            if args.title.chars().count() > 80 {
                return Ok(invalid_arguments("title exceeds 80 characters"));
            }
            if args.tags.chars().count() > 1000 {
                return Ok(invalid_arguments("tags exceeds 1000 characters"));
            }
            if args
                .lyrics
                .as_ref()
                .is_some_and(|lyrics| lyrics.chars().count() > 5000)
            {
                return Ok(invalid_arguments("lyrics exceeds 5000 characters"));
            }
            if !matches!(args.model.as_str(), "v6" | "v6-wild" | "v6-mini") {
                return Ok(invalid_arguments("model must be v6, v6-wild, or v6-mini"));
            }
            if args
                .duration
                .is_some_and(|duration| !(10..=360).contains(&duration))
            {
                return Ok(invalid_arguments("duration must be from 10 to 360"));
            }
            if args.variety.is_some_and(|variety| variety > 4) {
                return Ok(invalid_arguments("variety must be from 0 to 4"));
            }
            if !args.dry_run && !args.confirm_spend {
                return Ok(json!({
                    "content": [{"type": "text", "text": "Paid generation requires confirm_spend=true"}],
                    "isError": true
                }));
            }
            if args.title.trim().is_empty() || args.tags.trim().is_empty() {
                return Ok(json!({
                    "content": [{"type": "text", "text": "title and tags are required"}],
                    "isError": true
                }));
            }
            let mut argv = vec![
                "generate".to_string(),
                "--title".to_string(),
                args.title,
                "--tags".to_string(),
                args.tags,
                "--model".to_string(),
                args.model,
            ];
            if let Some(lyrics) = args.lyrics {
                argv.extend(["--lyrics".to_string(), lyrics]);
            }
            if args.instrumental {
                argv.push("--instrumental".to_string());
            }
            if let Some(duration) = args.duration {
                argv.extend(["--duration".to_string(), duration.to_string()]);
            }
            if let Some(variety) = args.variety {
                argv.extend(["--variety".to_string(), variety.to_string()]);
            }
            if args.wait {
                argv.push("--wait".to_string());
            }
            if args.dry_run {
                argv.push("--dry-run".to_string());
            }
            argv
        }
        _ => unreachable!("tool name validated before dispatch"),
    };

    let (ok, body) = invoke_cli(&argv).await?;
    Ok(json!({
        "content": [{"type": "text", "text": body}],
        "isError": !ok
    }))
}

async fn handle(request: Request) -> Result<Option<Value>, CliError> {
    let Some(id) = request.id else {
        return Ok(None);
    };
    let result = match request.method.as_str() {
        "initialize" => response(
            id,
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "suno-cli", "version": env!("CARGO_PKG_VERSION")}
            }),
        ),
        "ping" => response(id, json!({})),
        "tools/list" => response(id, tools()),
        "tools/call" => {
            let Some(name) = request.params.get("name").and_then(Value::as_str) else {
                return Ok(Some(error(id, -32602, "Missing tool name")));
            };
            if !matches!(name, "suno_models" | "suno_credits" | "suno_generate") {
                return Ok(Some(error(id, -32602, format!("Unknown tool: {name}"))));
            }
            let args = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            response(id, call_tool(name, args).await?)
        }
        _ => error(id, -32601, format!("Method not found: {}", request.method)),
    };
    Ok(Some(result))
}

async fn write_response(stdout: &Mutex<tokio::io::Stdout>, output: &Value) -> Result<(), CliError> {
    let mut stdout = stdout.lock().await;
    stdout
        .write_all(serde_json::to_string(output)?.as_bytes())
        .await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

pub async fn run() -> Result<(), CliError> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
    let pending = Arc::new(Mutex::new(HashMap::<String, oneshot::Sender<()>>::new()));
    let completed = Arc::new(Notify::new());
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let output = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                if request.method == "notifications/cancelled" {
                    if let Some(request_id) = request.params.get("requestId") {
                        let key = serde_json::to_string(request_id)?;
                        if let Some(cancel) = pending.lock().await.remove(&key) {
                            let _ = cancel.send(());
                        }
                    }
                    continue;
                }
                if request.method == "tools/call"
                    && let Some(id) = request.id.clone()
                {
                    let key = serde_json::to_string(&id)?;
                    let (cancel, mut cancelled) = oneshot::channel();
                    pending.lock().await.insert(key.clone(), cancel);
                    let stdout = Arc::clone(&stdout);
                    let pending = Arc::clone(&pending);
                    let completed = Arc::clone(&completed);
                    tokio::spawn(async move {
                        let output = tokio::select! {
                            _ = &mut cancelled => None,
                            result = handle(request) => match result {
                                Ok(output) => output,
                                Err(e) => Some(error(id, -32603, format!("Internal error: {e}"))),
                            },
                        };
                        if let Some(output) = output {
                            let _ = write_response(&stdout, &output).await;
                        }
                        pending.lock().await.remove(&key);
                        completed.notify_waiters();
                    });
                    continue;
                }
                let id = request.id.clone().unwrap_or(Value::Null);
                match handle(request).await {
                    Ok(output) => output,
                    Err(e) => Some(error(id, -32603, format!("Internal error: {e}"))),
                }
            }
            Err(e) => Some(error(Value::Null, -32700, format!("Parse error: {e}"))),
        };
        if let Some(output) = output {
            write_response(&stdout, &output).await?;
        }
    }
    loop {
        let notified = completed.notified();
        if pending.lock().await.is_empty() {
            break;
        }
        notified.await;
    }
    Ok(())
}
