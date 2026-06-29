/// SurrealDB MCP server — library crate.
///
/// Provides the `Config` struct for environment-based configuration
/// and the `SurrealDbClient` struct which wraps the SurrealDB HTTP API
/// (POST /sql) with tool-oriented methods: query, insert_knowledge, search_knowledge.

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// SurrealDB connection parameters, populated from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub user: String,
    pub pass: String,
    pub ns: String,
    pub db: String,
}

impl Config {
    /// Load config from env vars, using sensible defaults.
    ///
    /// | Variable          | Default                  |
    /// |-------------------|--------------------------|
    /// | `SURREALDB_URL`   | `http://localhost:8000`  |
    /// | `SURREALDB_USER`  | –                        |
    /// | `SURREALDB_PASS`  | –                        |
    /// | `SURREALDB_NS`    | `main`                   |
    /// | `SURREALDB_DB`    | `main`                   |
    pub fn from_env() -> Self {
        Self {
            url: std::env::var("SURREALDB_URL")
                .unwrap_or_else(|_| "http://localhost:8000".into()),
            user: std::env::var("SURREALDB_USER")
                .expect("SURREALDB_USER must be set"),
            pass: std::env::var("SURREALDB_PASS")
                .expect("SURREALDB_PASS must be set"),
            ns: std::env::var("SURREALDB_NS")
                .unwrap_or_else(|_| "main".into()),
            db: std::env::var("SURREALDB_DB")
                .unwrap_or_else(|_| "main".into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Thin wrapper around `reqwest::Client` that knows how to talk to SurrealDB's
/// HTTP `/sql` endpoint.
#[derive(Debug)]
pub struct SurrealDbClient {
    client: Client,
    config: Config,
}

impl SurrealDbClient {
    /// Build a new client from a pre-loaded `Config`.
    /// Uses a `Client` with proxy disabled (local SurrealDB should never go through a proxy).
    pub fn new(config: Config) -> Self {
        Self {
            client: Client::builder().no_proxy().build().expect("reqwest Client::new should never fail"),
            config,
        }
    }

    /// Execute a raw SQL / SURQL statement and return the full JSON response
    /// from the SurrealDB HTTP API.
    ///
    /// The response is a JSON array where each element has the shape
    /// `{ "status": "OK"|"ERR", "time": "…", "result": … }`.
    pub async fn query(&self, sql: &str) -> Result<Value> {
        let resp = self
            .client
            .post(format!("{}/sql", self.config.url))
            .basic_auth(&self.config.user, Some(&self.config.pass))
            .header("surreal-ns", &self.config.ns)
            .header("surreal-db", &self.config.db)
            .body(sql.to_owned())
            .send()
            .await
            .context("Failed to connect to SurrealDB")?;

        let http_status = resp.status();
        let body: Value = resp
            .json()
            .await
            .context("Failed to parse SurrealDB response body as JSON")?;

        // Non-2xx → bail immediately
        if !http_status.is_success() {
            anyhow::bail!(
                "SurrealDB HTTP {}: {}",
                http_status,
                serde_json::to_string(&body).unwrap_or_default()
            );
        }

        // SurrealDB may still return 200 with an ERR status inside the array
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

    /// Insert a record into the `knowledge` table.
    ///
    /// Returns the full JSON response from SurrealDB (the created record(s)
    /// will be inside the `result` field of the first array element).
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
        self.query(&sql).await
    }

    /// Search the `knowledge` table using SurrealDB's `CONTAINS` operator.
    ///
    /// * `query` — substring to search for in `title` and `body` (case-sensitive).
    /// * `project` — optional filter on the `project` field.
    /// * `tags` — optional filter; records must contain **every** listed tag.
    ///
    /// # Note on SurrealDB v3
    ///
    /// The original design specified the `~>` full-text operator, but SurrealDB v3.1.5
    /// does not ship with it enabled. The implementation uses `CONTAINS` on both string
    /// and array fields instead, which works out-of-the-box.
    pub async fn search_knowledge(
        &self,
        query: &str,
        project: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<Value> {
        let mut conditions: Vec<String> = Vec::new();

        // Text search — wrap in parens for correct operator precedence
        let text_or = format!(
            "(title CONTAINS '{}' OR body CONTAINS '{}')",
            escape_surql(query),
            escape_surql(query),
        );
        conditions.push(text_or);

        // Optional project filter
        if let Some(p) = project {
            conditions.push(format!("project = '{}'", escape_surql(p)));
        }

        // Optional tags filter (AND semantics)
        if let Some(t) = tags {
            for tag in t {
                conditions.push(format!("tags CONTAINS '{}'", escape_surql(tag)));
            }
        }

        let where_clause = conditions.join(" AND ");
        let sql = format!("SELECT * FROM knowledge WHERE {};", where_clause);
        self.query(&sql).await
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Escape strings for SurrealDB v3 SURQL.
///
/// SurrealDB v3 uses backslash escaping (`\'`), not SQL-style doubling (`''`).
/// Also escape backslash itself to prevent interpretation of escape sequences.
fn escape_surql(s: &str) -> String {
    // Order matters: escape backslash FIRST, then single quote
    s.replace("\\", "\\\\").replace("\'", "\\'")
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

    /// Helper: run a closure with a clean env snapshot (save/restore all SURREALDB_* vars).
    fn with_clean_env<F: FnOnce()>(f: F) {
        let keys = ["SURREALDB_URL", "SURREALDB_USER", "SURREALDB_PASS", "SURREALDB_NS", "SURREALDB_DB"];
        let saved: Vec<(String, Option<String>)> = keys
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();
        // Unset everything first
        for (k, _) in &saved {
            std::env::remove_var(k);
        }
        f();
        // Restore
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
            std::env::set_var("SURREALDB_NS", "ns1");
            std::env::set_var("SURREALDB_DB", "db1");

            let cfg = Config::from_env();
            assert_eq!(cfg.url, "http://127.0.0.1:9999");
            assert_eq!(cfg.user, "u1");
            assert_eq!(cfg.pass, "p1");
            assert_eq!(cfg.ns, "ns1");
            assert_eq!(cfg.db, "db1");
        });
    }
}
