# SurrealDB v3.1.5 Auth Investigation

> **Status:** Root auth (`--user root --pass root`) is fundamentally broken on this macOS aarch64 installation.
> **Workaround:** Use SurrealDB Cloud with JWT token, or run SurrealDB in Docker with a different version.

## Summary

SurrealDB v3.1.5 (`surreal start --user root --pass root`) creates the root user successfully but rejects all signin attempts. This affects both HTTP `/signin` and WebSocket `/rpc` endpoints, as well as the `surreal sql` CLI.

The `--unauthenticated` mode also doesn't help — anonymous users have **zero permissions** and cannot execute any queries.

## Tested Auth Formats (all fail)

### HTTP POST /signin (Content-Type: application/json)
- `{"user":"root","pass":"root"}` → 401
- `{"username":"root","password":"root"}` → 401
- `{"SC":"root","user":"root","pass":"root"}` → 401
- `{"NS":"main","DB":"main","user":"root","pass":"root"}` → 401
- `{"ACCESS":"root","user":"root","pass":"root"}` → 401

### JSON-RPC POST /rpc
- `{"id":1,"method":"signin","params":[{"user":"root","pass":"root"}]}` → InvalidAuth
- `{"id":1,"method":"signin","params":[{"SC":"root","user":"root","pass":"root"}]}` → InvalidAuth

### CLI
- `surreal sql --user root --pass root` → "There was a problem with authentication"

## Tested Server Modes

| Mode | Result |
|------|--------|
| Default (no flags) | Some queries reach parser (parse errors), but `RETURN`/`SELECT *` → 403 |
| `--user root --pass root` | Root user created, but signin always fails |
| `--unauthenticated` | All queries → 403 (Anonymous access not allowed) |
| `--unauthenticated --allow-all` | All queries → 403 |
| `--unauthenticated --allow-guests --allow-arbitrary-query all` | All queries → 403 |
| `--import-file init.surql` (with DEFINE TABLE PERMISSIONS FOR select FULL) | Import succeeds but anonymous still 403 |

## What DOES Work

- **Default mode (no auth flags):** SurrealDB starts with no root user. Anonymous requests reach the SQL parser (parse errors return 400, not 403). But actual execution requires permissions.
- **`--import-file`:** SQL in the import file runs as superuser (creates records, defines tables). But permissions set via `DEFINE TABLE ... PERMISSIONS FOR ... FULL` don't apply to anonymous users.

## Root Cause Hypothesis

The root user is created ("Credentials were provided, and no root users were found. The root user 'root' will be created") but the stored password hash doesn't match the signin attempt. This is likely a **platform-specific bug** in SurrealDB v3.1.5's password hashing (scrypt/argon2) on macOS aarch64.

## Recommendations

1. **For local testing:** Run default mode (no auth flags). The MCP server can start and `initialize`/`tools/list` will work. CRUD operations will fail until proper auth is configured.
2. **For production:** Use SurrealDB Cloud with a `SURREALDB_TOKEN` (JWT). Set `export SURREALDB_TOKEN=your_token` and the MCP server will use Bearer auth.
3. **For Docker:** Pull a SurrealDB image from Docker Hub. The auth may work correctly in a Linux container.
4. **File a bug:** Consider reporting this to https://github.com/surrealdb/surrealdb/issues

## MCP Server Auth Modes

The MCP server now supports three auth modes, controlled exclusively by environment variables:

| Mode | Env vars | Auth header |
|------|----------|-------------|
| JWT/Bearer | `SURREALDB_TOKEN=token` | `Authorization: Bearer <token>` |
| Basic | `SURREALDB_USER=u SURREALDB_PASS=p` | `Authorization: Basic <base64>` |
| No auth | All empty | None sent |

Auth priority: Token > Basic > None
