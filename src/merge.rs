//! Merge multiple claude-vault databases into one

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Merge statistics
#[derive(Debug, Default)]
pub struct MergeStats {
    pub sources_merged: usize,
    pub sessions_added: usize,
    pub sessions_duplicate: usize,
    pub messages_added: usize,
    pub messages_duplicate: usize,
    pub wiki_pages_added: usize,
    pub entities_added: usize,
}

/// Merge multiple databases into target
pub fn merge_databases(target: &Connection, source_paths: &[&Path]) -> Result<MergeStats> {
    let mut stats = MergeStats::default();

    for source_path in source_paths {
        let source_stats = merge_single(target, source_path)?;
        stats.sources_merged += 1;
        stats.sessions_added += source_stats.sessions_added;
        stats.sessions_duplicate += source_stats.sessions_duplicate;
        stats.messages_added += source_stats.messages_added;
        stats.messages_duplicate += source_stats.messages_duplicate;
        stats.wiki_pages_added += source_stats.wiki_pages_added;
        stats.entities_added += source_stats.entities_added;
    }

    Ok(stats)
}

/// Merge single source database into target
pub fn merge_single(target: &Connection, source_path: &Path) -> Result<MergeStats> {
    let source_name = "source_db";
    target.execute(
        &format!("ATTACH DATABASE ?1 AS {}", source_name),
        [source_path.to_str().unwrap()],
    )?;

    let mut stats = MergeStats::default();

    // Merge sessions
    let session_added = target
        .execute(
            &format!(
                "INSERT OR IGNORE INTO sessions
             (session_id, project, started_at, imported_at)
             SELECT session_id, project, started_at, imported_at FROM {}.sessions",
                source_name
            ),
            [],
        )
        .unwrap_or(0);
    stats.sessions_added = session_added;

    // Count duplicates
    stats.sessions_duplicate = target
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM {}.sessions s
             WHERE EXISTS (SELECT 1 FROM sessions t WHERE t.session_id = s.session_id)",
                source_name
            ),
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Merge messages (only for sessions that were added)
    let messages_added = target
        .execute(
            &format!(
                "INSERT OR IGNORE INTO messages
             (session_id, uuid, role, content, timestamp)
             SELECT session_id, uuid, role, content, timestamp
             FROM {}.messages m
             WHERE EXISTS (SELECT 1 FROM sessions s WHERE s.session_id = m.session_id)",
                source_name
            ),
            [],
        )
        .unwrap_or(0);
    stats.messages_added = messages_added;

    // Count message duplicates
    stats.messages_duplicate = target
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM {}.messages m
             WHERE EXISTS (SELECT 1 FROM messages t WHERE t.uuid = m.uuid AND m.uuid IS NOT NULL)",
                source_name
            ),
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Merge wiki pages if table exists
    if table_exists(target, source_name, "wiki_pages")? {
        let wiki_added = target
            .execute(
                &format!(
                    "INSERT OR IGNORE INTO wiki_pages
                 (id, title, frontmatter, content, quality_score, created_at, categories, tags)
                 SELECT id, title, frontmatter, content, quality_score, created_at, categories, tags
                 FROM {}.wiki_pages",
                    source_name
                ),
                [],
            )
            .unwrap_or(0);
        stats.wiki_pages_added = wiki_added;
    }

    // Merge entities if table exists
    if table_exists(target, source_name, "entities")? {
        let entity_added = target
            .execute(
                &format!(
                    "INSERT OR IGNORE INTO entities
                 (name, entity_type, count, first_seen)
                 SELECT name, entity_type, count, first_seen FROM {}.entities",
                    source_name
                ),
                [],
            )
            .unwrap_or(0);
        stats.entities_added = entity_added;
    }

    // Detach
    target.execute(&format!("DETACH DATABASE {}", source_name), [])?;

    Ok(stats)
}

/// Check if table exists in attached database
fn table_exists(conn: &Connection, db_name: &str, table_name: &str) -> Result<bool> {
    let exists: bool = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM {}.sqlite_master
                 WHERE type='table' AND name=?1",
                db_name
            ),
            [table_name],
            |row| row.get(0),
        )
        .unwrap_or(0)
        > 0;
    Ok(exists)
}

/// Print merge statistics
pub fn print_stats(stats: &MergeStats) {
    println!("\n=== Merge Results ===");
    println!("Sources merged: {}", stats.sources_merged);
    println!(
        "Sessions: {} added, {} duplicates",
        stats.sessions_added, stats.sessions_duplicate
    );
    println!(
        "Messages: {} added, {} duplicates",
        stats.messages_added, stats.messages_duplicate
    );
    if stats.wiki_pages_added > 0 {
        println!("Wiki pages: {} added", stats.wiki_pages_added);
    }
    if stats.entities_added > 0 {
        println!("Entities: {} added", stats.entities_added);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use tempfile::NamedTempFile;

    fn setup_test_db() -> (Connection, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = open_db(tmp.path()).unwrap();
        (conn, tmp)
    }

    #[test]
    fn test_merge_empty_databases() {
        let (target, target_file) = setup_test_db();
        let (source, source_file) = setup_test_db();

        let stats = merge_single(&target, source_file.path()).unwrap();
        assert_eq!(stats.sessions_added, 0);

        drop(source);
        drop(target_file);
    }

    #[test]
    fn test_table_exists_check() {
        let (conn, _tmp) = setup_test_db();

        // messages table should exist
        assert!(table_exists(&conn, "main", "messages").unwrap());

        // fake table should not exist
        assert!(!table_exists(&conn, "main", "nonexistent_table").unwrap());
    }
}
