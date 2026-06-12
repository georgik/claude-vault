//! Wiki generation from conversations
//!
//! Transforms scored conversations into structured wiki pages with metadata.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db::get_session_messages;
use crate::quality::QualityResult;

/// Wiki page with frontmatter and content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiPage {
    pub id: String,
    pub title: String,
    pub frontmatter: Frontmatter,
    pub content: String,
    pub references: Vec<String>,
}

/// YAML frontmatter metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frontmatter {
    pub title: String,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub quality_score: u8,
    pub created_at: String,
    pub session_id: String,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<String>>,
}

/// Wiki generator transforms conversations into wiki pages
pub struct WikiGenerator;

impl WikiGenerator {
    pub fn new() -> Self {
        Self
    }

    /// Generate wiki page from session
    pub fn generate_page(
        &self,
        conn: &Connection,
        session_id: &str,
        quality_result: &QualityResult,
        categories: &[String],
        tags: &[String],
    ) -> Result<WikiPage> {
        let messages = get_session_messages(conn, session_id)?;

        if messages.is_empty() {
            anyhow::bail!("Session {} has no messages", session_id);
        }

        // Extract title from first user message
        let title = self.extract_title(&messages);

        // Clean content for wiki
        let content = self.clean_content(&messages);

        // Detect cross-references
        let references = self.detect_references(&content);

        // Generate frontmatter
        let frontmatter = Frontmatter {
            title: title.clone(),
            categories: categories.to_vec(),
            tags: tags.to_vec(),
            quality_score: quality_result.score.value(),
            created_at: messages[0]
                .timestamp
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            session_id: session_id.to_string(),
            content_type: "conversation".to_string(),
            related: None,
            entities: None,
        };

        Ok(WikiPage {
            id: session_id.to_string(),
            title,
            frontmatter,
            content,
            references,
        })
    }

    /// Extract title from first user message
    fn extract_title(&self, messages: &[crate::db::SearchResult]) -> String {
        messages
            .iter()
            .filter(|m| m.role == "user")
            .find_map(|m| {
                m.content
                    .lines()
                    .map(|l| l.trim())
                    .find(|l| !l.is_empty() && !l.starts_with('#'))
            })
            .unwrap_or("Untitled")
            .chars()
            .take(80)
            .collect::<String>()
    }

    /// Clean content for wiki consumption
    fn clean_content(&self, messages: &[crate::db::SearchResult]) -> String {
        let mut cleaned = Vec::new();

        for msg in messages {
            if msg.role != "assistant" {
                continue;
            }

            // Remove tool_use lines
            let content: String = msg
                .content
                .lines()
                .filter(|line| !line.starts_with("[tool_use: "))
                .collect::<Vec<_>>()
                .join("\n");

            // Skip empty content
            if content.trim().is_empty() {
                continue;
            }

            cleaned.push(content);
        }

        cleaned.join("\n\n---\n\n")
    }

    /// Detect cross-references to other sessions
    fn detect_references(&self, content: &str) -> Vec<String> {
        let mut refs = Vec::new();

        // Look for session ID patterns (e.g., 47cf1f2e)
        let words: Vec<&str> = content.split_whitespace().collect();
        for word in words {
            if word.len() == 8
                && word.chars().all(|c| c.is_ascii_hexdigit())
                && !word.contains("http")
                && !word.contains("://")
            {
                refs.push(word.to_string());
            }
        }

        refs
    }

    /// Render wiki page as markdown with frontmatter
    pub fn render_markdown(&self, page: &WikiPage) -> String {
        let frontmatter_yaml =
            serde_yaml::to_string(&page.frontmatter).unwrap_or_else(|_| "---\n".to_string());

        format!("---\n{}---\n\n{}", frontmatter_yaml, page.content)
    }

    /// Generate wiki page from messages directly (for themed wiki generation)
    pub fn generate_page_from_messages(
        &self,
        messages: &[crate::db::Message],
        session_id: &str,
        quality_result: &QualityResult,
        categories: &[String],
        tags: &[String],
    ) -> Result<WikiPage> {
        if messages.is_empty() {
            anyhow::bail!("No messages provided");
        }

        // Convert messages to SearchResult format for reuse
        let search_results: Vec<crate::db::SearchResult> = messages
            .iter()
            .map(|m| crate::db::SearchResult {
                session_id: session_id.to_string(),
                project: "themed".to_string(),
                role: m.role.clone(),
                content: m.content.clone(),
                timestamp: m.timestamp.clone(),
            })
            .collect();

        // Extract title from first user message
        let title = self.extract_title(&search_results);

        // Clean content for wiki
        let content = self.clean_content(&search_results);

        // Detect cross-references
        let references = self.detect_references(&content);

        // Generate frontmatter
        let frontmatter = Frontmatter {
            title: title.clone(),
            categories: categories.to_vec(),
            tags: tags.to_vec(),
            quality_score: quality_result.score.value(),
            created_at: messages
                .iter()
                .find_map(|m| m.timestamp.as_ref())
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            session_id: session_id.to_string(),
            content_type: "conversation".to_string(),
            related: None,
            entities: None,
        };

        Ok(WikiPage {
            id: session_id.to_string(),
            title,
            frontmatter,
            content,
            references,
        })
    }
}

/// Initialize wiki pages table
pub fn init_wiki_tables(conn: &Connection) -> Result<()> {
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
    Ok(())
}

/// Save wiki page to database
pub fn save_wiki_page(conn: &Connection, page: &WikiPage) -> Result<()> {
    let frontmatter_json = serde_json::to_string(&page.frontmatter)?;
    let categories_json = serde_json::to_string(&page.frontmatter.categories)?;
    let tags_json = serde_json::to_string(&page.frontmatter.tags)?;

    conn.execute(
        "INSERT OR REPLACE INTO wiki_pages
         (id, title, frontmatter, content, quality_score, created_at, categories, tags)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            page.id,
            page.title,
            frontmatter_json,
            page.content,
            page.frontmatter.quality_score as i32,
            page.frontmatter.created_at,
            categories_json,
            tags_json,
        ],
    )?;

    Ok(())
}

/// Get all wiki pages from database
pub fn get_all_wiki_pages(conn: &Connection) -> Result<Vec<WikiPage>> {
    let mut stmt = conn.prepare("SELECT id, title, frontmatter, content FROM wiki_pages")?;

    let pages = stmt
        .query_map([], |row| {
            let frontmatter_json: String = row.get(2)?;
            let frontmatter: Frontmatter =
                serde_json::from_str(&frontmatter_json).unwrap_or_else(|_| Frontmatter {
                    title: "Unknown".to_string(),
                    categories: vec![],
                    tags: vec![],
                    quality_score: 1,
                    created_at: "unknown".to_string(),
                    session_id: "unknown".to_string(),
                    content_type: "unknown".to_string(),
                    related: None,
                    entities: None,
                });

            Ok(WikiPage {
                id: row.get(0)?,
                title: row.get(1)?,
                frontmatter,
                content: row.get(3)?,
                references: vec![],
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(pages)
}

/// Get high-quality wiki pages
pub fn get_high_quality_pages(
    conn: &Connection,
    threshold: u8,
    limit: usize,
) -> Result<Vec<WikiPage>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, frontmatter, content
         FROM wiki_pages
         WHERE quality_score >= ?1
         ORDER BY quality_score DESC
         LIMIT ?2",
    )?;

    let pages = stmt
        .query_map(params![threshold as i32, limit as i64], |row| {
            let frontmatter_json: String = row.get(2)?;
            let frontmatter: Frontmatter =
                serde_json::from_str(&frontmatter_json).unwrap_or_else(|_| Frontmatter {
                    title: "Unknown".to_string(),
                    categories: vec![],
                    tags: vec![],
                    quality_score: 1,
                    created_at: "unknown".to_string(),
                    session_id: "unknown".to_string(),
                    content_type: "unknown".to_string(),
                    related: None,
                    entities: None,
                });

            Ok(WikiPage {
                id: row.get(0)?,
                title: row.get(1)?,
                frontmatter,
                content: row.get(3)?,
                references: vec![],
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::{QualityMetrics, QualityScore, QualityScorer};

    #[test]
    fn test_extract_title() {
        let gen = WikiGenerator::new();
        let messages = vec![crate::db::SearchResult {
            session_id: "test".to_string(),
            project: "test".to_string(),
            role: "user".to_string(),
            content: "How do I implement async error handling?".to_string(),
            timestamp: Some("2024-01-01".to_string()),
        }];

        let title = gen.extract_title(&messages);
        assert_eq!(title, "How do I implement async error handling?");
    }

    #[test]
    fn test_clean_content_removes_tool_use() {
        let gen = WikiGenerator::new();
        let messages = vec![crate::db::SearchResult {
            session_id: "test".to_string(),
            project: "test".to_string(),
            role: "assistant".to_string(),
            content: "Here is the solution\n[tool_use: Edit] {...}\nDone.".to_string(),
            timestamp: Some("2024-01-01".to_string()),
        }];

        let cleaned = gen.clean_content(&messages);
        assert!(!cleaned.contains("[tool_use:"));
        assert!(cleaned.contains("Here is the solution"));
    }

    #[test]
    fn test_detect_references() {
        let gen = WikiGenerator::new();
        let content = "See also session 47cf1f2e for more details.";
        let refs = gen.detect_references(content);
        assert_eq!(refs, vec!["47cf1f2e"]);
    }

    #[test]
    fn test_render_markdown() {
        let gen = WikiGenerator::new();
        let page = WikiPage {
            id: "test".to_string(),
            title: "Test".to_string(),
            frontmatter: Frontmatter {
                title: "Test".to_string(),
                categories: vec!["Technology".to_string()],
                tags: vec!["rust".to_string()],
                quality_score: 4,
                created_at: "2024-01-01".to_string(),
                session_id: "test".to_string(),
                content_type: "conversation".to_string(),
                related: None,
                entities: None,
            },
            content: "This is test content.".to_string(),
            references: vec![],
        };

        let md = gen.render_markdown(&page);
        assert!(md.starts_with("---\n"));
        assert!(md.contains("title: Test"));
        assert!(md.contains("This is test content."));
    }

    #[test]
    fn test_frontmatter_serialization() {
        let fm = Frontmatter {
            title: "Test".to_string(),
            categories: vec!["Tech".to_string()],
            tags: vec!["rust".to_string()],
            quality_score: 4,
            created_at: "2024-01-01".to_string(),
            session_id: "abc".to_string(),
            content_type: "conversation".to_string(),
            related: None,
            entities: None,
        };

        let yaml = serde_yaml::to_string(&fm).unwrap();
        assert!(yaml.contains("title: Test"));
        assert!(yaml.contains("quality_score: 4"));
    }
}
