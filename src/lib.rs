use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub user: String,
    pub pass: String,
    pub token: String,  // JWT / Bearer token for SurrealDB Cloud or token-based auth
    pub ns: String,
    pub db: String,
    pub lmstudio_url: String,
    pub embedding_model: String,
    pub use_embedding: bool,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            url: std::env::var("SURREALDB_URL")
                .unwrap_or_else(|_| "http://localhost:8000".into()),
            user: std::env::var("SURREALDB_USER")
                .unwrap_or_else(|_| String::new()),
            pass: std::env::var("SURREALDB_PASS")
                .unwrap_or_else(|_| String::new()),
            token: std::env::var("SURREALDB_TOKEN")
                .unwrap_or_else(|_| String::new()),
            ns: std::env::var("SURREALDB_NS")
                .unwrap_or_else(|_| "main".into()),
            db: std::env::var("SURREALDB_DB")
                .unwrap_or_else(|_| "main".into()),
            lmstudio_url: std::env::var("LMSTUDIO_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:1234/v1".into()),
            embedding_model: std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-nomic-embed-text-v1.5".into()),
            use_embedding: std::env::var("USE_EMBEDDING")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(true),
        }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SurrealDbClient {
    client: Client,
    config: Config,
}

impl SurrealDbClient {
    pub fn new(config: Config) -> Self {
        // Build client without default auth headers — auth is sent per-request via basic_auth()
        Self {
            client: Client::builder()
                .no_proxy()
                .build()
                .expect("reqwest Client::new should never fail"),
            config,
        }
    }

    // ------------------------------------------------------------------
    // Internal request helpers
    // ------------------------------------------------------------------

    /// Apply auth to a request: Bearer token takes precedence, then basic_auth.
    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if !self.config.token.is_empty() {
            req.bearer_auth(&self.config.token)
        } else if !self.config.user.is_empty() && !self.config.pass.is_empty() {
            req.basic_auth(&self.config.user, Some(&self.config.pass))
        } else {
            req
        }
    }

    /// Send raw SQL (text/plain) and parse the response.
    async fn exec_raw(&self, sql: &str) -> Result<Value> {
        let req = self
            .client
            .post(format!("{}/sql", self.config.url));

        let req = self.apply_auth(req);

        let resp = req
            .header("surreal-ns", &self.config.ns)
            .header("surreal-db", &self.config.db)
            .header("Content-Type", "text/plain")
            .body(sql.to_owned())
            .send()
            .await
            .context("Failed to connect to SurrealDB")?;
        Self::check_response(resp).await
    }

    /// Replace $data / $args placeholders with inline JSON SurrealQL, then exec_raw.
    /// SurrealDB v3 HTTP API echoes param-bound JSON queries without executing —
    /// so we inline the values directly.
    async fn exec_inline(&self, sql: &str, data_value: &Value) -> Result<Value> {
        let inline = serde_json::to_string(data_value)?;
        let sql = if sql.contains("$data") {
            sql.replace("$data", &inline)
        } else if sql.contains("$args") {
            sql.replace("$args", &inline)
        } else {
            sql.to_string()
        };
        self.exec_raw(&sql).await
    }

    /// Parse an HTTP response from SurrealDB, checking status and ERR payloads.
    /// Handles both JSON and non-JSON (plain-text error) responses gracefully.
    async fn check_response(resp: reqwest::Response) -> Result<Value> {
        let http_status = resp.status();

        // Read body as text first (avoids consuming the response on parse failure).
        let body_text = resp
            .text()
            .await
            .context("Failed to read SurrealDB response body")?;

        // Try to parse as JSON.
        let body: Value = match serde_json::from_str(&body_text) {
            Ok(json) => json,
            Err(_) => {
                // Non-JSON response — usually auth failure (plain-text error).
                let preview = if body_text.len() > 200 {
                    format!("{}...", &body_text[..200])
                } else {
                    body_text.clone()
                };
                anyhow::bail!(
                    "SurrealDB returned non-JSON response (HTTP {}): {}. \
                     Hint: If SurrealDB is running with --unauthenticated, \
                     try setting SURREALDB_USER and SURREALDB_PASS to empty strings. \
                     For token-based auth, set SURREALDB_TOKEN.",
                    http_status,
                    preview,
                );
            }
        };

        if !http_status.is_success() {
            anyhow::bail!(
                "SurrealDB HTTP {}: {}",
                http_status,
                serde_json::to_string(&body).unwrap_or_default()
            );
        }

        if let Some(arr) = body.as_array() {
            for item in arr {
                if item.get("status").and_then(|v| v.as_str()) == Some("ERR") {
                    let msg = item
                        .get("result")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown SurrealDB error");
                    anyhow::bail!("SurrealDB query error: {}", msg);
                }
            }
        }

        Ok(body)
    }

    // ------------------------------------------------------------------
    // Tool methods — each corresponds to one MCP tool
    // ------------------------------------------------------------------

    pub async fn query(&self, sql: &str) -> Result<Value> {
        self.exec_raw(sql).await
    }

    pub async fn select_records(
        &self,
        table: &str,
        filter: Option<&str>,
        order: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Value> {
        let mut sql = format!("SELECT * FROM {}", escape_surql(table));
        if let Some(f) = filter {
            sql.push_str(&format!(" WHERE {}", f));
        }
        if let Some(o) = order {
            sql.push_str(&format!(" ORDER BY {}", o));
        }
        if let Some(l) = limit {
            sql.push_str(&format!(" LIMIT {}", l));
        }
        if let Some(o) = offset {
            sql.push_str(&format!(" START {}", o));
        }
        self.exec_raw(&sql).await
    }

    pub async fn create_record(&self, table: &str, data: Option<Value>) -> Result<Value> {
        let table = escape_surql(table);
        match data {
            Some(d) => {
                let sql = format!("CREATE {} CONTENT $data", table);
                self.exec_inline(&sql, &d).await
            }
            None => {
                let sql = format!("CREATE {}", table);
                self.exec_raw(&sql).await
            }
        }
    }

    pub async fn insert_data(&self, table: &str, data: Value) -> Result<Value> {
        let sql = format!("INSERT INTO {} $data", escape_surql(table));
        self.exec_inline(&sql, &data).await
    }

    pub async fn upsert_data(&self, table: &str, data: Value) -> Result<Value> {
        let sql = format!("UPSERT {} CONTENT $data", escape_surql(table));
        self.exec_inline(&sql, &data).await
    }

    pub async fn update_records(
        &self,
        table: &str,
        data: Value,
        filter: Option<&str>,
    ) -> Result<Value> {
        let mut sql = format!("UPDATE {} MERGE $data", escape_surql(table));
        if let Some(f) = filter {
            sql.push_str(&format!(" WHERE {}", f));
        }
        self.exec_inline(&sql, &data).await
    }

    pub async fn delete_records(&self, table: &str, filter: Option<&str>) -> Result<Value> {
        let mut sql = format!("DELETE {}", escape_surql(table));
        if let Some(f) = filter {
            sql.push_str(&format!(" WHERE {}", f));
        }
        self.exec_raw(&sql).await
    }

    pub async fn relate_records(
        &self,
        from: &str,
        edge: &str,
        to: &str,
        data: Option<Value>,
    ) -> Result<Value> {
        let sql = format!(
            "RELATE {} -> {} -> {}",
            escape_surql(from),
            escape_surql(edge),
            escape_surql(to),
        );
        match data {
            Some(d) => {
                let full = format!("{} CONTENT $data", sql);
                self.exec_inline(&full, &d).await
            }
            None => self.exec_raw(&sql).await,
        }
    }

    pub async fn call_function(&self, func: &str, args: Option<Vec<Value>>) -> Result<Value> {
        // SurrealDB v3: functions use `RETURN func(args)` — SELECT needs FROM.
        // Args are spread: `math::sin(1.57)` not `math::sin([1.57])`.
        let sql = match args {
            Some(a) if !a.is_empty() => {
                let parts: Vec<String> = a.iter()
                    .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()))
                    .collect();
                format!("RETURN {}({})", escape_surql(func), parts.join(", "))
            }
            _ => {
                format!("RETURN {}()", escape_surql(func))
            }
        };
        self.exec_raw(&sql).await
    }

    pub async fn list_schema(&self, scope: &str) -> Result<Value> {
        let sql = build_info_sql(scope);
        self.exec_raw(&sql).await
    }

    pub async fn use_context(&self, ns: Option<&str>, db: Option<&str>) -> Result<Value> {
        match (ns, db) {
            (Some(n), Some(d)) => {
                let sql = format!("USE NS {} DB {}", escape_surql(n), escape_surql(d));
                self.exec_raw(&sql).await
            }
            (Some(n), None) => {
                let sql = format!("USE NS {}", escape_surql(n));
                self.exec_raw(&sql).await
            }
            (None, Some(d)) => {
                let sql = format!("USE DB {}", escape_surql(d));
                self.exec_raw(&sql).await
            }
            (None, None) => anyhow::bail!("At least one of ns or db must be provided"),
        }
    }

    pub async fn info_schema(&self, scope: &str) -> Result<Value> {
        let lower = scope.to_lowercase();
        let sql = match lower.as_str() {
            // SurrealDB v3 doesn't expose version() — return system info instead
            "engine" | "version" => "INFO FOR KV".to_string(),
            _ => build_info_sql(scope),
        };
        self.exec_raw(&sql).await
    }

    // ------------------------------------------------------------------
    // Knowledge-specific methods (keep existing)
    // ------------------------------------------------------------------

    pub async fn insert_knowledge(
        &self,
        project: &str,
        type_: &str,
        title: &str,
        body: &str,
        tags: &[String],
    ) -> Result<Value> {
        let tags_surql: Vec<String> =
            tags.iter().map(|t| format!("'{}'", escape_surql(t))).collect();
        let sql = format!(
            "INSERT INTO knowledge (project, type, title, body, tags) VALUES ('{}', '{}', '{}', '{}', [{}]) RETURN *;",
            escape_surql(project),
            escape_surql(type_),
            escape_surql(title),
            escape_surql(body),
            tags_surql.join(", "),
        );
        self.exec_raw(&sql).await
    }

    pub async fn search_knowledge(
        &self,
        query: &str,
        project: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<Value> {
        let mut conditions: Vec<String> = Vec::new();

        let text_or = format!(
            "(title CONTAINS '{}' OR body CONTAINS '{}')",
            escape_surql(query),
            escape_surql(query),
        );
        conditions.push(text_or);

        if let Some(p) = project {
            conditions.push(format!("project = '{}'", escape_surql(p)));
        }

        if let Some(t) = tags {
            for tag in t {
                conditions.push(format!("tags CONTAINS '{}'", escape_surql(tag)));
            }
        }

        let where_clause = conditions.join(" AND ");
        let sql = format!("SELECT * FROM knowledge WHERE {};", where_clause);
        self.exec_raw(&sql).await
    }

    // ------------------------------------------------------------------
    // Embedding / vector methods
    // ------------------------------------------------------------------

    /// Embed text via LM Studio (OpenAI-compatible API).
    /// Returns a 768-dimensional vector for nomic-embed-text-v1.5.
    pub async fn lmstudio_embed(&self, text: &str) -> Result<Vec<f64>> {
        if !self.config.use_embedding {
            anyhow::bail!("Embedding is disabled");
        }

        let url = format!("{}/embeddings", self.config.lmstudio_url);
        let body = json!({
            "model": self.config.embedding_model,
            "input": text,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to connect to LM Studio")?;

        let http_status = resp.status();
        let body_text = resp
            .text()
            .await
            .context("Failed to read LM Studio response body")?;

        let json_body: Value = serde_json::from_str(&body_text)
            .context("Failed to parse LM Studio response as JSON")?;

        if !http_status.is_success() {
            anyhow::bail!(
                "LM Studio HTTP {}: {}",
                http_status,
                serde_json::to_string(&json_body).unwrap_or_default()
            );
        }

        let embedding = json_body["data"][0]["embedding"]
            .as_array()
            .context("LM Studio response missing data[0].embedding")?
            .iter()
            .map(|v| v.as_f64().context("Embedding value is not a valid f64"))
            .collect::<Result<Vec<f64>>>()?;

        Ok(embedding)
    }

    /// Store a record in the knowledge table with an auto-generated embedding vector.
    /// The body text is embedded via LM Studio before insertion.
    pub async fn store_with_vector(
        &self,
        project: &str,
        type_: &str,
        title: &str,
        body: &str,
        tags: &[String],
    ) -> Result<Value> {
        let vector = self.lmstudio_embed(body).await?;
        let vector_str: String = vector
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let tags_surql: Vec<String> =
            tags.iter().map(|t| format!("'{}'", escape_surql(t))).collect();
        let sql = format!(
            "INSERT INTO knowledge (project, type, title, body, tags, vector) VALUES ('{}', '{}', '{}', '{}', [{}], [{}]) RETURN *",
            escape_surql(project),
            escape_surql(type_),
            escape_surql(title),
            escape_surql(body),
            tags_surql.join(", "),
            vector_str,
        );
        self.exec_raw(&sql).await
    }

    /// Semantic search via DISKANN vector index.
    /// Embeds the query and performs vector similarity search.
    pub async fn find_similar(
        &self,
        query: &str,
        k: usize,
        ef: usize,
        project: Option<&str>,
        min_score: Option<f64>,
    ) -> Result<Value> {
        let query_vec = self.lmstudio_embed(query).await?;
        let query_vec_str: String = query_vec
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");

        let mut sql = format!(
            "SELECT *, vector::similarity::cosine(vector, [{}]) AS dist FROM knowledge WHERE vector <|{},{}|> [{}]",
            query_vec_str, k, ef, query_vec_str,
        );

        if let Some(p) = project {
            sql.push_str(&format!(" AND project = '{}'", escape_surql(p)));
        }

        if let Some(ms) = min_score {
            sql.push_str(&format!(" AND dist >= {}", ms));
        }

        sql.push_str(&format!(" ORDER BY dist LIMIT {}", k));

        self.exec_raw(&sql).await
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn escape_surql(s: &str) -> String {
    s.replace("\\", "\\\\").replace("\'", "\\'")
}

/// Build an `INFO FOR ...` SQL string from a scope descriptor.
///
/// Accepted forms:
/// - `"KV"` → `INFO FOR KV`
/// - `"NS"` → `INFO FOR NS`
/// - `"DB"` → `INFO FOR DB`
/// - `"TABLE user"` → `INFO FOR TABLE user`
/// - `"SCOPE web"` → `INFO FOR SCOPE web`
/// - etc.
fn build_info_sql(scope: &str) -> String {
    let trimmed = scope.trim();
    if trimmed.is_empty() {
        return "INFO FOR DB".into();
    }
    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let kind = parts[0].to_uppercase();
    match kind.as_str() {
        "KV" | "NS" | "DB" => format!("INFO FOR {}", kind),
        "TABLE" | "SCOPE" | "INDEX" | "USER" | "TOKEN" | "EVENT" | "FIELD" => {
            if parts.len() > 1 {
                // INFO FOR TABLE <name> — valid in v3
                format!("INFO FOR {} {}", kind.as_str(), escape_surql(parts[1]))
            } else {
                // Without a name these require a name in v3.
                // Fall back to DB-level info which includes tables/users/events etc.
                "INFO FOR DB".into()
            }
        }
        "DATABASE" => "INFO FOR DB".into(),
        // FUNCTION, PARAM, ANALYZER are not valid INFO targets in SurrealDB v3.
        // Fall back to DB-level info.
        "FUNCTION" | "PARAM" | "ANALYZER" => "INFO FOR DB".into(),
        // Bare table/scope name without keyword prefix
        _ => {
            // Try interpreting as a table name
            format!("INFO FOR TABLE {}", escape_surql(trimmed))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_surql_works() {
        assert_eq!(escape_surql("hello"), "hello");
        assert_eq!(escape_surql("it's"), "it\\'s");
        assert_eq!(escape_surql("'quoted'"), "\\'quoted\\'");
        assert_eq!(escape_surql("path\\to\\file"), "path\\\\to\\\\file");
        assert_eq!(escape_surql("\\'escaped\\'"), "\\\\\\'escaped\\\\\\'");
        assert_eq!(escape_surql(""), "");
    }

    #[test]
    fn build_info_sql_kv() {
        assert_eq!(build_info_sql("KV"), "INFO FOR KV");
    }

    #[test]
    fn build_info_sql_ns() {
        assert_eq!(build_info_sql("NS"), "INFO FOR NS");
    }

    #[test]
    fn build_info_sql_db() {
        assert_eq!(build_info_sql("DB"), "INFO FOR DB");
    }

    #[test]
    fn build_info_sql_table() {
        assert_eq!(build_info_sql("TABLE user"), "INFO FOR TABLE user");
    }

    #[test]
    fn build_info_sql_table_no_name() {
        // SurrealDB v3 requires a name after TABLE; fall back to DB-level info.
        assert_eq!(build_info_sql("TABLE"), "INFO FOR DB");
    }

    #[test]
    fn build_info_sql_bare_name_falls_back_to_table() {
        let result = build_info_sql("my_table");
        assert_eq!(result, "INFO FOR TABLE my_table");
    }

    #[test]
    fn build_info_sql_database_mapped_to_db() {
        let r1 = build_info_sql("DATABASE");
        assert_eq!(r1, "INFO FOR DB");
        let r2 = build_info_sql("DATABASE foo");
        assert_eq!(r2, "INFO FOR DB");
    }

    #[test]
    fn build_info_sql_empty_defaults_to_db() {
        assert_eq!(build_info_sql(""), "INFO FOR DB");
        assert_eq!(build_info_sql("   "), "INFO FOR DB");
    }

    /// Helper: run a closure with a clean env snapshot.
    fn with_clean_env<F: FnOnce()>(f: F) {
        let keys = [
            "SURREALDB_URL",
            "SURREALDB_USER",
            "SURREALDB_PASS",
            "SURREALDB_TOKEN",
            "SURREALDB_NS",
            "SURREALDB_DB",
            "LMSTUDIO_URL",
            "EMBEDDING_MODEL",
            "USE_EMBEDDING",
        ];
        let saved: Vec<(String, Option<String>)> = keys
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, _) in &saved {
            std::env::remove_var(k);
        }
        f();
        for (k, v) in saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }

    #[test]
    fn config_from_env_defaults() {
        with_clean_env(|| {
            std::env::set_var("SURREALDB_USER", "test_user");
            std::env::set_var("SURREALDB_PASS", "test_pass");

            let cfg = Config::from_env();
            assert_eq!(cfg.url, "http://localhost:8000");
            assert_eq!(cfg.user, "test_user");
            assert_eq!(cfg.pass, "test_pass");
            assert_eq!(cfg.token, "");
            assert_eq!(cfg.ns, "main");
            assert_eq!(cfg.db, "main");
        });
    }

    #[test]
    fn config_from_env_custom() {
        with_clean_env(|| {
            std::env::set_var("SURREALDB_URL", "http://127.0.0.1:9999");
            std::env::set_var("SURREALDB_USER", "u1");
            std::env::set_var("SURREALDB_PASS", "p1");
            std::env::set_var("SURREALDB_TOKEN", "tok_xxx");
            std::env::set_var("SURREALDB_NS", "ns1");
            std::env::set_var("SURREALDB_DB", "db1");

            let cfg = Config::from_env();
            assert_eq!(cfg.url, "http://127.0.0.1:9999");
            assert_eq!(cfg.user, "u1");
            assert_eq!(cfg.pass, "p1");
            assert_eq!(cfg.token, "tok_xxx");
            assert_eq!(cfg.ns, "ns1");
            assert_eq!(cfg.db, "db1");
        });
    }

    #[test]
    fn config_from_env_embedding_defaults() {
        with_clean_env(|| {
            let cfg = Config::from_env();
            assert_eq!(cfg.lmstudio_url, "http://127.0.0.1:1234/v1");
            assert_eq!(cfg.embedding_model, "text-embedding-nomic-embed-text-v1.5");
            assert_eq!(cfg.use_embedding, true);
        });
    }

    #[test]
    fn config_from_env_embedding_custom() {
        with_clean_env(|| {
            std::env::set_var("LMSTUDIO_URL", "http://custom:8080/v1");
            std::env::set_var("EMBEDDING_MODEL", "custom-model");
            std::env::set_var("USE_EMBEDDING", "false");

            let cfg = Config::from_env();
            assert_eq!(cfg.lmstudio_url, "http://custom:8080/v1");
            assert_eq!(cfg.embedding_model, "custom-model");
            assert_eq!(cfg.use_embedding, false);
        });
    }
}
