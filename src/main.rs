/// SurrealDB MCP server — binary crate.
///
/// Thin JSON-RPC 2.0 loop over stdin/stdout that wires MCP methods
/// (`initialize`, `tools/list`, `tools/call`, …) to the `SurrealDbClient`
/// in the library crate.
///
/// # Protocol
///
/// * Messages are newline-delimited JSON-RPC 2.0 objects on stdin/stdout.
/// * Stderr is reserved for diagnostics / tracing — it is **not** part of
///   the MCP transport.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb_mcp::{Config, SurrealDbClient};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    #[serde(default)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// ---------------------------------------------------------------------------
// Response builders
// ---------------------------------------------------------------------------

fn make_error(id: Option<Value>, code: i32, message: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message,
            data: None,
        }),
    }
}

fn make_success(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result: Some(result),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Tool definitions (MCP "list" response)
// ---------------------------------------------------------------------------

fn tool_definitions() -> Value {
    json!([
        {
            "name": "surrealdb-query",
            "description": "Execute a raw SQL / SURQL query against SurrealDB",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": {
                        "type": "string",
                        "description": "The SQL query to execute (e.g. SELECT * FROM knowledge;)"
                    }
                },
                "required": ["sql"]
            }
        },
        {
            "name": "surrealdb-insert",
            "description": "Insert a knowledge record into the 'knowledge' table",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project / namespace identifier" },
                    "type":    { "type": "string", "description": "Record type (e.g. 'doc', 'note', 'reference')" },
                    "title":   { "type": "string", "description": "Record title" },
                    "body":    { "type": "string", "description": "Record body / content" },
                    "tags":    { "type": "array", "items": { "type": "string" }, "description": "Tags for the record" }
                },
                "required": ["project", "type", "title", "body", "tags"]
            }
        },
        {
            "name": "surrealdb-search",
            "description": "Search knowledge records by full-text query with optional project/tag filters",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query":   { "type": "string", "description": "Search text (matched against title and body via CONTAINS)" },
                    "project": { "type": "string", "description": "Optional: filter by project" },
                    "tags":    { "type": "array", "items": { "type": "string" }, "description": "Optional: filter by tags (AND semantics)" }
                },
                "required": ["query"]
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// Request dispatcher
// ---------------------------------------------------------------------------

async fn handle_request(req: JsonRpcRequest, client: &SurrealDbClient) -> JsonRpcResponse {
    let id = req.id;

    match req.method.as_str() {
        // ── Lifecycle ──────────────────────────────────────────────
        "initialize" => make_success(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "surrealdb-mcp",
                    "version": "0.1.0"
                }
            }),
        ),

        "notifications/initialized" | "notifications/cancelled" | "notifications/roots/list_changed" => {
            // Notifications carry no id → caller already skips writing.
            // Return a no-op response (will not be transmitted).
            JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: None,
                result: None,
                error: None,
            }
        }

        "ping" => make_success(id, json!({})),

        // ── Tool discovery ─────────────────────────────────────────
        "tools/list" => make_success(id, json!({ "tools": tool_definitions() })),

        // ── Tool execution ─────────────────────────────────────────
        "tools/call" => {
            let name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let arguments = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));

            let tool_result = match name {
                "surrealdb-query" => {
                    let sql = arguments
                        .get("sql")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if sql.is_empty() {
                        return make_error(
                            id,
                            -32602,
                            "Missing required parameter: sql".into(),
                        );
                    }
                    match client.query(sql).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => {
                            return make_error(id, -32603, format!("Query failed: {e}"));
                        }
                    }
                }

                "surrealdb-insert" => {
                    let project = arguments.get("project").and_then(|v| v.as_str()).unwrap_or("");
                    let type_ = arguments.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let title = arguments.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    let body = arguments.get("body").and_then(|v| v.as_str()).unwrap_or("");
                    let tags: Vec<String> = arguments
                        .get("tags")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_default();

                    if project.is_empty() || type_.is_empty() || title.is_empty() || body.is_empty() {
                        return make_error(
                            id,
                            -32602,
                            "Missing required parameters. Required: project, type, title, body, tags".into(),
                        );
                    }
                    match client.insert_knowledge(project, type_, title, body, &tags).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => {
                            return make_error(id, -32603, format!("Insert failed: {e}"));
                        }
                    }
                }

                "surrealdb-search" => {
                    let query = arguments
                        .get("query")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let project = arguments.get("project").and_then(|v| v.as_str());
                    let tags: Option<Vec<String>> = arguments
                        .get("tags")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

                    if query.is_empty() {
                        return make_error(
                            id,
                            -32602,
                            "Missing required parameter: query".into(),
                        );
                    }
                    match client
                        .search_knowledge(query, project, tags.as_deref())
                        .await
                    {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => {
                            return make_error(id, -32603, format!("Search failed: {e}"));
                        }
                    }
                }

                other => {
                    return make_error(id, -32601, format!("Unknown tool: {other}"));
                }
            };

            make_success(id, tool_result)
        }

        // ── Fallback ───────────────────────────────────────────────
        _ => make_error(id, -32601, format!("Method not found: {}", req.method)),
    }
}

/// Wrap a JSON value as an MCP text content block.
fn mcp_text_content(data: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&data).unwrap_or_default()
        }]
    })
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Send all diagnostics to stderr so stdout stays clean for MCP.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();

    tracing::info!(
        "surrealdb-mcp starting — SurrealDB URL: {}",
        config.url
    );

    let client = SurrealDbClient::new(config);

    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                // Parse error — we have no id, use null per JSON-RPC spec
                let err_resp = make_error(Some(Value::Null), -32700, format!("Parse error: {e}"));
                let _ = write_json_to_stdout(&err_resp).await;
                continue;
            }
        };

        let is_notification = req.id.is_none();
        let resp = handle_request(req, &client).await;

        // Notifications have no `id` → client expects no response
        if !is_notification && resp.id.is_some() {
            if let Err(e) = write_json_to_stdout(&resp).await {
                tracing::error!("Failed to write response to stdout: {e}");
                break;
            }
        }
    }

    tracing::info!("stdin closed — surrealdb-mcp shutting down");
}

/// Serialise a JSON-RPC response and write it (newline-terminated) to stdout.
async fn write_json_to_stdout(resp: &JsonRpcResponse) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string(resp)?;
    let mut stdout = tokio::io::stdout();
    stdout.write_all(json.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}
