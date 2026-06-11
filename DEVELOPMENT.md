# Development Guide

## Project Structure

Multi-binary Rust workspace with three CLI tools:

```
claude-vault/
├── Cargo.toml          # Workspace configuration
├── src/
│   ├── main.rs         # claude-vault CLI (archive + search)
│   ├── db.rs           # Shared SQLite operations
│   ├── import.rs       # JSONL import logic
│   ├── mcp.rs          # claude-vault-mcp (MCP server)
│   └── trainer.rs      # claude-trainer (export + tokens)
└── xtask/              # Development tasks (Rust-based testing)
    └── src/main.rs
```

## Building

```bash
# Build all binaries
cargo build

# Build release
cargo build --release

# Build specific binary
cargo build --bin claude-vault-mcp
```

## Testing

Use xtask for Rust-based testing (no shell scripts):

```bash
# Run all validation (fmt, build, test)
cargo run -p xtask -- check

# Individual tasks
cargo run -p xtask -- test
cargo run -p xtask -- fmt
cargo run -p xtask -- build
cargo run -p xtask -- test-mcp
cargo run -p xtask -- test-trainer
```

## Code Style

- Run `cargo run -p xtask -- fmt` before committing
- No emojis in code or documentation
- Use `cargo test` for unit tests
- Use xtask integration tests for binary validation

## Binaries

### claude-vault
Archive and search Claude Code conversations.

```bash
claude-vault import
claude-vault search "query"
claude-vault list
```

### claude-vault-mcp
MCP server for Claude Code integration.

Exposes tools:
- `search_vault(query)` - FTS5 search
- `get_session(id)` - Full conversation
- `get_context_stats(id?)` - Token/message stats

Configuration:
```json
{
  "mcpServers": {
    "claude-vault": {
      "command": "claude-vault-mcp",
      "args": ["--db", "~/.local/share/claude-vault/vault.db"]
    }
  }
}
```

### claude-trainer
Export for LLM fine-tuning and token counting.

```bash
claude-trainer export --format training --output train.jsonl
claude-trainer tokens --session <id>
claude-trainer tokens --project <name>
```

## Database Schema

SQLite with FTS5 full-text search:

```sql
CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    started_at TEXT,
    imported_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    uuid TEXT,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    timestamp TEXT
);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    content, content_rowid='id', content='messages',
    tokenize='porter unicode61'
);
```

## Adding Features

1. Update relevant src file
2. Add tests to same file
3. Run `cargo run -p xtask -- check`
4. Update documentation
