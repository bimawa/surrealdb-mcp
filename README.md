# surrealdb-mcp

> **MCP (Model Context Protocol) server for SurrealDB** — let AI agents query, insert, and search your SurrealDB knowledge base.

`surrealdb-mcp` bridges the [Model Context Protocol](https://modelcontextprotocol.io) with [SurrealDB](https://surrealdb.com). It runs as a lightweight Rust binary that communicates over **stdin/stdout** using **JSON-RPC 2.0**, exposing SurrealDB operations as MCP tools.

Any MCP client (Claude Desktop, IDE plugins, custom AI agents) can connect and immediately run SQL queries or manage knowledge records.

---

## Features

- **Execute raw SQL / SURQL** — any query you can run in SurrealDB
- **Insert knowledge records** — structured entries with project, type, title, body, tags
- **Full-text search** — search by text with optional project/tag filters
- **Zero external dependencies** for the MCP transport — pure stdin/stdout
- **Configuration via environment variables** — fits any deployment (local, container, CI)

---

## How it works

```
┌──────────────┐   stdin/stdout    ┌──────────────┐   HTTP      ┌────────────┐
│  MCP Client  │ ◄─── JSON-RPC ──► │ surrealdb-mcp│ ──────────► │ SurrealDB  │
│  (Claude,    │                   │  (Rust)      │  POST /sql   │ (database) │
│   IDE, etc.) │                   └──────────────┘              └────────────┘
└──────────────┘
```

The server speaks MCP over standard I/O — every line on stdin is a JSON-RPC request, every line on stdout is a response. Stderr is used for diagnostics/logging.

---

## Quick start

### Prerequisites

- Rust 1.75+ (or use the pre-built binary)
- A running SurrealDB instance

### Run

```bash
# Clone and build
git clone <your-repo-url>
cd surrealdb-mcp
cargo build --release

# Set environment
export SURREALDB_URL=http://localhost:8000
export SURREALDB_USER=root
export SURREALDB_PASS=root
export SURREALDB_NS=main
export SURREALDB_DB=main

# Start the server (MCP transport over stdin/stdout)
./target/release/surrealdb-mcp
```

### Run with Claude Desktop

Add to your `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "surrealdb": {
      "command": "/path/to/surrealdb-mcp",
      "env": {
        "SURREALDB_URL": "http://localhost:8000",
        "SURREALDB_USER": "root",
        "SURREALDB_PASS": "root",
        "SURREALDB_NS": "main",
        "SURREALDB_DB": "main"
      }
    }
  }
}
```

---

## Configuration

| Variable | Default | Required | Description |
|---|---|---|---|
| `SURREALDB_URL` | `http://localhost:8000` | No | SurrealDB HTTP endpoint |
| `SURREALDB_USER` | — | **Yes** | SurrealDB username |
| `SURREALDB_PASS` | — | **Yes** | SurrealDB password |
| `SURREALDB_NS` | `main` | No | Namespace |
| `SURREALDB_DB` | `main` | No | Database |

---

## MCP Tools

### `surrealdb-query`

Execute a raw SQL / SURQL statement.

```json
{
  "sql": "SELECT * FROM knowledge WHERE project = 'my-app';"
}
```

### `surrealdb-insert`

Insert a structured knowledge record into the `knowledge` table.

```json
{
  "project": "my-app",
  "type": "doc",
  "title": "Architecture Overview",
  "body": "The system consists of three services…",
  "tags": ["architecture", "backend"]
}
```

### `surrealdb-search`

Search knowledge records by text content with optional filters.

```json
{
  "query": "authentication",
  "project": "my-app",
  "tags": ["security"]
}
```

> **Note:** The search uses SurrealDB's `CONTAINS` operator (works out-of-the-box on SurrealDB v3.x). Full-text search via `~>` requires additional SurrealDB configuration.

---

## Project layout

```
surrealdb-mcp/
├── src/
│   ├── main.rs         # Binary crate — stdin/stdout JSON-RPC loop + MCP dispatcher
│   └── lib.rs          # Library crate — SurrealDbClient, Config, query/insert/search
├── Cargo.toml
├── Cargo.lock
└── README.md
```

---

## Development

```bash
# Build
cargo build

# Run with tracing (stderr)
RUST_LOG=debug cargo run

# Test
cargo test

# Release build
cargo build --release
```

The server logs diagnostics to **stderr** at the level set by `RUST_LOG` (default: `info`).

---

## Testing

```rust
// Unit tests for config loading and SQL escaping
cargo test
```

The library includes tests for:
- Environment variable parsing with defaults
- Custom configuration overrides
- Single-quote escaping for SURQL injection safety

Integration tests against a real SurrealDB instance are best run separately with a dedicated test database.

---

## License

MIT
