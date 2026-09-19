use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
    let output = tokio::process::Command::new(exe)
        .arg("--json")
        .args(args)
        .output()
        .await?;
    let body = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    };
    Ok((output.status.success(), body))
}

async fn call_tool(params: &Value) -> Result<Value, CliError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
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
        _ => {
            return Ok(json!({
                "content": [{"type": "text", "text": format!("Unknown tool: {name}")}],
                "isError": true
            }));
        }
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
        "tools/call" => response(id, call_tool(&request.params).await?),
        _ => error(id, -32601, format!("Method not found: {}", request.method)),
    };
    Ok(Some(result))
}

pub async fn run() -> Result<(), CliError> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let output = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle(request).await?,
            Err(e) => Some(error(Value::Null, -32700, format!("Parse error: {e}"))),
        };
        if let Some(output) = output {
            stdout
                .write_all(serde_json::to_string(&output)?.as_bytes())
                .await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
