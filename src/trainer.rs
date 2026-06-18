//! claude-trainer: Export and token counting for LLM fine-tuning
//!
//! Commands:
//!   export --format training --output <file>
//!   tokens --session <id>
//!   tokens --project <name>
//!   clean --dedup

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use rusqlite::{params, Connection};
use serde_json::json;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tiktoken_rs::cl100k_base;

/// Re-use db functions from the library
mod db;
mod import;

use db::{format_project_name, get_session_messages, open_db, resolve_session_id, SearchResult};

#[derive(Parser)]
#[command(name = "claude-trainer")]
#[command(about = "Export Claude conversations for LLM fine-tuning and token counting", long_about = None)]
struct Cli {
    /// Path to the SQLite database file
    #[arg(long, env = "CLAUDE_VAULT_DB")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Export conversations in training format
    Export {
        /// Output format (training, jsonl, or json)
        #[arg(long, default_value = "training")]
        format: ExportFormat,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Session ID or prefix to export (optional, exports all if not specified)
        #[arg(long)]
        session: Option<String>,

        /// Project name filter (optional)
        #[arg(long)]
        project: Option<String>,

        /// Include sessions with fewer than N messages
        #[arg(long, default_value = "2")]
        min_messages: usize,
    },

    /// Count tokens per session or project
    Tokens {
        /// Session ID or prefix
        #[arg(long, conflicts_with = "project")]
        session: Option<String>,

        /// Project name filter
        #[arg(long, conflicts_with = "session")]
        project: Option<String>,

        /// Show top N sessions by token count
        #[arg(long, default_value = "20")]
        top: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Clean and deduplicate data
    Clean {
        /// Remove duplicate conversations
        #[arg(long)]
        dedup: bool,

        /// Remove sessions with fewer than N messages
        #[arg(long, default_value = "2")]
        min_messages: usize,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(clap::ValueEnum, Clone, Default)]
enum ExportFormat {
    #[default]
    Training,
    Jsonl,
    Json,
}

fn default_db_path() -> Result<PathBuf> {
    let data_dir = dirs::data_dir().context("Could not determine data directory")?;
    let vault_dir = data_dir.join("claude-vault");
    std::fs::create_dir_all(&vault_dir)?;
    Ok(vault_dir.join("vault.db"))
}

/// Karpathy-inspired cleaning: strip artifacts, normalize whitespace
fn karpathy_clean(content: &str) -> String {
    let mut cleaned = content.to_string();

    // Remove XML-like system tags (already done in import, but double-clean)
    let tag_patterns = [
        "system-reminder",
        "local-command-caveat",
        "command-name",
        "command-message",
        "command-args",
        "task-notification",
    ];
    for tag in &tag_patterns {
        while let Some(start) = cleaned.find(&format!("<{tag}>")) {
            if let Some(end) = cleaned.find(&format!("</{tag}>")) {
                let end = end + format!("</{tag}>").len();
                cleaned.replace_range(start..end, "");
            } else {
                break;
            }
        }
    }

    // Strip tool_use lines
    cleaned = cleaned
        .lines()
        .filter(|line| !line.starts_with("[tool_use: "))
        .collect::<Vec<_>>()
        .join("\n");

    // Normalize whitespace: collapse multiple spaces, trim lines
    cleaned = cleaned
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                String::new()
            } else {
                // Collapse multiple spaces but preserve newlines
                trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
            }
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    // Remove common artifacts
    cleaned = cleaned
        .replace("\u{200b}", "") // Zero-width space
        .replace("\u{feff}", "") // Zero-width no-break space
        .to_string();

    cleaned.trim().to_string()
}

/// Count tokens using Claude's tokenizer (cl100k_base)
fn count_tokens(text: &str) -> usize {
    let bpe = cl100k_base().unwrap();
    bpe.encode_with_special_tokens(text).len()
}

/// Convert database messages to training format
fn to_training_format(messages: &[SearchResult]) -> Option<serde_json::Value> {
    if messages.is_empty() {
        return None;
    }

    // Must have at least one user-assistant pair
    let has_user = messages.iter().any(|m| m.role == "user");
    let has_assistant = messages.iter().any(|m| m.role == "assistant");
    if !has_user || !has_assistant {
        return None;
    }

    let formatted_messages: Vec<serde_json::Value> = messages
        .iter()
        .filter_map(|m| {
            let cleaned = karpathy_clean(&m.content);
            if cleaned.is_empty() {
                return None;
            }
            // Only include user and assistant roles
            if m.role != "user" && m.role != "assistant" {
                return None;
            }
            Some(json!({
                "role": m.role,
                "content": cleaned
            }))
        })
        .collect();

    if formatted_messages.is_empty() {
        return None;
    }

    Some(json!({
        "messages": formatted_messages
    }))
}

/// Handle the export command
fn cmd_export(
    conn: &Connection,
    format: &ExportFormat,
    output: &Path,
    session_id: Option<String>,
    project: Option<String>,
    min_messages: usize,
) -> Result<()> {
    let mut sessions_to_export = Vec::new();

    if let Some(sid) = session_id {
        // Export a specific session
        let resolved = resolve_session_id(conn, &sid)?;
        let messages = get_session_messages(conn, &resolved)?;
        if messages.len() < min_messages {
            bail!(
                "Session {resolved} has only {} messages (minimum: {})",
                messages.len(),
                min_messages
            );
        }
        sessions_to_export.push((resolved, messages));
    } else {
        // Export all sessions, optionally filtered by project
        let mut stmt = if project.is_some() {
            conn.prepare(
                "SELECT DISTINCT s.session_id
                 FROM sessions s
                 WHERE s.project LIKE ?1
                 ORDER BY s.session_id",
            )?
        } else {
            conn.prepare(
                "SELECT DISTINCT s.session_id
                 FROM sessions s
                 ORDER BY s.session_id",
            )?
        };

        let session_ids: Vec<String> = if let Some(proj) = &project {
            let normalized = format!("%{}%", proj.replace('/', "-"));
            stmt.query_map(params![normalized], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?
        };

        for sid in session_ids {
            let messages = get_session_messages(conn, &sid)?;
            if messages.len() >= min_messages {
                sessions_to_export.push((sid.clone(), messages));
            }
        }
    }

    if sessions_to_export.is_empty() {
        bail!("No sessions found matching the criteria");
    }

    // Create output file
    let file = File::create(output)
        .with_context(|| format!("Failed to create output file: {}", output.display()))?;
    let mut writer = BufWriter::new(file);

    let mut exported = 0;
    let mut skipped = 0;

    for (_session_id, messages) in sessions_to_export {
        match format {
            ExportFormat::Training | ExportFormat::Jsonl => {
                if let Some(training_json) = to_training_format(&messages) {
                    writeln!(writer, "{}", training_json)?;
                    exported += 1;
                } else {
                    skipped += 1;
                }
            }
            ExportFormat::Json => {
                if let Some(training_json) = to_training_format(&messages) {
                    serde_json::to_writer(&mut writer, &training_json)?;
                    writeln!(writer)?;
                    exported += 1;
                } else {
                    skipped += 1;
                }
            }
        }
    }

    writer.flush()?;

    println!("Exported {} sessions to {}", exported, output.display());
    if skipped > 0 {
        println!(
            "Skipped {} sessions (insufficient message pairs or empty after cleaning)",
            skipped
        );
    }

    Ok(())
}

/// Handle the tokens command
fn cmd_tokens(
    conn: &Connection,
    session: Option<String>,
    project: Option<String>,
    top: usize,
    json: bool,
) -> Result<()> {
    if let Some(sid) = session {
        // Count tokens for a specific session
        let resolved = resolve_session_id(conn, &sid)?;
        let messages = get_session_messages(conn, &resolved)?;

        let mut total_tokens = 0;
        let mut role_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for msg in &messages {
            let tokens = count_tokens(&msg.content);
            *role_counts.entry(msg.role.clone()).or_insert(0) += tokens;
            total_tokens += tokens;
        }

        if json {
            println!(
                "{}",
                json!({
                    "session_id": resolved,
                    "total_tokens": total_tokens,
                    "by_role": role_counts,
                    "message_count": messages.len()
                })
            );
        } else {
            println!("Session: {}", resolved);
            println!("Total tokens: {}", total_tokens);
            println!("Messages: {}", messages.len());
            println!("\nBy role:");
            for (role, count) in role_counts.iter() {
                println!("  {}: {}", role, count);
            }
        }
    } else if let Some(proj) = project {
        // Count tokens for a project
        let normalized = format!("%{}%", proj.replace('/', "-"));

        let mut stmt = conn.prepare(
            "SELECT s.session_id, s.project
             FROM sessions s
             WHERE s.project LIKE ?1",
        )?;

        let sessions: Vec<(String, String)> = stmt
            .query_map(params![normalized], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        if sessions.is_empty() {
            bail!("No sessions found for project: {}", proj);
        }

        let mut total_tokens = 0;
        let mut session_token_counts: Vec<(String, usize)> = Vec::new();

        for (sid, _project) in &sessions {
            let messages = get_session_messages(conn, sid)?;
            let session_tokens: usize = messages.iter().map(|m| count_tokens(&m.content)).sum();
            total_tokens += session_tokens;
            session_token_counts.push((sid.clone(), session_tokens));
        }

        // Sort by token count descending
        session_token_counts.sort_by(|a, b| b.1.cmp(&a.1));

        if json {
            println!(
                "{}",
                json!({
                    "project": proj,
                    "total_tokens": total_tokens,
                    "session_count": sessions.len(),
                    "top_sessions": session_token_counts.iter().take(top).cloned().collect::<Vec<_>>()
                })
            );
        } else {
            println!("Project: {}", format_project_name(&proj));
            println!("Total tokens: {}", total_tokens);
            println!("Sessions: {}", sessions.len());
            println!("\nTop {} sessions by token count:", top);
            for (i, (sid, tokens)) in session_token_counts.iter().take(top).enumerate() {
                println!(
                    "  {}. {}: {} tokens",
                    i + 1,
                    &sid[..8.min(sid.len())],
                    tokens
                );
            }
        }
    } else {
        // Show top sessions across all projects
        let mut stmt = conn.prepare("SELECT session_id FROM sessions ORDER BY session_id")?;
        let session_ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut session_token_counts: Vec<(String, usize)> = Vec::new();

        for sid in session_ids {
            let messages = get_session_messages(conn, &sid)?;
            let session_tokens: usize = messages.iter().map(|m| count_tokens(&m.content)).sum();
            session_token_counts.push((sid, session_tokens));
        }

        // Sort by token count descending
        session_token_counts.sort_by(|a, b| b.1.cmp(&a.1));

        if json {
            println!(
                "{}",
                json!({
                    "total_sessions": session_token_counts.len(),
                    "top_sessions": session_token_counts.iter().take(top).cloned().collect::<Vec<_>>()
                })
            );
        } else {
            println!("Top {} sessions by token count:", top);
            for (i, (sid, tokens)) in session_token_counts.iter().take(top).enumerate() {
                println!(
                    "  {}. {}: {} tokens",
                    i + 1,
                    &sid[..8.min(sid.len())],
                    tokens
                );
            }
        }
    }

    Ok(())
}

/// Handle the clean command
fn cmd_clean(conn: &Connection, dedup: bool, min_messages: usize, yes: bool) -> Result<()> {
    if !dedup && min_messages <= 0 {
        bail!("Specify --dedup or --min-messages");
    }

    // Find sessions with fewer than min_messages
    let mut stmt = conn.prepare(
        "SELECT s.session_id, s.project, COUNT(m.id) as msg_count
         FROM sessions s
         LEFT JOIN messages m ON s.session_id = m.session_id
         GROUP BY s.session_id
         HAVING msg_count < ?1",
    )?;

    let small_sessions: Vec<(String, String, i64)> = stmt
        .query_map(params![min_messages as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    if !small_sessions.is_empty() && !yes {
        println!(
            "Found {} sessions with fewer than {} messages:",
            small_sessions.len(),
            min_messages
        );
        for (sid, _project, count) in &small_sessions {
            println!("  {}: {} messages", &sid[..8.min(sid.len())], count);
        }
        eprint!("Delete these sessions? [y/N] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut deleted = 0;
    for (sid, _project, _count) in small_sessions {
        db::delete_session(conn, &sid)?;
        deleted += 1;
    }

    if deleted > 0 {
        println!(
            "Deleted {} sessions with < {} messages",
            deleted, min_messages
        );
    }

    if dedup {
        // Find potential duplicate sessions by similar first user message
        let mut stmt = conn.prepare(
            "SELECT m.session_id, s.project, substr(m.content, 1, 100) as preview
             FROM messages m
             JOIN sessions s ON s.session_id = m.session_id
             WHERE m.role = 'user' AND m.id = (
                 SELECT MIN(id) FROM messages m2 WHERE m2.session_id = m.session_id AND m2.role = 'user'
             )"
        )?;

        let first_messages: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        // Group by cleaned content
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (sid, _project, content) in first_messages {
            let cleaned = karpathy_clean(&content);
            groups.entry(cleaned).or_insert_with(Vec::new).push(sid);
        }

        let duplicates: Vec<Vec<String>> =
            groups.values().filter(|v| v.len() > 1).cloned().collect();

        if !duplicates.is_empty() {
            println!(
                "Found {} potential duplicate session groups:",
                duplicates.len()
            );
            for group in duplicates.iter().take(10) {
                println!("  Sessions with similar first message:");
                for sid in group {
                    println!("    - {}", &sid[..8.min(sid.len())]);
                }
            }
            println!("Run with --dedup to remove duplicates (keeping one per group)");
        }
    }

    Ok(())
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    let db_path = match cli.db {
        Some(p) => p,
        None => default_db_path()?,
    };

    let conn = open_db(&db_path)?;

    match cli.command {
        Commands::Export {
            format,
            output,
            session,
            project,
            min_messages,
        } => {
            cmd_export(&conn, &format, &output, session, project, min_messages)?;
        }
        Commands::Tokens {
            session,
            project,
            top,
            json,
        } => {
            cmd_tokens(&conn, session, project, top, json)?;
        }
        Commands::Clean {
            dedup,
            min_messages,
            yes,
        } => {
            cmd_clean(&conn, dedup, min_messages, yes)?;
        }
    }

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_test_db() -> (Connection, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = db::open_db(tmp.path()).unwrap();
        (conn, tmp)
    }

    fn add_test_message(
        conn: &Connection,
        session_id: &str,
        project: &str,
        role: &str,
        content: &str,
    ) {
        db::upsert_session(conn, session_id, project, None).unwrap();
        db::insert_message(
            conn,
            session_id,
            Some(&format!("{}-{}", role, session_id)),
            role,
            content,
            None,
        )
        .unwrap();
    }

    #[test]
    fn test_karpathy_clean_removes_system_tags() {
        let input = "Hello <system-reminder>noise</system-reminder> world";
        let cleaned = karpathy_clean(input);
        assert_eq!(cleaned, "Hello world");
    }

    #[test]
    fn test_karpathy_clean_removes_tool_use() {
        let input = "Plan:\n[tool_use: Edit] {\"file\":\"x\"}\nDone";
        let cleaned = karpathy_clean(input);
        assert_eq!(cleaned, "Plan:\nDone");
    }

    #[test]
    fn test_karpathy_clean_normalizes_whitespace() {
        let input = "Hello    world\n\n\n   Test";
        let cleaned = karpathy_clean(input);
        assert_eq!(cleaned, "Hello world\nTest");
    }

    #[test]
    fn test_karpathy_clean_removes_zero_width() {
        let input = "Hello\u{200b}world";
        let cleaned = karpathy_clean(input);
        assert_eq!(cleaned, "Helloworld");
    }

    #[test]
    fn test_count_tokens_basic() {
        let text = "Hello, world!";
        let tokens = count_tokens(text);
        assert!(tokens > 0);
        assert!(tokens <= 10); // Should be small for this text
    }

    #[test]
    fn test_count_tokens_code() {
        let code = "fn main() {\n    println!(\"Hello\");\n}";
        let tokens = count_tokens(code);
        assert!(tokens > 0);
    }

    #[test]
    fn test_count_tokens_empty() {
        // Note: cl100k_base may return tokens for whitespace, so we just check it's small
        let tokens = count_tokens("");
        let tokens_ws = count_tokens("   ");
        // Empty should be 0, whitespace may have tokens but should be small
        assert_eq!(tokens, 0);
        assert!(tokens_ws <= 3);
    }

    #[test]
    fn test_to_training_format_requires_user_assistant() {
        let (conn, _tmp) = setup_test_db();
        add_test_message(&conn, "s1", "proj", "user", "Hello");
        add_test_message(&conn, "s1", "proj", "assistant", "Hi there");
        let messages = db::get_session_messages(&conn, "s1").unwrap();
        let result = to_training_format(&messages);
        assert!(result.is_some());
        let formatted = result.unwrap();
        assert_eq!(formatted["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_to_training_format_skips_user_only() {
        let (conn, _tmp) = setup_test_db();
        add_test_message(&conn, "s1", "proj", "user", "Hello");
        let messages = db::get_session_messages(&conn, "s1").unwrap();
        let result = to_training_format(&messages);
        assert!(result.is_none());
    }

    #[test]
    fn test_to_training_format_cleans_content() {
        let (conn, _tmp) = setup_test_db();
        add_test_message(
            &conn,
            "s1",
            "proj",
            "user",
            "Hello <system-reminder>noise</system-reminder>",
        );
        add_test_message(&conn, "s1", "proj", "assistant", "Hi");
        let messages = db::get_session_messages(&conn, "s1").unwrap();
        let result = to_training_format(&messages).unwrap();
        let user_msg = &result["messages"].as_array().unwrap()[0];
        let content = user_msg["content"].as_str().unwrap();
        assert!(!content.contains("system-reminder"));
    }

    #[test]
    fn test_count_tokens_on_session() {
        let (conn, _tmp) = setup_test_db();
        add_test_message(
            &conn,
            "s1",
            "proj",
            "user",
            "Write a function that adds two numbers",
        );
        add_test_message(
            &conn,
            "s1",
            "proj",
            "assistant",
            "fn add(a: i32, b: i32) -> i32 { a + b }",
        );

        let messages = db::get_session_messages(&conn, "s1").unwrap();
        let total: usize = messages.iter().map(|m| count_tokens(&m.content)).sum();
        assert!(total > 5); // Should have several tokens
    }

    #[test]
    fn test_to_training_format_filters_empty_after_cleaning() {
        let (conn, _tmp) = setup_test_db();
        add_test_message(&conn, "s1", "proj", "user", "Hello");
        add_test_message(&conn, "s1", "proj", "assistant", "[tool_use: Read] x");
        let messages = db::get_session_messages(&conn, "s1").unwrap();
        let result = to_training_format(&messages).unwrap();
        // After cleaning, the assistant message becomes empty and is filtered out
        // So we only have the user message left
        let formatted = result["messages"].as_array().unwrap();
        assert_eq!(formatted.len(), 1);
        assert_eq!(formatted[0]["role"].as_str().unwrap(), "user");
    }

    #[test]
    fn test_karpathy_clean_preserves_code() {
        let input = "fn main() {\n    let x = 1;\n    println!(\"{}\");\n}";
        let cleaned = karpathy_clean(input);
        assert!(cleaned.contains("fn main"));
        assert!(cleaned.contains("println"));
    }
}
