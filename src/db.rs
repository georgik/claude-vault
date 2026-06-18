#![allow(dead_code)]

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Convert project directory names like "-home-murou-ghq-github-com-user-repo" to "user/repo"
pub fn format_project_name(raw: &str) -> String {
    let parts: Vec<&str> = raw.trim_start_matches('-').split('-').collect();

    // Try to find github-com pattern and extract user/repo
    for (i, part) in parts.iter().enumerate() {
        if *part == "github" && parts.get(i + 1) == Some(&"com") && i + 3 < parts.len() {
            let user = parts[i + 2];
            let repo = parts[i + 3..].join("-");
            if let Some(pos) = repo.find("--worktrees") {
                return format!("{user}/{}", &repo[..pos]);
            }
            return format!("{user}/{repo}");
        }
    }

    // Fallback: take last meaningful segments
    let meaningful: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if meaningful.len() > 2 {
        meaningful[meaningful.len() - 2..].join("/")
    } else {
        raw.to_string()
    }
}

pub fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("Failed to open database: {}", path.display()))?;

    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         PRAGMA foreign_keys=ON;",
    )?;

    init_schema(&conn)?;

    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            project    TEXT NOT NULL,
            started_at TEXT,
            imported_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS messages (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            uuid       TEXT,
            role       TEXT NOT NULL,
            content    TEXT NOT NULL,
            timestamp  TEXT,
            FOREIGN KEY (session_id) REFERENCES sessions(session_id)
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_uuid
            ON messages(uuid) WHERE uuid IS NOT NULL;
        ",
    )?;

    // Create FTS table with Porter stemming, or migrate from old schema
    migrate_fts(conn)?;

    // Create embeddings table for semantic search
    init_embeddings(conn)?;

    // Create wiki pages table with FTS5
    init_wiki_tables(conn)?;

    // Create category tables for wiki generation
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS categories (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            parent_id INTEGER,
            description TEXT,
            document_count INTEGER DEFAULT 0,
            FOREIGN KEY (parent_id) REFERENCES categories(id)
        );

        CREATE INDEX IF NOT EXISTS idx_categories_parent ON categories(parent_id);

        CREATE TABLE IF NOT EXISTS message_categories (
            message_id INTEGER,
            category_id INTEGER,
            confidence REAL,
            PRIMARY KEY (message_id, category_id),
            FOREIGN KEY (message_id) REFERENCES messages(id),
            FOREIGN KEY (category_id) REFERENCES categories(id)
        );

        CREATE INDEX IF NOT EXISTS idx_msgcat_category ON message_categories(category_id);
        ",
    )?;

    conn.execute_batch(
        "
        CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
            INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
        END;

        CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
        END;

        CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
            INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
        END;
        ",
    )?;
    Ok(())
}

/// Ensure the FTS table uses Porter stemming tokenizer.
/// Migrates from the old tokenizer if needed.
fn migrate_fts(conn: &Connection) -> Result<()> {
    let fts_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages_fts')",
        [],
        |row| row.get(0),
    )?;

    if fts_exists {
        // Check if the FTS table already uses porter tokenizer
        let create_sql: String = conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='messages_fts'",
            [],
            |row| row.get(0),
        )?;

        if create_sql.contains("porter") {
            return Ok(());
        }

        // Old schema — drop and recreate with porter tokenizer
        eprintln!("Migrating FTS index to Porter stemming tokenizer...");
        conn.execute_batch(
            "
            DROP TRIGGER IF EXISTS messages_ai;
            DROP TRIGGER IF EXISTS messages_ad;
            DROP TRIGGER IF EXISTS messages_au;
            DROP TABLE messages_fts;
            ",
        )?;
    }

    conn.execute_batch(
        "
        CREATE VIRTUAL TABLE messages_fts USING fts5(
            content,
            content_rowid='id',
            content='messages',
            tokenize='porter unicode61'
        );
        ",
    )?;

    // If migrating from old schema, rebuild the index from existing messages
    if fts_exists {
        conn.execute_batch("INSERT INTO messages_fts(messages_fts) VALUES('rebuild')")?;
        eprintln!("FTS index rebuilt with Porter stemming.");
    }

    Ok(())
}

pub fn upsert_session(
    conn: &Connection,
    session_id: &str,
    project: &str,
    started_at: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (session_id, project, started_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(session_id) DO UPDATE SET
            project = excluded.project,
            started_at = COALESCE(excluded.started_at, sessions.started_at)",
        params![session_id, project, started_at],
    )?;
    Ok(())
}

pub fn insert_message(
    conn: &Connection,
    session_id: &str,
    uuid: Option<&str>,
    role: &str,
    content: &str,
    timestamp: Option<&str>,
) -> Result<bool> {
    // Skip if uuid already exists
    if let Some(uuid_val) = uuid {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE uuid = ?1)",
            params![uuid_val],
            |row| row.get(0),
        )?;
        if exists {
            return Ok(false);
        }
    }

    conn.execute(
        "INSERT INTO messages (session_id, uuid, role, content, timestamp)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session_id, uuid, role, content, timestamp],
    )?;
    Ok(true)
}

pub struct SearchResult {
    pub session_id: String,
    pub project: String,
    pub role: String,
    pub content: String,
    pub timestamp: Option<String>,
}

/// Escape a query string for safe use in FTS5 MATCH.
/// Wraps each token in double quotes to prevent FTS5 operator interpretation.
fn escape_fts_query(query: &str) -> String {
    // If the user already used explicit FTS5 syntax (AND, OR, NOT, quotes), pass through
    if query.contains('"')
        || query.contains(" AND ")
        || query.contains(" OR ")
        || query.contains(" NOT ")
    {
        return query.to_string();
    }
    // Otherwise, quote each whitespace-separated token
    query
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalize a project filter so that "user/repo" also matches raw dir names like
/// "-home-foo-ghq-github-com-user-repo". Replaces "/" with "-" for LIKE matching.
fn normalize_project_filter(filter: &str) -> String {
    let normalized = filter.replace('/', "-");
    format!("%{normalized}%")
}

pub fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
    project_filter: Option<&str>,
    role_filter: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<Vec<SearchResult>> {
    let escaped = escape_fts_query(query);
    if escaped.is_empty() {
        return Ok(vec![]);
    }

    let mut sql = String::from(
        "SELECT m.session_id, s.project, m.role, m.content, m.timestamp
         FROM messages_fts f
         JOIN messages m ON m.id = f.rowid
         JOIN sessions s ON s.session_id = m.session_id
         WHERE messages_fts MATCH ?1",
    );
    let mut param_idx = 2;

    if project_filter.is_some() {
        sql.push_str(&format!(" AND s.project LIKE ?{param_idx}"));
        param_idx += 1;
    }
    if role_filter.is_some() {
        sql.push_str(&format!(" AND m.role = ?{param_idx}"));
        param_idx += 1;
    }
    if since.is_some() {
        sql.push_str(&format!(" AND m.timestamp >= ?{param_idx}"));
        param_idx += 1;
    }
    if until.is_some() {
        sql.push_str(&format!(" AND m.timestamp <= ?{param_idx}"));
        param_idx += 1;
    }
    sql.push_str(&format!(" ORDER BY rank LIMIT ?{param_idx}"));

    let mut stmt = conn.prepare(&sql)?;

    // Build dynamic params
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    params_vec.push(Box::new(escaped));
    if let Some(proj) = project_filter {
        params_vec.push(Box::new(normalize_project_filter(proj)));
    }
    if let Some(role) = role_filter {
        params_vec.push(Box::new(role.to_string()));
    }
    if let Some(s) = since {
        params_vec.push(Box::new(s.to_string()));
    }
    if let Some(u) = until {
        params_vec.push(Box::new(u.to_string()));
    }
    params_vec.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();

    let results = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SearchResult {
                session_id: row.get(0)?,
                project: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

pub fn stats(conn: &Connection) -> Result<(i64, i64)> {
    let session_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
    let message_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
    Ok((session_count, message_count))
}

/// Resolve a session ID prefix to a full session ID.
/// Returns an error if the prefix matches zero or multiple sessions.
pub fn resolve_session_id(conn: &Connection, prefix: &str) -> Result<String> {
    let mut stmt =
        conn.prepare("SELECT session_id FROM sessions WHERE session_id LIKE ?1 || '%'")?;
    let matches: Vec<String> = stmt
        .query_map(params![prefix], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    match matches.len() {
        0 => anyhow::bail!("No session found matching: {prefix}"),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => {
            let previews: Vec<String> = matches.iter().take(5).cloned().collect();
            anyhow::bail!(
                "Ambiguous prefix '{prefix}' matches {n} sessions:\n  {}",
                previews.join("\n  ")
            );
        }
    }
}

pub fn get_session_messages(conn: &Connection, session_id: &str) -> Result<Vec<SearchResult>> {
    let mut stmt = conn.prepare(
        "SELECT m.session_id, s.project, m.role, m.content, m.timestamp
         FROM messages m
         JOIN sessions s ON s.session_id = m.session_id
         WHERE m.session_id = ?1
         ORDER BY m.id ASC",
    )?;

    let results = stmt
        .query_map(params![session_id], |row| {
            Ok(SearchResult {
                session_id: row.get(0)?,
                project: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Get the Nth most recent session ID (0-indexed).
pub fn nth_recent_session_id(conn: &Connection, n: usize) -> Result<String> {
    conn.query_row(
        "SELECT session_id FROM sessions
         ORDER BY COALESCE(started_at, imported_at) DESC
         LIMIT 1 OFFSET ?1",
        params![n as i64],
        |row| row.get(0),
    )
    .with_context(|| format!("No session found at position {}", n + 1))
}

pub struct SessionSummary {
    pub session_id: String,
    pub project: String,
    pub started_at: Option<String>,
    pub message_count: i64,
    pub first_user_message: Option<String>,
}

pub fn list_sessions(
    conn: &Connection,
    limit: usize,
    project_filter: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<Vec<SessionSummary>> {
    let mut sql = String::from(
        "SELECT s.session_id, s.project, s.started_at,
                (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.session_id) as msg_count,
                NULL as first_msg
         FROM sessions s",
    );
    let mut conditions = Vec::new();
    let mut param_idx = 1;
    if project_filter.is_some() {
        conditions.push(format!("s.project LIKE ?{param_idx}"));
        param_idx += 1;
    }
    if since.is_some() {
        conditions.push(format!(
            "COALESCE(s.started_at, s.imported_at) >= ?{param_idx}"
        ));
        param_idx += 1;
    }
    if until.is_some() {
        conditions.push(format!(
            "COALESCE(s.started_at, s.imported_at) <= ?{param_idx}"
        ));
        param_idx += 1;
    }
    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    sql.push_str(&format!(
        " ORDER BY COALESCE(s.started_at, s.imported_at) DESC LIMIT ?{param_idx}"
    ));

    let mut stmt = conn.prepare(&sql)?;

    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(proj) = project_filter {
        params_vec.push(Box::new(normalize_project_filter(proj)));
    }
    if let Some(s) = since {
        params_vec.push(Box::new(s.to_string()));
    }
    if let Some(u) = until {
        params_vec.push(Box::new(u.to_string()));
    }
    params_vec.push(Box::new(limit as i64));
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();

    let mut sessions: Vec<SessionSummary> = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(SessionSummary {
                session_id: row.get(0)?,
                project: row.get(1)?,
                started_at: row.get(2)?,
                message_count: row.get(3)?,
                first_user_message: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Fetch first meaningful user message for each session
    let mut user_stmt = conn.prepare(
        "SELECT substr(content, 1, 200) FROM messages
         WHERE session_id = ?1 AND role = 'user'
         ORDER BY id ASC LIMIT 10",
    )?;
    // Fallback: first assistant message
    let mut asst_stmt = conn.prepare(
        "SELECT substr(content, 1, 200) FROM messages
         WHERE session_id = ?1 AND role = 'assistant'
         ORDER BY id ASC LIMIT 3",
    )?;

    for session in &mut sessions {
        let candidates: Vec<String> = user_stmt
            .query_map(params![&session.session_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        session.first_user_message = candidates.into_iter().find(|c| is_meaningful_preview(c));

        // If no meaningful user message found, try assistant messages
        if session.first_user_message.is_none() {
            let asst_candidates: Vec<String> = asst_stmt
                .query_map(params![&session.session_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            session.first_user_message = asst_candidates
                .into_iter()
                .find(|c| is_meaningful_preview(c));
        }
    }

    Ok(sessions)
}

/// Check if a message is suitable as a session preview in `recent`.
/// Skips tool_result artifacts and system meta-messages.
fn is_meaningful_preview(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.len() < 5 {
        return false;
    }
    // Skip messages starting with JSON/XML/path characters (likely tool output)
    let first_char = trimmed.chars().next().unwrap_or(' ');
    if matches!(first_char, '{' | '[' | '<') {
        return false;
    }
    // System/meta messages that are never human input
    let noise_prefixes = [
        "Tool loaded",
        "This session is being continued",
        "Your task is to create a detailed summary",
    ];
    for prefix in &noise_prefixes {
        if trimmed.starts_with(prefix) {
            return false;
        }
    }
    true
}

pub fn delete_session(conn: &Connection, session_id: &str) -> Result<u64> {
    let msg_deleted = conn.execute(
        "DELETE FROM messages WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute(
        "DELETE FROM sessions WHERE session_id = ?1",
        params![session_id],
    )?;
    Ok(msg_deleted as u64)
}

pub fn verify(conn: &Connection) -> Result<()> {
    println!("=== 1. Messages by role ===");
    let mut stmt = conn.prepare("SELECT role, COUNT(*) FROM messages GROUP BY role")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (role, count) = row?;
        println!("  {role}: {count}");
    }

    println!("\n=== 2. Sessions and projects ===");
    let session_count: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
    let project_count: i64 =
        conn.query_row("SELECT COUNT(DISTINCT project) FROM sessions", [], |r| {
            r.get(0)
        })?;
    println!("  Sessions: {session_count}, Projects: {project_count}");

    println!("\n=== 3. Top 5 projects by message count ===");
    let mut stmt = conn.prepare(
        "SELECT s.project, COUNT(*) as cnt FROM messages m
         JOIN sessions s ON s.session_id = m.session_id
         GROUP BY s.project ORDER BY cnt DESC LIMIT 5",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (proj, count) = row?;
        println!("  {}: {count}", format_project_name(&proj));
    }

    println!("\n=== 4. FTS index integrity ===");
    let msg_count: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
    let fts_count: i64 = conn.query_row("SELECT COUNT(*) FROM messages_fts", [], |r| r.get(0))?;
    let fts_ok = msg_count == fts_count;
    println!("  messages: {msg_count} rows");
    println!("  messages_fts: {fts_count} rows");
    println!("  Match: {}", if fts_ok { "OK" } else { "FAIL" });

    println!("\n=== 5. UUID deduplication ===");
    let dup_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT uuid, COUNT(*) as cnt FROM messages WHERE uuid IS NOT NULL GROUP BY uuid HAVING cnt > 1)",
        [],
        |r| r.get(0),
    )?;
    let dup_ok = dup_count == 0;
    println!(
        "  Duplicate UUIDs: {dup_count} {}",
        if dup_ok { "OK" } else { "FAIL" }
    );

    println!("\n=== 6. Recent message samples ===");
    let mut stmt = conn.prepare(
        "SELECT m.role, substr(m.content, 1, 120), m.timestamp
         FROM messages m JOIN sessions s ON s.session_id = m.session_id
         ORDER BY m.timestamp DESC LIMIT 10",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (role, content, ts) = row?;
        let content = content.replace('\n', " ");
        println!("  [{role}] {} | {content}", ts.as_deref().unwrap_or("?"));
    }

    println!("\n=== 7. FTS5 search test ===");
    for query in ["import", "SQLite", "cargo"] {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH ?1",
            params![query],
            |r| r.get(0),
        )?;
        println!("  \"{query}\": {count} hits");
    }

    if !fts_ok || !dup_ok {
        anyhow::bail!("Verification failed");
    }
    println!("\nAll checks passed.");
    Ok(())
}

/// Initialize embeddings table for semantic search
fn init_embeddings(conn: &Connection) -> Result<()> {
    let embeddings_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='embeddings')",
        [],
        |row| row.get(0),
    )?;

    if !embeddings_exists {
        conn.execute_batch(
            "
            CREATE TABLE embeddings (
                message_id INTEGER PRIMARY KEY,
                embedding TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
            );

            CREATE INDEX idx_embeddings_message ON embeddings(message_id);
            ",
        )?;
    }

    Ok(())
}

/// Initialize wiki tables with FTS5 support
fn init_wiki_tables(conn: &Connection) -> Result<()> {
    // Create base wiki_pages table (matches wiki.rs schema)
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS wiki_pages (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            frontmatter TEXT NOT NULL,
            content TEXT NOT NULL,
            quality_score INTEGER,
            created_at TEXT,
            categories TEXT,
            tags TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_wiki_quality ON wiki_pages(quality_score);
        CREATE INDEX IF NOT EXISTS idx_wiki_categories ON wiki_pages(categories);
        ",
    )?;

    // Create synthesis_pages table
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS synthesis_pages (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            topic TEXT NOT NULL,
            summary TEXT NOT NULL,
            key_points TEXT,
            entities TEXT,
            related_topics TEXT,
            sources TEXT,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_synthesis_topic ON synthesis_pages(topic);
        ",
    )?;

    Ok(())
}

/// Store embedding for a message
#[allow(dead_code)]
pub fn store_embedding(conn: &Connection, message_id: i64, embedding: &[f32]) -> Result<()> {
    let embedding_json = serde_json::to_string(embedding)?;
    conn.execute(
        "INSERT INTO embeddings (message_id, embedding) VALUES (?1, ?2)
         ON CONFLICT(message_id) DO UPDATE SET embedding = excluded.embedding",
        params![message_id, embedding_json],
    )?;
    Ok(())
}

/// Get embedding for a message
#[allow(dead_code)]
pub fn get_embedding(conn: &Connection, message_id: i64) -> Result<Option<Vec<f32>>> {
    let result: Option<String> = conn
        .query_row(
            "SELECT embedding FROM embeddings WHERE message_id = ?1",
            params![message_id],
            |row| row.get(0),
        )
        .ok();

    match result {
        Some(json) => {
            let vec: Vec<f32> = serde_json::from_str(&json)?;
            Ok(Some(vec))
        }
        None => Ok(None),
    }
}

/// Cosine similarity between two vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

/// Result from semantic search
#[derive(Debug)]
#[allow(dead_code)]
pub struct SemanticSearchResult {
    pub message_id: i64,
    pub session_id: String,
    pub project: String,
    pub role: String,
    pub content: String,
    pub timestamp: Option<String>,
    pub score: f32,
}

/// Semantic search using embeddings
/// Returns messages similar to the query embedding, sorted by similarity
#[allow(dead_code)]
pub fn semantic_search(
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
    project_filter: Option<&str>,
    role_filter: Option<&str>,
) -> Result<Vec<SemanticSearchResult>> {
    // Get all messages that have embeddings
    let sql = if project_filter.is_some() {
        "SELECT m.id, m.session_id, s.project, m.role, m.content, m.timestamp, e.embedding
         FROM messages m
         JOIN sessions s ON s.session_id = m.session_id
         JOIN embeddings e ON e.message_id = m.id
         WHERE s.project LIKE ?1"
    } else {
        "SELECT m.id, m.session_id, s.project, m.role, m.content, m.timestamp, e.embedding
         FROM messages m
         JOIN sessions s ON s.session_id = m.session_id
         JOIN embeddings e ON e.message_id = m.id"
    };

    let mut stmt = conn.prepare(sql)?;

    let rows: Vec<(i64, String, String, String, String, Option<String>, String)> =
        if let Some(proj) = project_filter {
            let normalized = format!("%{}%", proj.replace('/', "-"));
            stmt.query_map(params![normalized], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

    // Calculate similarity scores
    let mut results: Vec<SemanticSearchResult> = rows
        .into_iter()
        .filter_map(
            |(msg_id, session_id, project, role, content, timestamp, emb_json)| {
                let embedding: Vec<f32> = serde_json::from_str(&emb_json).ok()?;
                let score = cosine_similarity(query_embedding, &embedding);

                // Filter by role if specified
                if let Some(ref role_filter) = role_filter {
                    if role.as_str() != *role_filter {
                        return None;
                    }
                }

                Some(SemanticSearchResult {
                    message_id: msg_id,
                    session_id,
                    project,
                    role,
                    content,
                    timestamp,
                    score,
                })
            },
        )
        .collect();

    // Sort by score (highest first)
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    // Apply limit
    results.truncate(limit);

    Ok(results)
}

/// Check if embeddings have been generated
#[allow(dead_code)]
pub fn has_embeddings(conn: &Connection) -> Result<bool> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))?;
    Ok(count > 0)
}

/// Get count of embeddings
#[allow(dead_code)]
pub fn embedding_count(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM embeddings", [], |row| row.get(0))
        .map_err(Into::into)
}

// ============================================================================
// Wiki search functions
// ============================================================================

/// Result from wiki search
#[derive(Debug)]
#[allow(dead_code)]
pub struct WikiSearchResult {
    pub id: String,
    pub title: String,
    pub content: String,
    pub quality_score: i32,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub session_id: String,
}

/// Search wiki pages using full-text search
#[allow(dead_code)]
pub fn search_wiki(
    conn: &Connection,
    query: &str,
    limit: usize,
    min_quality: Option<u8>,
) -> Result<Vec<WikiSearchResult>> {
    let limit = limit.min(100);

    // Build WHERE clause
    let mut where_clauses = vec!["1=1".to_string()];
    if let Some(min_q) = min_quality {
        where_clauses.push(format!("quality_score >= {}", min_q));
    }

    // Add content search using LIKE (simplified, can add FTS5 later)
    where_clauses.push(format!("(content LIKE '%{query}%' OR title LIKE '%{query}%' OR categories LIKE '%{query}%' OR tags LIKE '%{query}%')",
        query = query.replace('\'', "''")));

    let sql = format!(
        "SELECT id, title, content, quality_score, categories, tags
         FROM wiki_pages
         WHERE {}
         ORDER BY quality_score DESC
         LIMIT {limit}",
        where_clauses.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql)?;

    let rows: Vec<(String, String, String, i32, String, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let results: Vec<WikiSearchResult> = rows
        .into_iter()
        .map(
            |(id, title, content, quality_score, categories_json, tags_json)| {
                let categories: Vec<String> =
                    serde_json::from_str(&categories_json).unwrap_or_default();
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

                WikiSearchResult {
                    id: id.clone(),
                    title,
                    content,
                    quality_score,
                    categories,
                    tags,
                    session_id: id,
                }
            },
        )
        .collect();

    Ok(results)
}

/// Get synthesis page by topic
#[allow(dead_code)]
pub fn get_synthesis_page(conn: &Connection, topic: &str) -> Result<Option<SynthesisPage>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, topic, summary, key_points, entities, related_topics, sources
         FROM synthesis_pages
         WHERE topic = ?1",
    )?;

    let result = stmt
        .query_row(params![topic], |row| {
            let key_points: String = row.get(4)?;
            let entities: String = row.get(5)?;
            let related: String = row.get(6)?;
            let sources: String = row.get(7)?;

            Ok(SynthesisPage {
                id: row.get(0)?,
                title: row.get(1)?,
                topic: row.get(2)?,
                summary: row.get(3)?,
                key_points: serde_json::from_str(&key_points).unwrap_or_default(),
                entities: serde_json::from_str(&entities).unwrap_or_default(),
                related_topics: serde_json::from_str(&related).unwrap_or_default(),
                sources: serde_json::from_str(&sources).unwrap_or_default(),
            })
        })
        .ok();

    Ok(result)
}

/// Synthesis page structure
#[derive(Debug)]
#[allow(dead_code)]
pub struct SynthesisPage {
    pub id: String,
    pub title: String,
    pub topic: String,
    pub summary: String,
    pub key_points: Vec<String>,
    pub entities: Vec<String>,
    pub related_topics: Vec<String>,
    pub sources: Vec<String>,
}

/// List all available categories
#[allow(dead_code)]
pub fn list_categories(conn: &Connection) -> Result<Vec<CategoryInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, document_count
         FROM categories
         ORDER BY document_count DESC",
    )?;

    let categories = stmt
        .query_map([], |row| {
            Ok(CategoryInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                document_count: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(categories)
}

/// Category information
#[derive(Debug)]
#[allow(dead_code)]
pub struct CategoryInfo {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub document_count: i64,
}

/// Find entities by name pattern
#[allow(dead_code)]
pub fn find_entities(conn: &Connection, pattern: &str, limit: usize) -> Result<Vec<EntityInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, entity_type, COUNT(*) as mention_count
         FROM entities
         WHERE name LIKE ?1
         GROUP BY id
         ORDER BY mention_count DESC
         LIMIT ?2",
    )?;

    let entities = stmt
        .query_map(params![format!("%{}%", pattern), limit as i64], |row| {
            Ok(EntityInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                entity_type: row.get(2)?,
                mention_count: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entities)
}

/// Entity information
#[derive(Debug)]
#[allow(dead_code)]
pub struct EntityInfo {
    pub id: i64,
    pub name: String,
    pub entity_type: String,
    pub mention_count: i64,
}

/// Get wiki statistics
#[allow(dead_code)]
pub fn wiki_stats(conn: &Connection) -> Result<WikiStats> {
    let page_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM wiki_pages", [], |row| row.get(0))?;
    let synthesis_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM synthesis_pages", [], |row| row.get(0))?;
    let category_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM categories", [], |row| row.get(0))?;
    let entity_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;

    let avg_quality: f64 = conn
        .query_row(
            "SELECT AVG(quality_score) FROM wiki_pages WHERE quality_score > 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    Ok(WikiStats {
        page_count,
        synthesis_count,
        category_count,
        entity_count,
        avg_quality,
    })
}

/// Wiki statistics
#[derive(Debug)]
#[allow(dead_code)]
pub struct WikiStats {
    pub page_count: i64,
    pub synthesis_count: i64,
    pub category_count: i64,
    pub entity_count: i64,
    pub avg_quality: f64,
}

// ============================================================================
// Theme support for themed wikis
// ============================================================================

/// Theme for domain-specific wiki projection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub wiki_db: String,
    pub created_at: String,
    pub entry_count: i64,
}

/// Theme registry loaded from themes.json
#[derive(Debug, Serialize, Deserialize)]
struct ThemeRegistry {
    themes: Vec<Theme>,
    #[serde(default)]
    default_theme: Option<String>,
}

/// Get the wiki directory path
pub fn wiki_dir() -> Result<PathBuf> {
    let data_dir = dirs::data_dir().ok_or_else(|| anyhow!("Failed to find data directory"))?;
    Ok(data_dir.join("claude-vault").join("wiki"))
}

/// Get the themes.json path
pub fn themes_json_path() -> Result<PathBuf> {
    Ok(wiki_dir()?.join("themes.json"))
}

/// Ensure wiki directory exists
pub fn ensure_wiki_dir() -> Result<()> {
    let dir = wiki_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create wiki directory: {}", dir.display()))?;
    Ok(())
}

/// Load all themes from themes.json
#[allow(dead_code)]
pub fn load_themes() -> Result<Vec<Theme>> {
    let path = themes_json_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read themes.json: {}", path.display()))?;

    let registry: ThemeRegistry = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse themes.json: {}", path.display()))?;

    Ok(registry.themes)
}

/// Save themes to themes.json
fn save_themes(themes: &[Theme]) -> Result<()> {
    ensure_wiki_dir()?;

    let path = themes_json_path()?;
    let registry = ThemeRegistry {
        themes: themes.to_vec(),
        default_theme: None,
    };

    let content =
        serde_json::to_string_pretty(&registry).with_context(|| "Failed to serialize themes")?;

    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write themes.json: {}", path.display()))?;

    Ok(())
}

/// Get theme by ID
#[allow(dead_code)]
pub fn get_theme(id: &str) -> Result<Theme> {
    let themes = load_themes()?;
    themes
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| anyhow!("Theme not found: {}", id))
}

/// Create a new theme
#[allow(dead_code)]
pub fn create_theme(
    id: String,
    name: String,
    description: String,
    keywords: Vec<String>,
) -> Result<Theme> {
    // Validate ID is slug-like
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        bail!("Invalid theme ID: must contain only letters, numbers, hyphens, underscores");
    }

    // Check for duplicate
    let themes = load_themes()?;
    if themes.iter().any(|t| t.id == id) {
        bail!("Theme already exists: {}", id);
    }

    ensure_wiki_dir()?;

    let wiki_db = format!("{}/{}.db", wiki_dir()?.display(), id);

    let theme = Theme {
        id: id.clone(),
        name,
        description,
        keywords,
        wiki_db,
        created_at: chrono::Utc::now().to_rfc3339(),
        entry_count: 0,
    };

    let mut updated_themes = themes;
    updated_themes.push(theme.clone());
    save_themes(&updated_themes)?;

    Ok(theme)
}

/// Delete a theme
#[allow(dead_code)]
pub fn delete_theme(id: &str) -> Result<()> {
    let themes = load_themes()?;
    let theme = themes
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| anyhow!("Theme not found: {}", id))?;

    // Delete wiki database
    let db_path = PathBuf::from(&theme.wiki_db);
    if db_path.exists() {
        std::fs::remove_file(&db_path)
            .with_context(|| format!("Failed to delete wiki database: {}", db_path.display()))?;
    }

    // Remove from registry
    let updated_themes: Vec<_> = themes.into_iter().filter(|t| t.id != id).collect();
    save_themes(&updated_themes)?;

    Ok(())
}

/// Get wiki database path for a theme
#[allow(dead_code)]
pub fn theme_wiki_path(theme_id: &str) -> Result<PathBuf> {
    let theme = get_theme(theme_id)?;
    Ok(PathBuf::from(theme.wiki_db))
}

/// Open wiki database for a theme
#[allow(dead_code)]
pub fn open_theme_wiki(theme_id: &str) -> Result<Connection> {
    let db_path = theme_wiki_path(theme_id)?;
    if !db_path.exists() {
        bail!(
            "Theme wiki database not found: {}. Run rebuild_theme first.",
            db_path.display()
        );
    }
    open_db(&db_path)
}

/// Get default wiki database path (wiki.db)
pub fn default_wiki_path() -> Result<PathBuf> {
    Ok(wiki_dir()?.join("wiki.db"))
}

/// Open default wiki database
#[allow(dead_code)]
pub fn open_default_wiki() -> Result<Connection> {
    let db_path = default_wiki_path()?;
    open_db(&db_path)
}

/// Filter sessions by keywords using FTS5
/// Returns session IDs that match any of the keywords
#[allow(dead_code)]
pub fn filter_sessions_by_keywords(conn: &Connection, keywords: &[String]) -> Result<Vec<String>> {
    if keywords.is_empty() {
        // Return all sessions if no keywords
        let mut stmt = conn.prepare("SELECT DISTINCT session_id FROM sessions")?;
        let result = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(result);
    }

    // Build FTS5 query with OR between keywords
    let query = keywords
        .iter()
        .map(|k| format!("\"{}\"", k.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");

    let sql = "
        SELECT DISTINCT m.session_id
        FROM messages_fts f
        JOIN messages m ON m.id = f.rowid
        WHERE messages_fts MATCH ?1
    ";

    let mut stmt = conn.prepare(sql)?;
    let session_ids: Vec<String> = stmt
        .query_map(params![query], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(session_ids)
}

/// Get messages for a specific session
#[allow(dead_code)]
pub fn get_session_messages_for_wiki(conn: &Connection, session_id: &str) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, timestamp
         FROM messages
         WHERE session_id = ?1
         ORDER BY id",
    )?;

    let messages = stmt
        .query_map(params![session_id], |row| {
            Ok(Message {
                id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                timestamp: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(messages)
}

/// Message structure for wiki generation
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Message {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub timestamp: Option<String>,
}

// ============================================================================
// Current session tracking for parallel project sessions
// ============================================================================

/// Get the sessions.db path
#[allow(dead_code)]
pub fn sessions_db_path() -> Result<PathBuf> {
    let data_dir = dirs::data_dir().ok_or_else(|| anyhow!("Failed to find data directory"))?;
    Ok(data_dir.join("claude-vault").join("sessions.db"))
}

/// Open or create sessions.db
#[allow(dead_code)]
pub fn open_sessions_db() -> Result<Connection> {
    let path = sessions_db_path()?;
    let conn = Connection::open(&path)
        .with_context(|| format!("Failed to open sessions database: {}", path.display()))?;

    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS current_sessions (
             project TEXT PRIMARY KEY,
             session_id TEXT NOT NULL,
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )?;

    Ok(conn)
}

/// Set current session for a project
#[allow(dead_code)]
pub fn set_current_session(conn: &Connection, project: &str, session_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO current_sessions (project, session_id) VALUES (?1, ?2)
         ON CONFLICT(project) DO UPDATE SET session_id=excluded.session_id, updated_at=datetime('now')",
        params![project, session_id],
    )?;
    Ok(())
}

/// Get current session for a project
#[allow(dead_code)]
pub fn get_current_session(conn: &Connection, project: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT session_id FROM current_sessions WHERE project = ?1",
            params![project],
            |row| row.get(0),
        )
        .ok())
}

/// Delete current session for a project
#[allow(dead_code)]
pub fn delete_current_session(conn: &Connection, project: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM current_sessions WHERE project = ?1",
        params![project],
    )?;
    Ok(())
}

/// List all current sessions
#[allow(dead_code)]
pub fn list_current_sessions(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt =
        conn.prepare("SELECT project, session_id FROM current_sessions ORDER BY updated_at DESC")?;

    let sessions = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(sessions)
}

/// Get latest session ID
/// If project is None, returns global most recent from vault
/// If project is Some, resolves to that project's current session via sessions.db
pub fn get_latest_session(vault_conn: &Connection, project: Option<&str>) -> Result<String> {
    if let Some(proj) = project {
        let sessions_conn = open_sessions_db()?;
        get_current_session(&sessions_conn, proj)?
            .ok_or_else(|| anyhow!("No current session for project: {}", proj))
    } else {
        nth_recent_session_id(vault_conn, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_db() -> (Connection, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = open_db(tmp.path()).unwrap();
        (conn, tmp)
    }

    #[test]
    fn test_open_and_init() {
        let (_conn, _tmp) = setup_db();
    }

    #[test]
    fn test_upsert_session() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "sess-1", "my-project", Some("2024-01-01T00:00:00Z")).unwrap();
        upsert_session(&conn, "sess-1", "my-project", None).unwrap();

        let project: String = conn
            .query_row(
                "SELECT project FROM sessions WHERE session_id = 'sess-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(project, "my-project");
    }

    #[test]
    fn test_insert_and_search() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "sess-1", "proj", None).unwrap();
        insert_message(
            &conn,
            "sess-1",
            Some("u1"),
            "user",
            "hello world",
            Some("2024-01-01T00:00:00Z"),
        )
        .unwrap();
        insert_message(
            &conn,
            "sess-1",
            Some("u2"),
            "assistant",
            "hi there",
            Some("2024-01-01T00:00:01Z"),
        )
        .unwrap();

        let results = search(&conn, "hello", 10, None, None, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].role, "user");
        assert_eq!(results[0].content, "hello world");
    }

    #[test]
    fn test_porter_stemming() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        insert_message(
            &conn,
            "s1",
            Some("u1"),
            "user",
            "the server is running fine",
            None,
        )
        .unwrap();
        insert_message(
            &conn,
            "s1",
            Some("u2"),
            "user",
            "configure the database settings",
            None,
        )
        .unwrap();

        // "run" should match "running" via stemming
        let results = search(&conn, "run", 10, None, None, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("running"));

        // "configuration" should match "configure" via stemming
        let results = search(&conn, "configuration", 10, None, None, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("configure"));
    }

    #[test]
    fn test_duplicate_uuid_skipped() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "sess-1", "proj", None).unwrap();
        let inserted = insert_message(&conn, "sess-1", Some("u1"), "user", "first", None).unwrap();
        assert!(inserted);
        let inserted =
            insert_message(&conn, "sess-1", Some("u1"), "user", "duplicate", None).unwrap();
        assert!(!inserted);

        let (_, msg_count) = stats(&conn).unwrap();
        assert_eq!(msg_count, 1);
    }

    #[test]
    fn test_stats() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "p1", None).unwrap();
        upsert_session(&conn, "s2", "p2", None).unwrap();
        insert_message(&conn, "s1", Some("u1"), "user", "msg1", None).unwrap();
        insert_message(&conn, "s1", Some("u2"), "assistant", "msg2", None).unwrap();
        insert_message(&conn, "s2", Some("u3"), "user", "msg3", None).unwrap();

        let (sessions, messages) = stats(&conn).unwrap();
        assert_eq!(sessions, 2);
        assert_eq!(messages, 3);
    }

    #[test]
    fn test_search_no_results() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        insert_message(&conn, "s1", Some("u1"), "user", "hello", None).unwrap();

        let results = search(&conn, "nonexistent", 10, None, None, None, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_with_date_filter() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        insert_message(
            &conn,
            "s1",
            Some("u1"),
            "user",
            "early message",
            Some("2024-01-01T00:00:00Z"),
        )
        .unwrap();
        insert_message(
            &conn,
            "s1",
            Some("u2"),
            "user",
            "late message",
            Some("2024-06-01T00:00:00Z"),
        )
        .unwrap();

        let results = search(&conn, "message", 10, None, None, Some("2024-03-01"), None).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("late"));

        let results = search(&conn, "message", 10, None, None, None, Some("2024-03-01")).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("early"));
    }

    #[test]
    fn test_search_empty_query() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        insert_message(&conn, "s1", Some("u1"), "user", "hello", None).unwrap();

        let results = search(&conn, "", 10, None, None, None, None).unwrap();
        assert!(results.is_empty());

        let results = search(&conn, "   ", 10, None, None, None, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_project_filter_with_slash() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "-home-user-ghq-github-com-owner-repo", None).unwrap();
        insert_message(&conn, "s1", Some("u1"), "user", "test msg", None).unwrap();

        // Filter with formatted name "owner/repo"
        let results = search(&conn, "test", 10, Some("owner/repo"), None, None, None).unwrap();
        assert_eq!(results.len(), 1);

        // Filter with just "repo"
        let results = search(&conn, "test", 10, Some("repo"), None, None, None).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_delete_session() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        insert_message(&conn, "s1", Some("u1"), "user", "msg1", None).unwrap();
        insert_message(&conn, "s1", Some("u2"), "assistant", "msg2", None).unwrap();

        let deleted = delete_session(&conn, "s1").unwrap();
        assert_eq!(deleted, 2);

        let (sessions, messages) = stats(&conn).unwrap();
        assert_eq!(sessions, 0);
        assert_eq!(messages, 0);
    }

    #[test]
    fn test_list_sessions_with_date_filter() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", Some("2024-01-15T00:00:00Z")).unwrap();
        upsert_session(&conn, "s2", "proj", Some("2024-06-15T00:00:00Z")).unwrap();
        insert_message(&conn, "s1", Some("u1"), "user", "old session", None).unwrap();
        insert_message(&conn, "s2", Some("u2"), "user", "new session", None).unwrap();

        let sessions = list_sessions(&conn, 100, None, Some("2024-03-01"), None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "s2");

        let sessions = list_sessions(&conn, 100, None, None, Some("2024-03-01")).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "s1");
    }

    #[test]
    fn test_insert_without_uuid() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        let inserted = insert_message(&conn, "s1", None, "user", "msg1", None).unwrap();
        assert!(inserted);
        // Without uuid, duplicate check is skipped, so second insert also succeeds
        let inserted = insert_message(&conn, "s1", None, "user", "msg2", None).unwrap();
        assert!(inserted);

        let (_, count) = stats(&conn).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_filter_sessions_by_keywords_empty() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        upsert_session(&conn, "s2", "proj", None).unwrap();

        let sessions = filter_sessions_by_keywords(&conn, &[]).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_filter_sessions_by_keywords_match() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        insert_message(&conn, "s1", Some("u1"), "user", "rust async code", None).unwrap();
        insert_message(&conn, "s1", Some("u2"), "assistant", "tokio spawn", None).unwrap();

        upsert_session(&conn, "s2", "proj", None).unwrap();
        insert_message(&conn, "s2", Some("u3"), "user", "python flask", None).unwrap();

        let sessions =
            filter_sessions_by_keywords(&conn, &["rust".to_string(), "tokio".to_string()]).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0], "s1");
    }

    #[test]
    fn test_filter_sessions_by_keywords_no_match() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        insert_message(&conn, "s1", Some("u1"), "user", "python flask", None).unwrap();

        let sessions = filter_sessions_by_keywords(&conn, &["rust".to_string()]).unwrap();
        assert_eq!(sessions.len(), 0);
    }

    #[test]
    fn test_get_session_messages_for_wiki() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", None).unwrap();
        insert_message(
            &conn,
            "s1",
            Some("u1"),
            "user",
            "test message",
            Some("2024-01-01T00:00:00Z"),
        )
        .unwrap();

        let messages = get_session_messages_for_wiki(&conn, "s1").unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "test message");
    }

    #[test]
    fn test_get_latest_session_global() {
        let (conn, _tmp) = setup_db();
        upsert_session(&conn, "s1", "proj", Some("2024-01-01T00:00:00Z")).unwrap();
        upsert_session(&conn, "s2", "proj", Some("2024-06-15T12:00:00Z")).unwrap();
        upsert_session(&conn, "s3", "proj", Some("2024-03-01T00:00:00Z")).unwrap();

        let latest = get_latest_session(&conn, None).unwrap();
        assert_eq!(latest, "s2");
    }

    #[test]
    fn test_get_latest_session_empty_global() {
        let (conn, _tmp) = setup_db();

        let result = get_latest_session(&conn, None);
        assert!(result.is_err());
    }

    // sessions.db tests
    #[test]
    fn test_open_sessions_db() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE current_sessions (
                project TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        assert!(conn
            .execute(
                "INSERT INTO current_sessions (project, session_id) VALUES ('test', 'session1')",
                []
            )
            .is_ok());
    }

    #[test]
    fn test_set_current_session() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE current_sessions (
                project TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        set_current_session(&conn, "project1", "session1").unwrap();

        let session_id: String = conn
            .query_row(
                "SELECT session_id FROM current_sessions WHERE project = 'project1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_id, "session1");
    }

    #[test]
    fn test_set_current_session_update() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE current_sessions (
                project TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        set_current_session(&conn, "project1", "session1").unwrap();
        set_current_session(&conn, "project1", "session2").unwrap();

        let session_id: String = conn
            .query_row(
                "SELECT session_id FROM current_sessions WHERE project = 'project1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_id, "session2");
    }

    #[test]
    fn test_get_current_session() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE current_sessions (
                project TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO current_sessions (project, session_id) VALUES ('project1', 'session1')",
            [],
        )
        .unwrap();

        let session = get_current_session(&conn, "project1").unwrap();
        assert_eq!(session, Some("session1".to_string()));

        let missing = get_current_session(&conn, "project2").unwrap();
        assert_eq!(missing, None);
    }

    #[test]
    fn test_delete_current_session() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE current_sessions (
                project TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO current_sessions (project, session_id) VALUES ('project1', 'session1')",
            [],
        )
        .unwrap();

        delete_current_session(&conn, "project1").unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM current_sessions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_list_current_sessions() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = Connection::open(tmp.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE current_sessions (
                project TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        set_current_session(&conn, "project1", "session1").unwrap();
        set_current_session(&conn, "project2", "session2").unwrap();

        let sessions = list_current_sessions(&conn).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_get_latest_session_with_project() {
        let tmp = NamedTempFile::new().unwrap();
        let sessions_conn = Connection::open(tmp.path()).unwrap();
        sessions_conn
            .execute_batch(
                "CREATE TABLE current_sessions (
                    project TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );",
            )
            .unwrap();

        set_current_session(&sessions_conn, "my-project", "abc123").unwrap();

        let result = get_latest_session(&sessions_conn, Some("my-project"));
        assert!(result.is_err());
    }
}
