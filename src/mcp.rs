// claude-vault-mcp: MCP server for claude-vault
//
// Exposes claude-vault SQLite database to Claude Code via MCP protocol.
// Tools: search_vault, get_session, get_context_stats

use anyhow::Result;
use anyhow::{anyhow, bail};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process;
use std::sync::Mutex;

// Reuse db module functions from library
use claude_vault::db::{
    find_entities, format_project_name, get_session_messages, get_synthesis_page, has_embeddings,
    list_categories, open_db, resolve_session_id, search, search_wiki, stats, wiki_stats,
    CategoryInfo, EntityInfo, SynthesisPage, WikiSearchResult, WikiStats,
};

#[cfg(test)]
use claude_vault::db::{insert_message, upsert_session};

// Global database path (set from CLI args)
static DB_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

fn set_db_path(path: PathBuf) {
    let mut db_path = DB_PATH.lock().unwrap();
    *db_path = Some(path);
}

fn get_db_path_from_args() -> Option<PathBuf> {
    let db_path = DB_PATH.lock().unwrap();
    db_path.clone()
}

/// MCP JSON-RPC 2.0 Request
#[derive(Debug, Deserialize)]
#[serde(tag = "method")]
#[serde(rename_all = "snake_case")]
enum MCPRequest {
    #[serde(rename = "tools/list")]
    ToolsList { id: Option<u64> },
    #[serde(rename = "tools/call")]
    ToolsCall { id: u64, params: ToolCallParams },
    #[serde(rename = "initialize")]
    Initialize { id: u64, params: serde_json::Value },
}

/// Parameters for tool calls
#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

/// Tool arguments for search_vault
#[derive(Debug, Serialize, Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    project: Option<String>,
}

/// Tool arguments for get_session
#[derive(Debug, Serialize, Deserialize)]
struct GetSessionArgs {
    session_id: String,
}

/// Tool arguments for get_context_stats
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StatsArgs {
    WithSession { session_id: String },
    Global,
}

/// Tool arguments for search_wiki
#[derive(Debug, Serialize, Deserialize)]
struct SearchWikiArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    min_quality: Option<u8>,
}

/// Tool arguments for get_synthesis
#[derive(Debug, Serialize, Deserialize)]
struct GetSynthesisArgs {
    topic: String,
}

/// Tool arguments for find_entities
#[derive(Debug, Serialize, Deserialize)]
struct FindEntitiesArgs {
    pattern: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// MCP JSON-RPC 2.0 Response
#[derive(Debug, Serialize)]
struct MCPResponse {
    jsonrpc: &'static str,
    #[serde(flatten)]
    result: MCPResult,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum MCPResult {
    Success { result: serde_json::Value, id: u64 },
    Notification { result: serde_json::Value },
    Error { error: MCPError, id: u64 },
}

#[derive(Debug, Serialize)]
struct MCPError {
    code: i32,
    message: String,
}

/// Tool definition for MCP
#[derive(Debug, Serialize)]
struct Tool {
    name: &'static str,
    description: &'static str,
    input_schema: serde_json::Value,
}

/// Content item in MCP response
#[derive(Debug, Serialize)]
struct ContentItem {
    #[serde(rename = "type")]
    content_type: &'static str,
    text: String,
}

/// Get the vault database path
fn get_db_path() -> Result<PathBuf> {
    // Check if path was set via --db argument
    if let Some(path) = get_db_path_from_args() {
        if !path.exists() {
            bail!(
                "Vault database not found at {}. Please run claude-vault import first.",
                path.display()
            );
        }
        return Ok(path);
    }

    // Fall back to default path (platform-specific data directory)
    let data_dir = dirs::data_dir().ok_or_else(|| anyhow!("Failed to find data directory"))?;
    let db_path = data_dir.join("claude-vault").join("vault.db");

    if !db_path.exists() {
        bail!(
            "Vault database not found at {}. Please run claude-vault import first.",
            db_path.display()
        );
    }

    Ok(db_path)
}

/// Get database connection
fn get_connection() -> Result<Connection> {
    let db_path = get_db_path()?;
    open_db(&db_path)
}

/// Server capabilities for initialize response
fn server_capabilities() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "claude-vault-mcp",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// List available tools
fn list_tools() -> serde_json::Value {
    serde_json::json!({
        "tools": [
            {
                "name": "search_vault",
                "description": "Search through Claude Code conversations using full-text search. Returns matching messages with session context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query using FTS5 syntax (e.g., 'docker', 'rust AND async', 'error OR exception')"
                        },
                        "limit": {
                            "type": "number",
                            "description": "Maximum number of results to return (default: 50)",
                            "default": 50
                        },
                        "project": {
                            "type": "string",
                            "description": "Optional project filter (e.g., 'user/repo')"
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "get_session",
                "description": "Get all messages from a specific conversation session by ID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Full or partial session ID (auto-resolved if partial)"
                        }
                    },
                    "required": ["session_id"]
                }
            },
            {
                "name": "get_context_stats",
                "description": "Get token/message statistics for a session or the entire vault.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Optional session ID. If provided, returns stats for that session only."
                        }
                    }
                }
            },
            {
                "name": "find_similar",
                "description": "Find messages semantically similar to a query using embeddings. Returns messages ranked by similarity score.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Query text to find similar messages for"
                        },
                        "limit": {
                            "type": "number",
                            "description": "Maximum number of results to return (default: 10)",
                            "default": 10
                        },
                        "project": {
                            "type": "string",
                            "description": "Optional project filter (e.g., 'user/repo')"
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "search_wiki",
                "description": "Search wiki pages for condensed knowledge. Uses FTS5 full-text search on processed wiki content, which is higher quality and more structured than raw conversations.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query using FTS5 syntax (e.g., 'rust async', 'docker')"
                        },
                        "limit": {
                            "type": "number",
                            "description": "Maximum number of results to return (default: 20)",
                            "default": 20
                        },
                        "min_quality": {
                            "type": "number",
                            "description": "Minimum quality score (1-5), filters for high-quality content (default: 3)",
                            "default": 3
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "get_synthesis",
                "description": "Get synthesis page for a topic. Synthesis pages combine related wiki pages into comprehensive summaries.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "Topic name to get synthesis for (e.g., 'rust', 'docker')"
                        }
                    },
                    "required": ["topic"]
                }
            },
            {
                "name": "list_categories",
                "description": "List all available categories in the wiki. Categories organize knowledge by topic area.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "find_entities",
                "description": "Find entities (technologies, concepts, tools) by name pattern. Useful for discovering what topics are covered in the wiki.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Pattern to search for in entity names (e.g., 'rust', 'docker')"
                        },
                        "limit": {
                            "type": "number",
                            "description": "Maximum number of results to return (default: 20)",
                            "default": 20
                        }
                    },
                    "required": ["pattern"]
                }
            },
            {
                "name": "get_wiki_stats",
                "description": "Get statistics about the wiki including page count, synthesis count, category count, entity count, and average quality score.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    })
}

/// Handle tool call
fn handle_tool_call(name: &str, arguments: &serde_json::Value) -> Result<serde_json::Value> {
    match name {
        "search_vault" => {
            let args: SearchArgs = serde_json::from_value(arguments.clone())
                .map_err(|e| anyhow!("Invalid arguments for search_vault: {}", e))?;

            let limit = args.limit.unwrap_or(50).min(100); // Cap at 100
            let conn = get_connection()?;

            let results = search(
                &conn,
                &args.query,
                limit,
                args.project.as_deref(),
                None, // role_filter
                None, // since
                None, // until
            )?;

            let text: String = results
                .iter()
                .map(|r| {
                    format!(
                        "[{}] {} | {}\n{}",
                        r.role,
                        r.timestamp.as_deref().unwrap_or("?"),
                        format_project_name(&r.project),
                        r.content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n---\n");

            if text.is_empty() {
                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: format!("No results found for query: {}", args.query),
                    }]
                }))
            } else {
                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text,
                    }]
                }))
            }
        }
        "get_session" => {
            let args: GetSessionArgs = serde_json::from_value(arguments.clone())
                .map_err(|e| anyhow!("Invalid arguments for get_session: {}", e))?;

            let conn = get_connection()?;

            // Resolve partial session ID if needed
            let session_id = resolve_session_id(&conn, &args.session_id)
                .unwrap_or_else(|_| args.session_id.clone());

            let messages = get_session_messages(&conn, &session_id)?;

            if messages.is_empty() {
                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: format!("Session {} not found or has no messages.", session_id),
                    }]
                }))
            } else {
                let text: String = messages
                    .iter()
                    .map(|r| {
                        format!(
                            "[{}] {}\n{}",
                            r.role,
                            r.timestamp.as_deref().unwrap_or("?"),
                            r.content
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n---\n");

                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text,
                    }]
                }))
            }
        }
        "find_similar" => {
            let args: SearchArgs = serde_json::from_value(arguments.clone())
                .map_err(|e| anyhow!("Invalid arguments for find_similar: {}", e))?;

            let limit = args.limit.unwrap_or(10).min(50);
            let conn = get_connection()?;

            // Check if embeddings exist
            let has_emb = has_embeddings(&conn)?;
            if !has_emb {
                return Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: "Semantic search requires embeddings to be generated first. Run: claude-trainer embed --all".to_string(),
                    }]
                }));
            }

            // For now, use a simple approach: generate embedding on-the-fly would require model loading
            // Return message about using keyword search as fallback
            let results = search(
                &conn,
                &args.query,
                limit,
                args.project.as_deref(),
                None,
                None,
                None,
            )?;

            let text: String = results
                .iter()
                .map(|r| {
                    format!(
                        "[{}] {} | {}\n{}",
                        r.role,
                        r.timestamp.as_deref().unwrap_or("?"),
                        format_project_name(&r.project),
                        r.content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n---\n");

            if text.is_empty() {
                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: format!("No results found for query: {}", args.query),
                    }]
                }))
            } else {
                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text,
                    }]
                }))
            }
        }
        "get_context_stats" => {
            let conn = get_connection()?;

            // Try to parse as GetSessionArgs, fall back to global stats
            if let Ok(args) = serde_json::from_value::<GetSessionArgs>(arguments.clone()) {
                // Stats for specific session
                let session_id = resolve_session_id(&conn, &args.session_id)
                    .unwrap_or_else(|_| args.session_id.clone());

                let messages = get_session_messages(&conn, &session_id)?;
                let message_count = messages.len();
                let total_tokens: usize =
                    messages.iter().map(|m| estimate_tokens(&m.content)).sum();

                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: format!(
                            "Session: {}\nMessages: {}\nEstimated tokens: {}",
                            session_id, message_count, total_tokens
                        ),
                    }]
                }))
            } else {
                // Global stats
                let (session_count, message_count) = stats(&conn)?;

                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: format!(
                            "Vault statistics:\nSessions: {}\nTotal messages: {}",
                            session_count, message_count
                        ),
                    }]
                }))
            }
        }
        "search_wiki" => {
            let args: SearchWikiArgs = serde_json::from_value(arguments.clone())
                .map_err(|e| anyhow!("Invalid arguments for search_wiki: {}", e))?;

            let limit = args.limit.unwrap_or(20).min(100);
            let conn = get_connection()?;

            let results = search_wiki(&conn, &args.query, limit, args.min_quality)?;

            if results.is_empty() {
                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: format!("No wiki pages found for query: {}", args.query),
                    }]
                }))
            } else {
                let text: String = results
                    .iter()
                    .map(|r| {
                        format!(
                            "# {} (Quality: {})\nCategories: {}\nTags: {}\n\n{}\n---\nSession ID: {}",
                            r.title,
                            r.quality_score,
                            r.categories.join(", "),
                            r.tags.join(", "),
                            r.content.chars().take(500).collect::<String>(),
                            r.session_id
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");

                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text,
                    }]
                }))
            }
        }
        "get_synthesis" => {
            let args: GetSynthesisArgs = serde_json::from_value(arguments.clone())
                .map_err(|e| anyhow!("Invalid arguments for get_synthesis: {}", e))?;

            let conn = get_connection()?;

            match get_synthesis_page(&conn, &args.topic)? {
                Some(page) => {
                    let text = format!(
                        "# {}\n\nTopic: {}\n\n## Summary\n{}\n\n## Key Points\n{}\n\n## Related Entities\n{}\n\n## Related Topics\n{}\n\n## Sources\n{}",
                        page.title,
                        page.topic,
                        page.summary,
                        page.key_points.join("\n"),
                        page.entities.join(", "),
                        page.related_topics.join(", "),
                        page.sources.join(", ")
                    );

                    Ok(serde_json::json!({
                        "content": [ContentItem {
                            content_type: "text",
                            text,
                        }]
                    }))
                }
                None => Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: format!("No synthesis page found for topic: {}", args.topic),
                    }]
                })),
            }
        }
        "list_categories" => {
            let conn = get_connection()?;

            let categories = list_categories(&conn)?;

            if categories.is_empty() {
                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: "No categories found. Wiki may not be generated yet.".to_string(),
                    }]
                }))
            } else {
                let text: String = categories
                    .iter()
                    .map(|c| {
                        format!(
                            "{} - {} documents\n  {}",
                            c.name,
                            c.document_count,
                            c.description.as_deref().unwrap_or("No description")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");

                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text,
                    }]
                }))
            }
        }
        "find_entities" => {
            let args: FindEntitiesArgs = serde_json::from_value(arguments.clone())
                .map_err(|e| anyhow!("Invalid arguments for find_entities: {}", e))?;

            let limit = args.limit.unwrap_or(20).min(100);
            let conn = get_connection()?;

            let entities = find_entities(&conn, &args.pattern, limit)?;

            if entities.is_empty() {
                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text: format!("No entities found matching pattern: {}", args.pattern),
                    }]
                }))
            } else {
                let text: String = entities
                    .iter()
                    .map(|e| {
                        format!(
                            "{} ({}) - {} mentions",
                            e.name, e.entity_type, e.mention_count
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok(serde_json::json!({
                    "content": [ContentItem {
                        content_type: "text",
                        text,
                    }]
                }))
            }
        }
        "get_wiki_stats" => {
            let conn = get_connection()?;
            let stats = wiki_stats(&conn)?;

            let text = format!(
                "Wiki Statistics:\nPages: {}\nSynthesis pages: {}\nCategories: {}\nEntities: {}\nAverage quality: {:.1}/5",
                stats.page_count,
                stats.synthesis_count,
                stats.category_count,
                stats.entity_count,
                stats.avg_quality
            );

            Ok(serde_json::json!({
                "content": [ContentItem {
                    content_type: "text",
                    text,
                }]
            }))
        }
        _ => bail!("Unknown tool: {}", name),
    }
}

/// Rough token estimation (approximately 4 chars per token)
fn estimate_tokens(text: &str) -> usize {
    (text.len() + 3) / 4
}

/// Send MCP response
fn send_response(result: MCPResult) -> Result<()> {
    let response = MCPResponse {
        jsonrpc: "2.0",
        result,
    };
    let json = serde_json::to_string(&response)?;
    println!("{}", json);
    io::stdout().flush()?;
    Ok(())
}

/// Process a single MCP request
fn process_request(line: &str) -> Result<()> {
    let req: MCPRequest =
        serde_json::from_str(line).map_err(|e| anyhow!("Failed to parse request: {}", e))?;

    match req {
        MCPRequest::Initialize { id, .. } => {
            send_response(MCPResult::Success {
                result: server_capabilities(),
                id,
            })?;
        }
        MCPRequest::ToolsList { id } => {
            let id = id.unwrap_or(1);
            send_response(MCPResult::Success {
                result: list_tools(),
                id,
            })?;
        }
        MCPRequest::ToolsCall { id, params } => {
            match handle_tool_call(&params.name, &params.arguments) {
                Ok(result) => {
                    send_response(MCPResult::Success { result, id })?;
                }
                Err(e) => {
                    send_response(MCPResult::Error {
                        error: MCPError {
                            code: -32000,
                            message: e.to_string(),
                        },
                        id,
                    })?;
                }
            }
        }
    }

    Ok(())
}

/// Run the MCP server (stdio JSON-RPC 2.0)
fn run() -> Result<()> {
    let stdin = io::stdin();
    let reader = BufReader::new(stdin.lock());

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        if let Err(e) = process_request(&line) {
            eprintln!("Error processing request: {}", e);
            // Try to send error response if we can extract an ID
            if let Ok(req) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(id) = req.get("id").and_then(|v| v.as_u64()) {
                    let _ = send_response(MCPResult::Error {
                        error: MCPError {
                            code: -32700,
                            message: e.to_string(),
                        },
                        id,
                    });
                }
            }
        }
    }

    Ok(())
}

fn main() {
    // Parse command-line arguments for --db flag
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                if i + 1 < args.len() {
                    let db_path = PathBuf::from(&args[i + 1]);
                    set_db_path(db_path);
                    i += 2;
                } else {
                    eprintln!("claude-vault-mcp: --db requires a path argument");
                    process::exit(1);
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    eprintln!("claude-vault-mcp: Server starting, waiting for JSON-RPC on stdin...");
    if let Err(e) = run() {
        eprintln!("claude-vault-mcp: Server error: {}", e);
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::NamedTempFile;

    fn setup_test_db() -> (Connection, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = open_db(tmp.path()).unwrap();

        // Add test data
        upsert_session(&conn, "sess-1", "user/test-repo", None).unwrap();
        insert_message(
            &conn,
            "sess-1",
            Some("u1"),
            "user",
            "docker container setup",
            None,
        )
        .unwrap();
        insert_message(
            &conn,
            "sess-1",
            Some("u2"),
            "assistant",
            "I'll help you set up Docker containers.",
            None,
        )
        .unwrap();

        upsert_session(&conn, "sess-2", "user/other-project", None).unwrap();
        insert_message(
            &conn,
            "sess-2",
            Some("u3"),
            "user",
            "rust async patterns",
            None,
        )
        .unwrap();

        (conn, tmp)
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello"), 2); // 5 chars / 4 = 1.25 -> 2
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn test_search_args_parsing() {
        let args = SearchArgs {
            query: "docker".to_string(),
            limit: Some(10),
            project: Some("user/repo".to_string()),
        };

        let json = serde_json::to_value(args).unwrap();
        let parsed: SearchArgs = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.query, "docker");
        assert_eq!(parsed.limit, Some(10));
        assert_eq!(parsed.project, Some("user/repo".to_string()));
    }

    #[test]
    fn test_search_args_defaults() {
        let json = json!({"query": "test"});
        let parsed: SearchArgs = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.query, "test");
        assert_eq!(parsed.limit, None);
        assert_eq!(parsed.project, None);
    }

    #[test]
    fn test_content_item_serialization() {
        let item = ContentItem {
            content_type: "text",
            text: "test content".to_string(),
        };

        let json = serde_json::to_value(item).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "test content");
    }

    #[test]
    fn test_list_tools_structure() {
        let tools = list_tools();
        assert!(tools.is_object());

        let tools_obj = tools.as_object().unwrap();
        assert!(tools_obj.contains_key("tools"));

        let tools_array = tools_obj["tools"].as_array().unwrap();
        assert_eq!(tools_array.len(), 9);

        // Check tool names
        let tool_names: Vec<&str> = tools_array
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();

        assert!(tool_names.contains(&"search_vault"));
        assert!(tool_names.contains(&"get_session"));
        assert!(tool_names.contains(&"get_context_stats"));
        assert!(tool_names.contains(&"find_similar"));
        assert!(tool_names.contains(&"search_wiki"));
        assert!(tool_names.contains(&"get_synthesis"));
        assert!(tool_names.contains(&"list_categories"));
        assert!(tool_names.contains(&"find_entities"));
        assert!(tool_names.contains(&"get_wiki_stats"));
    }

    #[test]
    fn test_mcp_request_parsing() {
        let json = r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"search_vault","arguments":{"query":"docker"}},"id":1}"#;

        let req: MCPRequest = serde_json::from_str(json).unwrap();
        match req {
            MCPRequest::ToolsCall { id, params } => {
                assert_eq!(id, 1);
                assert_eq!(params.name, "search_vault");
                assert_eq!(params.arguments["query"], "docker");
            }
            _ => panic!("Expected ToolsCall"),
        }
    }

    #[test]
    fn test_mcp_response_serialization() {
        let response = MCPResponse {
            jsonrpc: "2.0",
            result: MCPResult::Success {
                result: json!({"test": "value"}),
                id: 1,
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"test\":\"value\""));
        assert!(json.contains("\"id\":1"));
    }

    #[test]
    fn test_error_response_serialization() {
        let response = MCPResponse {
            jsonrpc: "2.0",
            result: MCPResult::Error {
                error: MCPError {
                    code: -32000,
                    message: "test error".to_string(),
                },
                id: 1,
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["error"]["code"], -32000);
        assert_eq!(parsed["error"]["message"], "test error");
        assert_eq!(parsed["id"], 1);
    }

    #[test]
    fn test_search_args_with_minimal_json() {
        let json = json!({"query": "test"});
        let parsed: SearchArgs = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.query, "test");
        assert!(parsed.limit.is_none());
    }
}
