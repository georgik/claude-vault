# claude-vault Usage Guide

Complete guide for using claude-vault to archive, search, and integrate with Claude Code.

## Installation

```bash
# From source
cargo install --path .

# Or build locally
cargo build --release
```

Three binaries will be installed:
- `claude-vault` - Archive and search conversations
- `claude-vault-mcp` - MCP server for Claude Code integration
- `claude-trainer` - Export for LLM fine-tuning

## Overview

claude-vault stores Claude Code conversations in SQLite with:
- Full-text search (FTS5)
- Token counting
- Semantic search (with embeddings)
- Training data export

## Binary 1: claude-vault

### Import Conversations

```bash
# Import all conversations from ~/.claude/projects/
claude-vault import

# Import a specific file
claude-vault import-file /path/to/session.jsonl --project my-project
```

### Search Conversations

```bash
# Keyword search
claude-vault search "docker"
claude-vault search "rust AND async"

# Filter by project
claude-vault search "error" --project user/repo

# Filter by date
claude-vault search "deploy" --since 2024-01-01

# JSON output for scripts
claude-vault search "query" --json
```

### List Sessions

```bash
# List recent sessions
claude-vault list

# Filter by project
claude-vault list --project user/repo

# Show all
claude-vault list -n 0

# JSON output
claude-vault list --json
```

### Export Sessions

```bash
# Export by session ID
claude-vault export abc123

# Export most recent
claude-vault export --last

# Export as Markdown
claude-vault export abc123 --format markdown
```

### Database Management

```bash
# Show statistics
claude-vault stats

# Verify integrity
claude-vault verify

# Delete a session
claude-vault delete abc123
```

## Binary 2: claude-vault-mcp

MCP server exposes tools to Claude Code during sessions.

### Configuration

Add to `~/.claude/settings.json`:

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

The database path varies by platform:
- macOS: `~/Library/Application Support/claude-vault/vault.db`
- Linux: `~/.local/share/claude-vault/vault.db`
- Windows: `%APPDATA%\claude-vault\vault.db`

### Available Tools

Once configured, Claude can call these tools during conversations:

#### `search_vault(query, limit?, project?)`

Search past conversations using full-text search.

Example in Claude Code:
```
Claude: What did we discuss about Docker previously?
(Claude calls search_vault internally)
```

#### `get_session(session_id)`

Get full conversation by ID.

```
Claude: Show me the full conversation from session abc123.
```

#### `get_context_stats(session_id?)`

Get token/message statistics.

```
Claude: How many tokens is this conversation?
```

#### `find_similar(query, limit?, project?)`

Find messages semantically similar to query (requires embeddings).

```
Claude: Find similar discussions about error handling.
```

### Manual Testing

```bash
echo '{"jsonrpc":"2.0","method":"tools/list","id":1}' | claude-vault-mcp
echo '{"jsonrpc":"2.0","method":"tools/call","params":{"name":"search_vault","arguments":{"query":"docker"}},"id":2}' | claude-vault-mcp
```

## Binary 3: claude-trainer

### Token Counting

```bash
# Count tokens for a session
claude-trainer tokens --session abc123

# Aggregate by project
claude-trainer tokens --project user/repo

# Show top sessions by token count
claude-trainer tokens --project my-repo --top 10
```

### Export for Training

```bash
# Export in training format (JSONL)
claude-trainer export --format training --output train.jsonl

# Export specific session
claude-trainer export --session abc123 --output session.jsonl

# Export by project
claude-trainer export --project user/repo --output project.jsonl

# Filter small sessions
claude-trainer export --min-messages 5 --output train.jsonl
```

Export formats:
- `training` - JSONL for OpenAI/Anthropic fine-tuning
- `jsonl` - Single JSONL per line
- `json` - Single JSON file

### Data Cleaning

```bash
# Remove sessions with fewer than N messages
claude-trainer clean --min-messages 2

# Dry run first
claude-trainer clean --min-messages 2
```

## Integration with Claude Code

### Automatic Archiving

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

This archives conversations:
- Before `/compact` runs (captures full context)
- When sessions end (background, non-blocking)

### MCP Integration

With MCP configured, Claude can access past conversations:

```
User: What did we decide about the auth flow?

Claude: (calls search_vault("auth flow"))
Based on our previous discussions, we decided to use JWT tokens...
```

### Complete Example Configuration

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
  },
  "mcpServers": {
    "claude-vault": {
      "command": "claude-vault-mcp",
      "args": ["--db", "~/.local/share/claude-vault/vault.db"]
    }
  }
}
```

For macOS, use: `~/Library/Application Support/claude-vault/vault.db`
```

## Semantic Search (Optional)

For similarity-based search, generate embeddings:

```bash
# This feature requires embeddings to be generated first
# Future: claude-trainer embed --all
```

Once embeddings exist, `find_similar` tool becomes available in MCP.

## Database Location

Default varies by platform:
- macOS: `~/Library/Application Support/claude-vault/vault.db`
- Linux: `~/.local/share/claude-vault/vault.db`
- Windows: `%APPDATA%\claude-vault\vault.db`

Custom: `--db` flag or `CLAUDE_VAULT_DB` environment variable.

## Examples

### Find all discussions about a specific topic

```bash
claude-vault search "authentication"
```

### Export conversations for fine-tuning

```bash
claude-trainer export --format training --output train.jsonl --min-messages 3
```

### Check token usage

```bash
claude-trainer tokens --project my-app
```

### Use in Claude Code session

With MCP configured, just ask:
```
User: What have we discussed about Rust async?
```

Claude will search your vault automatically.

## Troubleshooting

### Vault database not found

Run `claude-vault import` first to create the database.

### MCP server not responding

Check `~/.claude/settings.json` syntax and binary path.

### Empty search results

Try broader queries or check with `claude-vault list` what's imported.
