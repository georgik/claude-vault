//! Categorization engine for wiki generation
//!
//! Auto-discovers categories from conversation content without predefined taxonomy.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db::get_session_messages;

/// Result of categorizing a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryResult {
    pub title: String,
    pub primary_category: String,
    pub subcategory: String,
    pub tags: Vec<String>,
    pub content_type: ContentType,
    pub confidence: f32,
}

/// Type of content in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Coding,
    Discussion,
    Planning,
    Debugging,
    Explanation,
    Architecture,
    Configuration,
    Other(String),
}

/// Category in the taxonomy
#[derive(Debug, Clone)]
pub struct Category {
    #[expect(dead_code)]
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    #[expect(dead_code)]
    pub description: String,
    pub document_count: i64,
}

/// Categorizer analyzes content and discovers categories
pub struct Categorizer {
    // For now, rule-based. Can be extended with LLM integration.
}

impl Categorizer {
    pub fn new() -> Self {
        Self {}
    }

    /// Analyze a session and extract categories
    pub fn categorize_session(
        &self,
        conn: &Connection,
        session_id: &str,
    ) -> Result<CategoryResult> {
        let messages = get_session_messages(conn, session_id)?;

        if messages.is_empty() {
            anyhow::bail!("Session {} has no messages", session_id);
        }

        // Combine all user messages for analysis
        let combined_content: String = messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        if combined_content.len() < 50 {
            anyhow::bail!("Not enough content to categorize");
        }

        Ok(self.categorize_content(&combined_content))
    }

    /// Analyze content and discover categories (rule-based for now)
    fn categorize_content(&self, content: &str) -> CategoryResult {
        let content_lower = content.to_lowercase();

        // Detect content type
        let content_type = self.detect_content_type(&content_lower);

        // Discover primary category
        let (primary_category, subcategory) = self.discover_categories(&content_lower);

        // Extract tags
        let tags = self.extract_tags(&content_lower);

        // Generate title
        let title = self.generate_title(content, &primary_category, &subcategory);

        CategoryResult {
            title,
            primary_category,
            subcategory,
            tags,
            content_type,
            confidence: 0.7, // Rule-based confidence
        }
    }

    fn detect_content_type(&self, content: &str) -> ContentType {
        if content.contains("fn ") || content.contains("function(") || content.contains("class ") {
            return ContentType::Coding;
        }
        if content.contains("plan") || content.contains("implement") || content.contains("design") {
            return ContentType::Planning;
        }
        if content.contains("bug") || content.contains("error") || content.contains("fix") {
            return ContentType::Debugging;
        }
        if content.contains("architecture")
            || content.contains("structure")
            || content.contains("pattern")
        {
            return ContentType::Architecture;
        }
        if content.contains("config") || content.contains("setting") || content.contains(".json") {
            return ContentType::Configuration;
        }
        if content.contains("explain")
            || content.contains("how does")
            || content.contains("what is")
        {
            return ContentType::Explanation;
        }
        ContentType::Discussion
    }

    fn discover_categories(&self, content: &str) -> (String, String) {
        // Technology-related
        if content.contains("rust") || content.contains("cargo") || content.contains("tokio") {
            if content.contains("async") || content.contains("await") {
                return ("Technology".to_string(), "Rust Async".to_string());
            }
            return ("Technology".to_string(), "Rust".to_string());
        }

        // Database-related
        if content.contains("sql") || content.contains("database") || content.contains("query") {
            return ("Technology".to_string(), "Database".to_string());
        }

        // Web-related
        if content.contains("http") || content.contains("api") || content.contains("endpoint") {
            return ("Technology".to_string(), "Web API".to_string());
        }

        // Testing
        if content.contains("test") || content.contains("spec") || content.contains("assert") {
            return ("Development".to_string(), "Testing".to_string());
        }

        // DevOps
        if content.contains("deploy") || content.contains("ci/cd") || content.contains("docker") {
            return ("Development".to_string(), "DevOps".to_string());
        }

        // Architecture
        if content.contains("pattern")
            || content.contains("design")
            || content.contains("structure")
        {
            return ("Architecture".to_string(), "Design Patterns".to_string());
        }

        // Default
        ("General".to_string(), "Discussion".to_string())
    }

    fn extract_tags(&self, content: &str) -> Vec<String> {
        let mut tags = Vec::new();

        // Common technology tags
        let tech_tags = [
            "rust",
            "python",
            "javascript",
            "typescript",
            "go",
            "java",
            "sql",
            "postgresql",
            "mysql",
            "sqlite",
            "redis",
            "docker",
            "kubernetes",
            "aws",
            "gcp",
            "tokio",
            "async",
            "serde",
            "clap",
            "react",
            "vue",
            "angular",
            "svelte",
            "nodejs",
            "deno",
            "bun",
        ];

        for tag in tech_tags {
            if content.contains(tag) {
                tags.push(tag.to_string());
            }
        }

        // Concept tags
        if content.contains("error") || content.contains("handling") {
            tags.push("error-handling".to_string());
        }
        if content.contains("auth") || content.contains("login") || content.contains("security") {
            tags.push("security".to_string());
        }
        if content.contains("performance") || content.contains("optimize") {
            tags.push("performance".to_string());
        }

        tags
    }

    fn generate_title(&self, content: &str, _primary: &str, subcategory: &str) -> String {
        // Take first meaningful line as title base
        let first_line = content
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .next()
            .unwrap_or("Untitled");

        // Truncate if too long
        let title_base = if first_line.len() > 60 {
            format!("{}...", &first_line[..57])
        } else {
            first_line.to_string()
        };

        format!("{} - {}", title_base, subcategory)
    }
}

/// Initialize categories table in database
pub fn init_categories(conn: &Connection) -> Result<()> {
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
        ",
    )?;

    Ok(())
}

/// Initialize message_categories junction table
pub fn init_message_categories(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
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

    Ok(())
}

/// Save category result to database
pub fn save_category_result(
    conn: &Connection,
    session_id: &str,
    result: &CategoryResult,
) -> Result<i64> {
    // Get or create primary category
    let primary_id = get_or_create_category(conn, &result.primary_category, None, "")?;

    // Get or create subcategory
    let sub_id = get_or_create_category(
        conn,
        &result.subcategory,
        Some(primary_id),
        &result.primary_category,
    )?;

    // Store tags in a simple way (for now, just JSON)
    let tags_json = serde_json::to_string(&result.tags)?;

    // Store as metadata (we could have a tags table, but keep it simple)
    conn.execute(
        "INSERT INTO categories (name, description) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET description = excluded.description",
        params![format!("{}:tags", session_id), tags_json],
    )?;

    Ok(sub_id)
}

/// Get existing category or create new one
fn get_or_create_category(
    conn: &Connection,
    name: &str,
    parent_id: Option<i64>,
    description: &str,
) -> Result<i64> {
    // Try to get existing
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM categories WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        Ok(id)
    } else {
        // Create new
        conn.execute(
            "INSERT INTO categories (name, parent_id, description) VALUES (?1, ?2, ?3)",
            params![name, parent_id, description],
        )?;
        Ok(conn.last_insert_rowid())
    }
}

/// Get all categories as a tree
pub fn get_category_tree(conn: &Connection) -> Result<Vec<Category>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, parent_id, description, document_count
         FROM categories
         ORDER BY name",
    )?;

    let categories = stmt
        .query_map([], |row| {
            Ok(Category {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
                description: row.get(3)?,
                document_count: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(categories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_test_db() -> (Connection, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = crate::db::open_db(tmp.path()).unwrap();
        init_categories(&conn).unwrap();
        init_message_categories(&conn).unwrap();
        (conn, tmp)
    }

    #[test]
    fn test_categorizer_detect_content_type() {
        let categorizer = Categorizer::new();

        assert!(matches!(
            categorizer.detect_content_type("fn main() { println!(\"hello\"); }"),
            ContentType::Coding
        ));

        assert!(matches!(
            categorizer.detect_content_type("plan the implementation"),
            ContentType::Planning
        ));

        assert!(matches!(
            categorizer.detect_content_type("fix the bug in error handling"),
            ContentType::Debugging
        ));
    }

    #[test]
    fn test_discover_categories() {
        let categorizer = Categorizer::new();

        let (primary, sub) = categorizer.discover_categories("rust async await tokio");
        assert_eq!(primary, "Technology");
        assert_eq!(sub, "Rust Async");

        let (primary, sub) = categorizer.discover_categories("sql database query");
        assert_eq!(primary, "Technology");
        assert_eq!(sub, "Database");
    }

    #[test]
    fn test_extract_tags() {
        let categorizer = Categorizer::new();
        let tags = categorizer.extract_tags("rust tokio async error handling");

        assert!(tags.contains(&"rust".to_string()));
        assert!(tags.contains(&"tokio".to_string()));
        assert!(tags.contains(&"async".to_string()));
        assert!(tags.contains(&"error-handling".to_string()));
    }

    #[test]
    fn test_category_crud() {
        let (conn, _tmp) = setup_test_db();

        // Create category
        let id = get_or_create_category(&conn, "Technology", None, "Tech topics").unwrap();
        assert!(id > 0);

        // Should return same ID on second call
        let id2 = get_or_create_category(&conn, "Technology", None, "Tech topics").unwrap();
        assert_eq!(id, id2);

        // Create child category
        let child_id = get_or_create_category(&conn, "Rust", Some(id), "Rust language").unwrap();
        assert!(child_id > 0);
    }

    #[test]
    fn test_get_category_tree() {
        let (conn, _tmp) = setup_test_db();

        // Create some categories
        let parent = get_or_create_category(&conn, "Technology", None, "").unwrap();
        get_or_create_category(&conn, "Rust", Some(parent), "").unwrap();

        let tree = get_category_tree(&conn).unwrap();
        // Should have Technology and Rust (tags are stored separately)
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn test_categorize_content_minimal() {
        let categorizer = Categorizer::new();
        let result = categorizer.categorize_content("rust async code example");

        assert_eq!(result.primary_category, "Technology");
        assert_eq!(result.subcategory, "Rust Async");
        assert!(result.tags.contains(&"rust".to_string()));
        assert!(result.tags.contains(&"async".to_string()));
    }
}
