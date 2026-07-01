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
// Tool definitions (MCP "list" response)
// ---------------------------------------------------------------------------

fn tool_definitions() -> Value {
    json!([
        {
            "name": "surrealdb-query",
            "description": "Execute a raw SQL / SURQL statement against SurrealDB",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": {
                        "type": "string",
                        "description": "The SURQL query to execute"
                    }
                },
                "required": ["sql"]
            }
        },
        {
            "name": "surrealdb-select",
            "description": "Select records from a table",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table": { "type": "string", "description": "Table name" },
                    "filter": { "type": "string", "description": "Optional WHERE clause (e.g. 'age > 21')" },
                    "order": { "type": "string", "description": "Optional ORDER BY clause (e.g. 'created_at DESC')" },
                    "limit": { "type": "number", "description": "Optional LIMIT count" },
                    "offset": { "type": "number", "description": "Optional START / OFFSET count" }
                },
                "required": ["table"]
            }
        },
        {
            "name": "surrealdb-create",
            "description": "Create a new record in a table (auto-generates record ID)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table": { "type": "string", "description": "Table name" },
                    "data": {
                        "type": "object",
                        "description": "Optional record data as JSON object (uses CONTENT syntax)"
                    }
                },
                "required": ["table"]
            }
        },
        {
            "name": "surrealdb-insert",
            "description":
                "General-purpose INSERT: provide a table name and data (object or array of objects).\n\
                 For knowledge-base insertion use the 'project', 'type', 'title', 'body', 'tags' params instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table": { "type": "string", "description": "Table name (required for general insert)" },
                    "data": {
                        "description": "Record data as a JSON object or array of objects (required for general insert)"
                    },
                    "project": { "type": "string", "description": "Knowledge-base: project name" },
                    "type": { "type": "string", "description": "Knowledge-base: record type" },
                    "title": { "type": "string", "description": "Knowledge-base: record title" },
                    "body": { "type": "string", "description": "Knowledge-base: record body" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Knowledge-base: tags"
                    }
                },
                "oneOf": [
                    { "required": ["table", "data"] },
                    { "required": ["project", "type", "title", "body"] }
                ]
            }
        },
        {
            "name": "surrealdb-upsert",
            "description": "Upsert a record — create or replace based on unique identifier",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table": { "type": "string", "description": "Table name" },
                    "data": { "type": "object", "description": "Record data as JSON object (uses CONTENT syntax)" }
                },
                "required": ["table", "data"]
            }
        },
        {
            "name": "surrealdb-update",
            "description": "Update records in a table",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table": { "type": "string", "description": "Table name" },
                    "data": { "type": "object", "description": "Fields to merge (uses MERGE syntax)" },
                    "filter": { "type": "string", "description": "Optional WHERE clause" }
                },
                "required": ["table", "data"]
            }
        },
        {
            "name": "surrealdb-delete",
            "description": "Delete records from a table",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table": { "type": "string", "description": "Table name" },
                    "filter": { "type": "string", "description": "Optional WHERE clause" }
                },
                "required": ["table"]
            }
        },
        {
            "name": "surrealdb-relate",
            "description": "Create a graph edge between two records",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Source record ID (e.g. 'user:123')" },
                    "edge": { "type": "string", "description": "Edge type (e.g. 'purchased')" },
                    "to": { "type": "string", "description": "Target record ID (e.g. 'product:456')" },
                    "data": { "type": "object", "description": "Optional edge properties as JSON object" }
                },
                "required": ["from", "edge", "to"]
            }
        },
        {
            "name": "surrealdb-run",
            "description": "Call a SurrealDB database function",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "func": { "type": "string", "description": "Function name (e.g. 'math::sin', 'array::sort')" },
                    "args": {
                        "type": "array",
                        "items": {},
                        "description": "Optional arguments array"
                    }
                },
                "required": ["func"]
            }
        },
        {
            "name": "surrealdb-list",
            "description": "List schema objects — namespaces, databases, tables, indexes, users, etc.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "description": "What to list: 'KV', 'NS', 'DB', 'TABLE', 'TABLE <name>', 'SCOPE', 'INDEX', 'USER', 'TOKEN', 'PARAM', 'EVENT', 'FIELD', 'FUNCTION', 'ANALYZER'"
                    }
                },
                "required": ["scope"]
            }
        },
        {
            "name": "surrealdb-use",
            "description": "Switch the active namespace and/or database context",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ns": { "type": "string", "description": "Optional: namespace to switch to" },
                    "db": { "type": "string", "description": "Optional: database to switch to" }
                },
                "anyOf": [
                    { "required": ["ns"] },
                    { "required": ["db"] }
                ]
            }
        },
        {
            "name": "surrealdb-info",
            "description": "Get schema or engine information for a scope",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "description": "Scope: 'DB', 'TABLE <name>', 'KV', 'NS', 'SCOPE <name>', 'engine' (or 'version')"
                    }
                },
                "required": ["scope"]
            }
        },
        {
            "name": "surrealdb-search",
            "description": "Search knowledge records by full-text query with optional project/tag filters",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search text" },
                    "project": { "type": "string", "description": "Optional: filter by project" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional: filter by tags (AND semantics)"
                    }
                },
                "required": ["query"]
            }
        },
        {
            "name": "surrealdb-embed",
            "description": "Convert text to an embedding vector via LM Studio (nomic-embed-text-v1.5, 768-dim)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to embed" }
                },
                "required": ["text"]
            }
        },
        {
            "name": "surrealdb-store",
            "description": "Store text with auto-generated embedding vector into the knowledge base. Automatically embeds the 'body' text via LM Studio before inserting.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project name" },
                    "type": { "type": "string", "description": "Record type (e.g. 'doc', 'note')" },
                    "title": { "type": "string", "description": "Record title" },
                    "body": { "type": "string", "description": "Record body content (will be embedded)" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tags" }
                },
                "required": ["project", "type", "title", "body"]
            }
        },
        {
            "name": "surrealdb-find",
            "description": "Semantic search via DISKANN vector index. Embeds the query text and performs vector similarity search on the knowledge base.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language search query" },
                    "k": { "type": "number", "description": "Number of results to return (default: 10)" },
                    "ef": { "type": "number", "description": "DISKANN EF parameter — search breadth (default: 100)" },
                    "project": { "type": "string", "description": "Optional: filter by project name" },
                    "min_score": { "type": "number", "description": "Optional: minimum similarity score (0.0 to 1.0)" }
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
                    "version": "0.2.0"
                }
            }),
        ),

        "notifications/initialized" | "notifications/cancelled" | "notifications/roots/list_changed" => {
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
            let tool_name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));

            let tool_result = match tool_name {
                // ── Raw query ───────────────────────────────────────
                "surrealdb-query" => {
                    let sql = args.get("sql").and_then(|v| v.as_str()).unwrap_or("");
                    if sql.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: sql".into());
                    }
                    match client.query(sql).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Query failed: {e}")),
                    }
                }

                // ── Select ──────────────────────────────────────────
                "surrealdb-select" => {
                    let table = args.get("table").and_then(|v| v.as_str()).unwrap_or("");
                    if table.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: table".into());
                    }
                    let filter = args.get("filter").and_then(|v| v.as_str());
                    let order = args.get("order").and_then(|v| v.as_str());
                    let limit = args.get("limit").and_then(|v| v.as_i64());
                    let offset = args.get("offset").and_then(|v| v.as_i64());
                    match client.select_records(table, filter, order, limit, offset).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Select failed: {e}")),
                    }
                }

                // ── Create ──────────────────────────────────────────
                "surrealdb-create" => {
                    let table = args.get("table").and_then(|v| v.as_str()).unwrap_or("");
                    if table.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: table".into());
                    }
                    let data = args.get("data").cloned();
                    match client.create_record(table, data).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Create failed: {e}")),
                    }
                }

                // ── Insert (dual-route: general OR knowledge) ──────
                "surrealdb-insert" => {
                    let has_table = args.get("table").and_then(|v| v.as_str()).map_or(false, |s| !s.is_empty());
                    let has_project = args.get("project").and_then(|v| v.as_str()).map_or(false, |s| !s.is_empty());

                    if has_project {
                        // Knowledge path
                        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
                        let type_ = args.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
                        let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
                        let tags: Vec<String> = args
                            .get("tags")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default();

                        if project.is_empty() || type_.is_empty() || title.is_empty() || body.is_empty() {
                            return make_error(
                                id,
                                -32602,
                                "Missing required knowledge params. Required: project, type, title, body".into(),
                            );
                        }
                        match client.insert_knowledge(project, type_, title, body, &tags).await {
                            Ok(data) => mcp_text_content(data),
                            Err(e) => return make_error(id, -32603, format!("Insert knowledge failed: {e}")),
                        }
                    } else if has_table {
                        // General data path
                        let table = args.get("table").and_then(|v| v.as_str()).unwrap_or("");
                        let data = match args.get("data") {
                            Some(d) if !d.is_null() => d.clone(),
                            _ => {
                                return make_error(id, -32602, "Missing required parameter: data".into());
                            }
                        };
                        match client.insert_data(table, data).await {
                            Ok(data) => mcp_text_content(data),
                            Err(e) => return make_error(id, -32603, format!("Insert failed: {e}")),
                        }
                    } else {
                        return make_error(
                            id,
                            -32602,
                            "Provide either 'table'+'data' for general insert, or 'project'+'type'+'title'+'body' for knowledge insert".into(),
                        );
                    }
                }

                // ── Upsert ──────────────────────────────────────────
                "surrealdb-upsert" => {
                    let table = args.get("table").and_then(|v| v.as_str()).unwrap_or("");
                    if table.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: table".into());
                    }
                    let data = match args.get("data") {
                        Some(d) if !d.is_null() => d.clone(),
                        _ => return make_error(id, -32602, "Missing required parameter: data".into()),
                    };
                    match client.upsert_data(table, data).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Upsert failed: {e}")),
                    }
                }

                // ── Update ──────────────────────────────────────────
                "surrealdb-update" => {
                    let table = args.get("table").and_then(|v| v.as_str()).unwrap_or("");
                    if table.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: table".into());
                    }
                    let data = match args.get("data") {
                        Some(d) if !d.is_null() => d.clone(),
                        _ => return make_error(id, -32602, "Missing required parameter: data".into()),
                    };
                    let filter = args.get("filter").and_then(|v| v.as_str());
                    match client.update_records(table, data, filter).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Update failed: {e}")),
                    }
                }

                // ── Delete ──────────────────────────────────────────
                "surrealdb-delete" => {
                    let table = args.get("table").and_then(|v| v.as_str()).unwrap_or("");
                    if table.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: table".into());
                    }
                    let filter = args.get("filter").and_then(|v| v.as_str());
                    match client.delete_records(table, filter).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Delete failed: {e}")),
                    }
                }

                // ── Relate ──────────────────────────────────────────
                "surrealdb-relate" => {
                    let from = args.get("from").and_then(|v| v.as_str()).unwrap_or("");
                    let edge = args.get("edge").and_then(|v| v.as_str()).unwrap_or("");
                    let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("");
                    if from.is_empty() || edge.is_empty() || to.is_empty() {
                        return make_error(
                            id,
                            -32602,
                            "Missing required parameters. Required: from, edge, to".into(),
                        );
                    }
                    let data = args.get("data").cloned();
                    match client.relate_records(from, edge, to, data).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Relate failed: {e}")),
                    }
                }

                // ── Run (function call) ─────────────────────────────
                "surrealdb-run" => {
                    let func = args.get("func").and_then(|v| v.as_str()).unwrap_or("");
                    if func.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: func".into());
                    }
                    let fn_args = args.get("args").and_then(|v| v.as_array()).cloned();
                    match client.call_function(func, fn_args).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Run failed: {e}")),
                    }
                }

                // ── List schema ─────────────────────────────────────
                "surrealdb-list" => {
                    let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("");
                    if scope.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: scope".into());
                    }
                    match client.list_schema(scope).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("List failed: {e}")),
                    }
                }

                // ── Use context ─────────────────────────────────────
                "surrealdb-use" => {
                    let ns = args.get("ns").and_then(|v| v.as_str());
                    let db = args.get("db").and_then(|v| v.as_str());
                    if ns.is_none() && db.is_none() {
                        return make_error(
                            id,
                            -32602,
                            "At least one of 'ns' or 'db' must be provided".into(),
                        );
                    }
                    match client.use_context(ns, db).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Use failed: {e}")),
                    }
                }

                // ── Info ────────────────────────────────────────────
                "surrealdb-info" => {
                    let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("");
                    if scope.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: scope".into());
                    }
                    match client.info_schema(scope).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Info failed: {e}")),
                    }
                }

                // ── Search (knowledge) ──────────────────────────────
                "surrealdb-search" => {
                    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                    let project = args.get("project").and_then(|v| v.as_str());
                    let tags: Option<Vec<String>> = args
                        .get("tags")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect());

                    if query.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: query".into());
                    }
                    match client.search_knowledge(query, project, tags.as_deref()).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Search failed: {e}")),
                    }
                }

                // ── Embed ────────────────────────────────────────────
                "surrealdb-embed" => {
                    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    if text.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: text".into());
                    }
                    match client.lmstudio_embed(text).await {
                        Ok(embedding) => mcp_text_content(json!({"embedding": embedding})),
                        Err(e) => return make_error(id, -32603, format!("Embed failed: {e}")),
                    }
                }

                // ── Store with vector ────────────────────────────────
                "surrealdb-store" => {
                    let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
                    let type_ = args.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
                    let tags: Vec<String> = args
                        .get("tags")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_default();

                    if project.is_empty() || type_.is_empty() || title.is_empty() || body.is_empty() {
                        return make_error(
                            id,
                            -32602,
                            "Missing required parameters. Required: project, type, title, body".into(),
                        );
                    }
                    match client.store_with_vector(project, type_, title, body, &tags).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Store failed: {e}")),
                    }
                }

                // ── Find similar (vector search) ─────────────────────
                "surrealdb-find" => {
                    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                    let k = args.get("k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                    let ef = args.get("ef").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
                    let project = args.get("project").and_then(|v| v.as_str());
                    let min_score = args.get("min_score").and_then(|v| v.as_f64());

                    if query.is_empty() {
                        return make_error(id, -32602, "Missing required parameter: query".into());
                    }
                    match client.find_similar(query, k, ef, project, min_score).await {
                        Ok(data) => mcp_text_content(data),
                        Err(e) => return make_error(id, -32603, format!("Find failed: {e}")),
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

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let config = Config::from_env();

    tracing::info!(
        "surrealdb-mcp v{} starting — SurrealDB URL: {}",
        "0.2.0",
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
                let err_resp = make_error(Some(Value::Null), -32700, format!("Parse error: {e}"));
                let _ = write_json_to_stdout(&err_resp).await;
                continue;
            }
        };

        let is_notification = req.id.is_none();
        let resp = handle_request(req, &client).await;

        if !is_notification && resp.id.is_some() {
            if let Err(e) = write_json_to_stdout(&resp).await {
                tracing::error!("Failed to write response to stdout: {e}");
                break;
            }
        }
    }

    tracing::info!("stdin closed — surrealdb-mcp shutting down");
}

async fn write_json_to_stdout(resp: &JsonRpcResponse) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string(resp)?;
    let mut stdout = tokio::io::stdout();
    stdout.write_all(json.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}
