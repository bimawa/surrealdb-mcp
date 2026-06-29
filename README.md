# surrealdb-mcp

> **MCP (Model Context Protocol) server for SurrealDB** — let AI agents fully manage a SurrealDB database through 13 dedicated tools.

`surrealdb-mcp` bridges the [Model Context Protocol](https://modelcontextprotocol.io) with [SurrealDB](https://surrealdb.com). It runs as a lightweight Rust binary that communicates over **stdin/stdout** using **JSON-RPC 2.0**, exposing SurrealDB operations as MCP tools.

Any MCP client (Claude Desktop, IDE plugins, custom AI agents, opencode) can connect and immediately run queries, manage schema, manipulate records, and work with knowledge entries.

---

## Features

- **13 MCP tools** — covers the full SurrealDB v3.x API surface
- **Raw SURQL execution** — any query you can run in SurrealDB
- **Typed CRUD helpers** — select, create, insert, upsert, update, delete
- **Graph edge management** — `relate` tool for relationships
- **Schema exploration** — `list` and `info` tools
- **Database functions** — `run` tool for `fn::*` calls
- **Context switching** — `use` tool for NS/DB selection
- **Knowledge base** — dedicated `insert` and `search` tools for the `knowledge` table
- **Parameterised queries** — safe value binding via SurrealDB's JSON API
- **Zero external dependencies** for MCP transport — pure stdin/stdout
- **JWT / Bearer token auth** — for SurrealDB Cloud
- **Configuration via environment variables** — fits any deployment

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
- A running SurrealDB instance (v3.x recommended)

### Build

```bash
git clone <your-repo-url>
cd surrealdb-mcp
cargo build --release
```

> **Note:** If `CARGO_TARGET_DIR` is set, the binary will be at `$CARGO_TARGET_DIR/release/surrealdb-mcp`, not `./target/release/`.

### Run

```bash
export SURREALDB_URL=http://localhost:8000
export SURREALDB_USER=root
export SURREALDB_PASS=root
export SURREALDB_NS=main
export SURREALDB_DB=main

# Start the server (MCP transport over stdin/stdout)
surrealdb-mcp
```

For SurrealDB Cloud or token-based auth:
```bash
export SURREALDB_TOKEN=your_jwt_token_here
# SURREALDB_USER and SURREALDB_PASS are ignored when SURREALDB_TOKEN is set
```

For `--unauthenticated` SurrealDB (no auth required):
```bash
export SURREALDB_USER=
export SURREALDB_PASS=
# All three of USER, PASS, TOKEN empty → no auth header sent
```

### Run with Claude Desktop / opencode

Add to your `claude_desktop_config.json` or `~/.config/opencode/opencode.json`:

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

Restart the MCP client. All 13 tools will appear in the AI assistant.

---

## MCP Tools

| # | Tool | Description |
|---|------|-------------|
| 1 | `surrealdb-query` | Execute a raw SQL / SURQL statement |
| 2 | `surrealdb-select` | Select records from a table |
| 3 | `surrealdb-create` | Create a new record (auto-generates ID) |
| 4 | `surrealdb-insert` | Insert records (general) **or** insert knowledge (when `project`/`type`/`title`/`body` are provided) |
| 5 | `surrealdb-upsert` | Upsert a record (create or replace) |
| 6 | `surrealdb-update` | Update records with MERGE semantics |
| 7 | `surrealdb-delete` | Delete records from a table |
| 8 | `surrealdb-relate` | Create a graph edge between two records |
| 9 | `surrealdb-run` | Call a SurrealDB database function |
| 10 | `surrealdb-list` | List schema objects (KV, NS, DB, TABLE, INDEX, USER, …) |
| 11 | `surrealdb-use` | Switch the active namespace and/or database |
| 12 | `surrealdb-info` | Get schema or engine information |
| 13 | `surrealdb-search` | Search knowledge records by text with optional filters |

### Tool reference

#### `surrealdb-query`
Execute any SurrealQL statement.
```json
{ "sql": "SELECT * FROM user WHERE age > 21 ORDER BY name LIMIT 10;" }
```

#### `surrealdb-select`
Select records from a table with optional filtering, ordering, and pagination.
```json
{
  "table": "user",
  "filter": "age > 21",
  "order": "created_at DESC",
  "limit": 10,
  "offset": 0
}
```

#### `surrealdb-create`
Create a new record. SurrealDB auto-generates a unique ID.
```json
{
  "table": "user",
  "data": { "name": "Alice", "age": 30 }
}
```

#### `surrealdb-insert`
**Two modes** detected automatically by the parameters provided:

**General insert** — insert data (object or array) into any table.
```json
{
  "table": "user",
  "data": { "name": "Bob", "age": 25 }
}
```

**Knowledge insert** — insert a structured record into the `knowledge` table.
```json
{
  "project": "my-app",
  "type": "doc",
  "title": "Architecture Overview",
  "body": "The system consists of three services…",
  "tags": ["architecture", "backend"]
}
```

#### `surrealdb-upsert`
Create or replace a record based on a unique identifier.
```json
{
  "table": "user",
  "data": { "id": "user:123", "name": "Charlie", "age": 35 }
}
```

#### `surrealdb-update`
Update existing records with MERGE semantics (partial update).
```json
{
  "table": "user",
  "data": { "age": 31 },
  "filter": "name = 'Alice'"
}
```

#### `surrealdb-delete`
Delete records from a table.
```json
{
  "table": "user",
  "filter": "age < 18"
}
```

#### `surrealdb-relate`
Create a graph edge between two records.
```json
{
  "from": "user:123",
  "edge": "purchased",
  "to": "product:456",
  "data": { "quantity": 2, "price": 29.99 }
}
```

#### `surrealdb-run`
Call a SurrealDB database function.
```json
{
  "func": "math::sin",
  "args": [1.57]
}
```

#### `surrealdb-list`
List schema objects. Scope examples: `KV`, `NS`, `DB`, `TABLE`, `TABLE user`, `SCOPE`, `INDEX`, `USER`, `TOKEN`, `PARAM`, `EVENT`, `FIELD`, `FUNCTION`, `ANALYZER`.
```json
{ "scope": "TABLE" }
```

#### `surrealdb-use`
Switch namespace and/or database context.
```json
{ "ns": "production", "db": "analytics" }
```

#### `surrealdb-info`
Get schema or engine information. Scope examples: `DB`, `TABLE user`, `KV`, `NS`, `engine` (or `version`).
```json
{ "scope": "DB" }
```

#### `surrealdb-search`
Search knowledge records by text with optional project/tag filters.
```json
{
  "query": "authentication",
  "project": "my-app",
  "tags": ["security"]
}
```

---

## Configuration

| Variable | Default | Required | Description |
|---|---|---|---|
| `SURREALDB_URL` | `http://localhost:8000` | No | SurrealDB HTTP endpoint |
| `SURREALDB_USER` | `""` (empty) | No | SurrealDB username (leave empty for `--unauthenticated`) |
| `SURREALDB_PASS` | `""` (empty) | No | SurrealDB password |
| `SURREALDB_TOKEN` | `""` (empty) | No | JWT / Bearer token (takes precedence over USER/PASS) |
| `SURREALDB_NS` | `main` | No | Namespace |
| `SURREALDB_DB` | `main` | No | Database |

**Auth priority:**
1. If `SURREALDB_TOKEN` is non-empty → Bearer token auth
2. Else if both `SURREALDB_USER` and `SURREALDB_PASS` are non-empty → Basic auth
3. Else → no auth header (compatible with `--unauthenticated` SurrealDB)

---

## Project layout

```
surrealdb-mcp/
├── src/
│   ├── main.rs      # Binary crate — JSON-RPC 2.0 loop + MCP dispatcher
│   └── lib.rs       # Library crate — SurrealDbClient, all tool methods
├── Cargo.toml
├── Cargo.lock
└── README.md
```

---

## Development

```bash
cargo build                # Debug build
cargo build --release      # Release build
RUST_LOG=debug cargo run   # Run with verbose diagnostics
cargo test                 # Run unit tests
```

The server logs diagnostics to **stderr** at the level set by `RUST_LOG` (default: `info`).

### Known issues

**SurrealDB v3.1.5 root auth is broken on macOS.** The `--user root --pass root` flags create a root user, but `/signin` always returns 401. Workarounds:
- Use SurrealDB Cloud with a JWT token (`SURREALDB_TOKEN`)
- Run SurrealDB in a Docker container with a different version
- See `docs/auth.md` for full investigation details

---

## License

MIT
