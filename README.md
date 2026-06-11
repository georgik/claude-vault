# claude-vault

[![CI](https://github.com/kuroko1t/claude-vault/actions/workflows/ci.yml/badge.svg)](https://github.com/kuroko1t/claude-vault/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/claude-vault.svg)](https://crates.io/crates/claude-vault)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Archive your Claude Code conversations into a searchable SQLite database. Single binary, zero dependencies.

## Why?

Claude Code stores session history as JSONL files under `~/.claude/projects/`, but these files have problems:

- **Files are deleted by default** — Old sessions are auto-deleted over time (`cleanupPeriodDays` setting). Even if you change this, the other issues remain.
- **Data loss on compact** — `/compact` compresses in-memory context, and the original conversation details are lost
- **Poor searchability** — JSONL files are scattered across directories with no cross-session search

claude-vault copies conversations into a durable SQLite database with full-text search — once archived, your history survives file deletion, compaction, and cleanup. Zero runtime dependencies, single binary.

## Demo

### List sessions

```
$ claude-vault list -n 5
ID         DATE                    MSGS  PROJECT                   PREVIEW
----------------------------------------------------------------------------------------------------
47cf1f2e   2026-03-13T14:21:48      555  user/my-project           How to auto-archive without manual steps?
a4d5aa81   2026-03-13T03:01:35      232  user/another-repo         Prepare README.md for OSS publishing
6b4b6b21   2026-03-13T00:40:44       81  user/experiment           Search for open-source tools and compare features
67e36ae8   2026-03-12T22:59:44      465  user/trading-bot          Implement daily strategy with backtesting
cd9851d6   2026-03-04T04:12:25      312  org/ml-pipeline           Fix Docker build for GPU training container

Export: claude-vault export <ID>
```

### Search conversations

```
$ claude-vault search "Docker"
[user] user/my-project | 84d8d116 | 2026-02-04T01:15:53Z
docker info | grep -A5 "Build"
  buildx: Docker Buildx (Docker Inc.)
    Version:  v0.28.0
---
[assistant] user/trading-bot | c86a3eec | 2026-02-09T23:40:43Z
The session is running inside Docker, so docker commands are not available.
Run the following on the host:
  docker compose -f docker-compose.safe.yml build --no-cache
```

### Database stats

```
$ claude-vault stats
Database: /home/user/.local/share/claude-vault/vault.db
Sessions: 178
Messages: 94562
```

## Features

- **FTS5 full-text search** with Porter stemming (e.g. "running" matches "run")
- **Automatic archiving** via Claude Code hooks (PreCompact + SessionEnd)
- **Noise filtering** — strips tool results, system tags, and meta messages
- **UUID deduplication** — safe to re-import; duplicates are skipped
- **Session export** — Markdown, JSON, or plain text
- **Single binary** — no Python, Node.js, or other runtime required

## Install

### From GitHub Releases (recommended)

Download a prebuilt binary from [Releases](https://github.com/kuroko1t/claude-vault/releases):

```bash
# Linux x86_64
curl -fsSL https://github.com/kuroko1t/claude-vault/releases/latest/download/claude-vault-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv claude-vault /usr/local/bin/

# macOS Apple Silicon
curl -fsSL https://github.com/kuroko1t/claude-vault/releases/latest/download/claude-vault-aarch64-apple-darwin.tar.gz | tar xz
sudo mv claude-vault /usr/local/bin/
```

### From crates.io

```bash
cargo install claude-vault
```

### From source

```bash
cargo install --path .
```

## Quick Start

1. Import all existing conversations from `~/.claude/projects/`:

```bash
claude-vault import
# Imported 94562 messages (0 skipped, 12847 filtered, 0 errors) from 203 files
```

2. Search your history:

```bash
claude-vault search "error handling"
```

3. Browse sessions:

```bash
claude-vault list
```

4. (Optional) Set up auto-archiving — see [Hooks Setup](#auto-archive-with-claude-code-hooks).

## Using from Claude Code

Search past conversations directly from a Claude Code session:

```bash
# Keyword search
claude-vault search "previous Docker configuration"

# Structured output for Claude to parse
claude-vault search "auth bug" --json
claude-vault list --json
```

With [auto-archiving hooks](#auto-archive-with-claude-code-hooks) configured, Claude Code can always search your full history — even for sessions whose JSONL files have been deleted.

<details>
<summary><h2>Usage</h2></summary>

### import

Import all JSONL files from `~/.claude/projects/` recursively (including subagent directories):

```bash
claude-vault import
```

### import-file

Import a single JSONL session file:

```bash
claude-vault import-file /path/to/session.jsonl --project my-project
```

### search

```bash
claude-vault search "Docker"
claude-vault search "deploy" --project my-app
claude-vault search "deploy" --since 2024-01-01 --until 2024-06-30
claude-vault search '"error handling" AND rust'   # FTS5 syntax
claude-vault search "auth bug" --json              # machine-readable output
```

### export

Export a session to Markdown, JSON, or plain text. Accepts session ID prefixes (like git short hashes):

```bash
claude-vault export 47cf1f2e
claude-vault export --last                          # most recent session
claude-vault export --last --format markdown > session.md
```

### list

```bash
claude-vault list
claude-vault list --project my-app --since 2024-01-01
claude-vault list --json
```

### Other commands

```bash
claude-vault delete 47cf -y                        # delete a session
claude-vault stats                                  # show database statistics
claude-vault verify                                 # check database integrity
claude-vault completions zsh > ~/.zfunc/_claude-vault  # shell completions
claude-vault --db /path/to/vault.db search "query"  # custom database path
```

</details>

<details>
<summary><h2>Wiki Generation</h2></summary>

claude-vault can transform conversations into a structured knowledge base with quality scoring, categorization, and entity extraction.

### Quick Start

Generate wiki from all conversations:

```bash
# Run full pipeline (score + categorize + wiki + entities + synthesis)
claude-vault pipeline

# Generate wiki pages only
claude-vault wiki --all

# Export entire wiki to markdown files
claude-vault wiki --export-all ~/wiki-export
```

### Wiki Commands

```bash
# Generate wiki for specific session
claude-vault wiki --session 47cf1f2e --export page.md

# Generate all wiki pages
claude-vault wiki --all

# Export all wiki pages to directory
claude-vault wiki --export-all ~/wiki-export
claude-vault wiki --export-all ~/wiki-export --min 4    # only high quality

# Show top wiki pages by quality
claude-vault wiki --top
```

### Export Directory Structure

The `--export-all` option creates organized markdown files:

```
~/wiki-export/
├── technology/
│   ├── rust_async_error_handling.md
│   └── docker_deployment_guide.md
├── development/
│   ├── testing_strategy.md
│   └── code_review_checklist.md
├── architecture/
│   └── microservices_design.md
└── general/
    └── project_overview.md
```

Each file includes YAML frontmatter with metadata:

```yaml
---
title: Rust Async Error Handling
categories:
- Technology
- Rust Async
tags:
- rust
- async
- error-handling
quality_score: 5
created_at: 2026-06-10T19:59:34.962Z
session_id: 65babf27-d613-4b3f-a7fa-490ac327a29a
content_type: conversation
---
```

### Full Pipeline

The full pipeline processes conversations through multiple stages:

```bash
claude-vault pipeline
# or with options:
claude-vault pipeline --min-quality 4 --no-dedup --no-synthesis
```

Pipeline stages:
1. **Categorization** — Auto-discovers categories from content
2. **Quality Scoring** — Rates conversations 1-5 by technical depth, actionability, explanation quality
3. **Wiki Generation** — Creates structured pages from high-quality sessions
4. **Entity Extraction** — Identifies technologies, concepts, and patterns
5. **Synthesis** — Generates summary pages combining related conversations
6. **Lint** — Deduplicates pages and fixes formatting issues

### Quality Scoring

Conversations are scored on three dimensions:

| Metric | Criteria |
|--------|----------|
| Technical Depth | Algorithms, optimization, patterns, async, concurrency, architecture |
| Actionability | Step-by-step guidance, code examples, complete solutions |
| Explanation Quality | Detail length, structure, reasoning indicators |

### Other Wiki Features

```bash
# Score conversations without generating wiki
claude-vault score --all
claude-vault score --session 47cf1f2e

# Categorize conversations
claude-vault categorize --tree

# Extract entities
claude-vault entities --extract
claude-vault entities --find rust

# Generate synthesis pages
claude-vault synthesize --top
claude-vault synthesize --topic "rust"

# Lint and clean wiki
claude-vault lint --deduplicate
claude-vault lint --fix
```

</details>

## Auto-archive with Claude Code Hooks

Add to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PreCompact": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "claude-vault import >/dev/null 2>&1"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "claude-vault import >/dev/null 2>&1 &"
          }
        ]
      }
    ]
  }
}
```

| Hook | Timing | Mode |
|------|--------|------|
| **PreCompact** | Before `/compact` runs | Synchronous — captures full data before compression |
| **SessionEnd** | When a session ends | Background — non-blocking |

Once configured, conversations are archived automatically with no manual steps.

<details>
<summary><h2>How It Works</h2></summary>

```
~/.claude/projects/<project>/<session>.jsonl
        │
        ▼
    ┌─────────┐     ┌──────────┐     ┌───────────┐
    │  Parse   │────▶│  Filter  │────▶│  SQLite   │
    │  JSONL   │     │  & Clean │     │  + FTS5   │
    └─────────┘     └──────────┘     └───────────┘
```

1. **Parse** — Reads each JSONL record, extracts `user` and `assistant` messages
2. **Filter** — Removes system-injected noise (see below)
3. **Store** — Inserts into SQLite with UUID-based dedup; FTS5 index is updated via triggers

### Noise Filtering

| Category | What's removed |
|----------|---------------|
| Tool results | All `tool_result` content (Bash output, file creation messages, web fetch results, etc.) |
| System tags | `<system-reminder>`, `<local-command-caveat>`, `<local-command-stdout>`, `<command-name>`, `<command-message>`, `<command-args>` |
| Read-only tools | Read, Glob, Grep, LSP, ToolSearch, browser snapshot/navigation, TaskGet/TaskOutput/TaskList |
| Meta messages | eval-loop iterations/commands, Stop hook feedback, empty/whitespace-only content |

User text input, assistant responses, and code-modifying tool calls (Edit, Write, Bash, etc.) are preserved.

</details>

<details>
<summary><h2>Schema</h2></summary>

```sql
CREATE TABLE sessions (
    session_id  TEXT PRIMARY KEY,
    project     TEXT NOT NULL,
    started_at  TEXT,
    imported_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE messages (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(session_id),
    uuid       TEXT,
    role       TEXT NOT NULL,
    content    TEXT NOT NULL,
    timestamp  TEXT
);

CREATE UNIQUE INDEX idx_messages_uuid ON messages(uuid) WHERE uuid IS NOT NULL;

CREATE VIRTUAL TABLE messages_fts USING fts5(
    content, content_rowid='id', content='messages',
    tokenize='porter unicode61'
);
```

Default database location varies by platform:
- macOS: `~/Library/Application Support/claude-vault/vault.db`
- Linux: `~/.local/share/claude-vault/vault.db`
- Windows: `%APPDATA%\claude-vault\vault.db`

SQLite is configured with WAL mode and a 5-second busy timeout for safe concurrent access.

</details>

<details>
<summary><h2>MCP Server Integration</h2></summary>

claude-vault includes an MCP (Model Context Protocol) server that lets Claude Code search your conversation history directly.

### Installation

Build or install claude-vault, then register the MCP server with Claude Code:

```bash
# Add MCP server for local project
claude mcp add claude-vault -- claude-vault-mcp --db /path/to/vault.db

# Add for all projects (user scope)
# macOS: ~/Library/Application Support/claude-vault/vault.db
# Linux: ~/.local/share/claude-vault/vault.db
claude mcp add --scope user claude-vault -- claude-vault-mcp --db ~/.local/share/claude-vault/vault.db
```

### Available Tools

Once connected, Claude Code can use these tools:

| Tool | Description |
|------|-------------|
| `search_vault` | Full-text search through conversations using FTS5 syntax |
| `get_session` | Retrieve all messages from a specific session by ID |
| `get_context_stats` | Get token/message statistics for sessions or entire vault |
| `find_similar` | Find messages semantically similar to a query (requires embeddings) |

### Usage in Claude Code

After connecting the MCP server, ask Claude to search your history:

```
Search my vault for "rust async error handling"
What did we discuss about docker deployment?
Show me statistics for my conversation history
```

### Verify Connection

Check MCP server status:

```bash
claude mcp list
claude mcp get claude-vault
```

### Troubleshooting

If the server shows "Failed to connect":

1. **Verify database path**: Ensure the database file exists and is readable
2. **Check server binary**: `claude-vault-mcp` must be in your PATH or use full path
3. **Test manually**: `echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}' | claude-vault-mcp --db /path/to/vault.db`
4. **Increase timeout**: `MCP_TIMEOUT=60000 claude` for slower databases

### Remove Server

```bash
claude mcp remove claude-vault
```

</details>

## License

MIT
