//! The terminal's tools over the Model Context Protocol, so Claude Desktop
//! and Claude Code can use them within a Claude subscription
//! (`miso-terminal --mcp`). The terminal sends nothing to Anthropic on this
//! route: the client calls the tools, the tools read MISO's public data.
//!
//! The transport is MCP's stdio one: JSON-RPC 2.0 messages, one per line,
//! on stdin and stdout. Logs must never go to stdout.

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::tools::{self, ToolBox};

/// The protocol versions this server speaks, newest first. A client asking
/// for another gets the newest, as the protocol says.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Who the server says it is.
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    /// Read by clients as guidance for the model (the price reference).
    pub instructions: String,
}

/// Serve until the client closes stdin. Malformed lines get a JSON-RPC
/// parse error and the session goes on.
pub async fn serve<R, W>(
    tools: &dyn ToolBox,
    info: &ServerInfo,
    input: R,
    mut output: W,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = input.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let mut replies = Vec::new();
                for msg in batch {
                    if let Some(r) = handle(tools, info, msg).await {
                        replies.push(r);
                    }
                }
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(msg) => handle(tools, info, msg).await,
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(reply) = reply {
            let mut text = reply.to_string();
            text.push('\n');
            output.write_all(text.as_bytes()).await?;
            output.flush().await?;
        }
    }
    Ok(())
}

/// One message; `None` for notifications (no `id`), which get no reply.
async fn handle(tools: &dyn ToolBox, info: &ServerInfo, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str().unwrap_or_default();
    let params = &msg["params"];
    let Some(id) = id else {
        // `notifications/initialized`, `notifications/cancelled`: nothing to say.
        return None;
    };
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = VERSIONS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": info.name, "version": info.version},
                "instructions": info.instructions,
            })
        }
        "ping" => json!({}),
        "tools/list" => {
            let list: Vec<Value> = tools
                .specs()
                .into_iter()
                .map(|s| {
                    json!({
                        "name": s.name,
                        "description": s.description,
                        "inputSchema": s.input_schema,
                        "annotations": {"readOnlyHint": true, "openWorldHint": false},
                    })
                })
                .collect();
            json!({"tools": list})
        }
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(error(id, -32602, "tools/call needs a tool name"));
            };
            if !tools.specs().iter().any(|s| s.name == name) {
                return Some(error(id, -32602, &format!("unknown tool {name}")));
            }
            let args = match &params["arguments"] {
                Value::Null => json!({}),
                a => a.clone(),
            };
            let (text, is_error) = match tools::run(tools, name, args).await {
                Ok(v) => (v.to_string(), false),
                Err(e) => (e, true),
            };
            json!({"content": [{"type": "text", "text": text}], "isError": is_error})
        }
        other => return Some(error(id, -32601, &format!("unknown method {other}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolResult, ToolSpec};
    use mt_data::BoxFuture;

    struct Echo;

    impl ToolBox for Echo {
        fn specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "echo".into(),
                description: "Says it back".into(),
                input_schema: json!({"type": "object",
                    "properties": {"say": {"type": "string"}},
                    "required": ["say"], "additionalProperties": false}),
            }]
        }
        fn call<'a>(&'a self, _name: &'a str, input: Value) -> BoxFuture<'a, ToolResult> {
            Box::pin(async move {
                match input["say"].as_str() {
                    Some("fail") => Err("asked to fail".into()),
                    Some(s) => Ok(json!({"said": s})),
                    None => Err("nothing".into()),
                }
            })
        }
    }

    /// Plays a client's lines against the server and returns its replies.
    async fn session(lines: &[Value]) -> Vec<Value> {
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let mut input = input.into_bytes();
        input.extend_from_slice(b"this is not json\n\n");
        let mut out = Vec::new();
        let info = ServerInfo {
            name: "miso-terminal".into(),
            version: "0.3.0".into(),
            instructions: "Prices are in $/MWh.".into(),
        };
        serve(&Echo, &info, &input[..], &mut out)
            .await
            .expect("served");
        String::from_utf8(out)
            .expect("utf8")
            .lines()
            .map(|l| serde_json::from_str(l).expect("one JSON message per line"))
            .collect()
    }

    #[tokio::test]
    async fn a_client_session() {
        let replies = session(&[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                           "clientInfo": {"name": "test", "version": "1"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "echo", "arguments": {"say": "hi"}}}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": {"name": "echo", "arguments": {"say": 5}}}),
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": {"name": "echo", "arguments": {"say": "fail"}}}),
            json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
                "params": {"name": "place_order", "arguments": {}}}),
            json!({"jsonrpc": "2.0", "id": 7, "method": "resources/list"}),
            json!([{"jsonrpc": "2.0", "id": 8, "method": "ping"},
                   {"jsonrpc": "2.0", "method": "notifications/cancelled"}]),
        ])
        .await;
        assert_eq!(replies.len(), 9, "{replies:#?}");
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "miso-terminal");
        assert_eq!(replies[0]["result"]["instructions"], "Prices are in $/MWh.");
        let tool = &replies[1]["result"]["tools"][0];
        assert_eq!(tool["name"], "echo");
        assert_eq!(tool["inputSchema"]["required"][0], "say");
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(
            replies[2]["result"]["content"][0]["text"],
            "{\"said\":\"hi\"}"
        );
        assert_eq!(replies[2]["result"]["isError"], false);
        assert_eq!(replies[3]["result"]["isError"], true);
        assert_eq!(
            replies[3]["result"]["content"][0]["text"],
            "invalid input: input.say should be string"
        );
        assert_eq!(replies[4]["result"]["content"][0]["text"], "asked to fail");
        assert_eq!(replies[5]["error"]["code"], -32602);
        assert_eq!(replies[6]["error"]["code"], -32601);
        assert_eq!(replies[7][0]["id"], 8);
        assert_eq!(replies[8]["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn an_unknown_protocol_version_gets_the_newest() {
        let replies = session(&[json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "1999-01-01"}})])
        .await;
        assert_eq!(replies[0]["result"]["protocolVersion"], VERSIONS[0]);
    }
}
