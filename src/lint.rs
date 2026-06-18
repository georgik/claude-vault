//! Lint operations for wiki quality
//!
//! Cleans up duplicate content, formatting issues, and broken links.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Lint result for a wiki page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResult {
    pub wiki_id: String,
    pub issues: Vec<LintIssue>,
    pub fixed: usize,
}

/// Lint issue type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintIssue {
    DuplicateContent { similar_id: String, similarity: f32 },
    ShortContent { length: usize },
    BrokenLink { link: String },
    EmptySection { section_name: String },
    FormattingIssue { description: String },
}

/// Wiki linter checks and fixes quality issues
pub struct WikiLinter;

impl WikiLinter {
    pub fn new() -> Self {
        Self
    }

    /// Lint all wiki pages
    pub fn lint_all(&self, conn: &Connection) -> Result<Vec<LintResult>> {
        let mut stmt =
            conn.prepare("SELECT id, content FROM wiki_pages ORDER BY quality_score ASC")?;

        let pages: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut results = Vec::new();

        for (wiki_id, content) in pages {
            let issues = self.lint_content(&wiki_id, &content, conn)?;
            if !issues.is_empty() {
                results.push(LintResult {
                    wiki_id,
                    issues,
                    fixed: 0,
                });
            }
        }

        Ok(results)
    }

    /// Lint a single wiki page
    pub fn lint_page(&self, conn: &Connection, wiki_id: &str) -> Result<LintResult> {
        let mut stmt = conn.prepare("SELECT content FROM wiki_pages WHERE id = ?1")?;

        let content: String = stmt.query_row(params![wiki_id], |row| row.get(0))?;

        let issues = self.lint_content(wiki_id, &content, conn)?;

        Ok(LintResult {
            wiki_id: wiki_id.to_string(),
            issues,
            fixed: 0,
        })
    }

    /// Lint content and return issues
    fn lint_content(
        &self,
        wiki_id: &str,
        content: &str,
        conn: &Connection,
    ) -> Result<Vec<LintIssue>> {
        let mut issues = Vec::new();

        // Check for short content
        if content.len() < 100 {
            issues.push(LintIssue::ShortContent {
                length: content.len(),
            });
        }

        // Check for duplicates
        if let Some(duplicate) = self.find_duplicate(wiki_id, content, conn)? {
            issues.push(LintIssue::DuplicateContent {
                similar_id: duplicate,
                similarity: 0.9,
            });
        }

        // Check for broken links
        for link in self.find_links(content) {
            if self.is_broken_link(&link, conn)? {
                issues.push(LintIssue::BrokenLink { link });
            }
        }

        // Check for empty sections
        for section in self.find_empty_sections(content) {
            issues.push(LintIssue::EmptySection {
                section_name: section,
            });
        }

        Ok(issues)
    }

    /// Find duplicate content
    fn find_duplicate(
        &self,
        wiki_id: &str,
        content: &str,
        conn: &Connection,
    ) -> Result<Option<String>> {
        let mut stmt = conn.prepare("SELECT id, content FROM wiki_pages WHERE id != ?1")?;

        let pages: Vec<(String, String)> = stmt
            .query_map(params![wiki_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        // Simple similarity check (word overlap)
        let content_words: std::collections::HashSet<&str> = content.split_whitespace().collect();

        for (id, other_content) in pages {
            let other_words: std::collections::HashSet<&str> =
                other_content.split_whitespace().collect();

            let intersection = content_words.intersection(&other_words).count();
            let union = content_words.union(&other_words).count();

            if union > 0 {
                let similarity = intersection as f32 / union as f32;
                if similarity > 0.8 {
                    return Ok(Some(id));
                }
            }
        }

        Ok(None)
    }

    /// Find all links in content
    fn find_links(&self, content: &str) -> Vec<String> {
        let mut links = Vec::new();

        // Match wiki page references (8-char hex IDs)
        for word in content.split_whitespace() {
            if word.len() == 8 && word.chars().all(|c| c.is_ascii_hexdigit()) {
                links.push(word.to_string());
            }
        }

        links
    }

    /// Check if link is broken (wiki page doesn't exist)
    fn is_broken_link(&self, link: &str, conn: &Connection) -> Result<bool> {
        let mut stmt = conn.prepare("SELECT COUNT(*) FROM wiki_pages WHERE id = ?1")?;

        let count: i64 = stmt.query_row(params![link], |row| row.get(0))?;

        Ok(count == 0)
    }

    /// Find empty sections
    fn find_empty_sections(&self, content: &str) -> Vec<String> {
        let mut empty_sections = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        let mut current_section = "Introduction".to_string();
        let mut section_content = String::new();

        for line in &lines {
            if line.starts_with('#') {
                // Check if previous section was empty
                if section_content.trim().is_empty() && !current_section.is_empty() {
                    empty_sections.push(current_section.clone());
                }
                current_section = line.trim_start_matches('#').trim().to_string();
                section_content.clear();
            } else {
                section_content.push_str(line);
            }
        }

        empty_sections
    }

    /// Deduplicate wiki pages
    pub fn deduplicate(&self, conn: &Connection) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT id, content, quality_score FROM wiki_pages ORDER BY quality_score DESC",
        )?;

        let pages: Vec<(String, String, i32)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut to_remove = Vec::new();
        let mut seen: HashMap<String, (String, i32)> = HashMap::new();

        for (id, content, quality) in pages {
            let content_hash = self.content_hash(&content);

            if let Some((existing_id, existing_quality)) = seen.get(&content_hash) {
                // Keep the one with higher quality
                if quality < *existing_quality {
                    to_remove.push(id);
                } else {
                    to_remove.push(existing_id.clone());
                    seen.insert(content_hash, (id, quality));
                }
            } else {
                seen.insert(content_hash, (id, quality));
            }
        }

        Ok(to_remove)
    }

    /// Generate simple hash of content
    fn content_hash(&self, content: &str) -> String {
        // Normalize for comparison: lowercase, remove extra whitespace
        content
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(100)
            .collect::<String>()
    }

    /// Fix common formatting issues
    pub fn fix_formatting(&self, conn: &Connection) -> Result<usize> {
        let mut stmt = conn.prepare("SELECT id, content FROM wiki_pages")?;

        let pages: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut fixed = 0;

        for (id, content) in pages {
            let cleaned = self.clean_content(&content);
            if cleaned != content {
                conn.execute(
                    "UPDATE wiki_pages SET content = ?1 WHERE id = ?2",
                    params![cleaned, id],
                )?;
                fixed += 1;
            }
        }

        Ok(fixed)
    }

    /// Clean content formatting
    fn clean_content(&self, content: &str) -> String {
        let mut cleaned = content.to_string();

        // Remove excessive blank lines (> 2 consecutive)
        cleaned = cleaned.replace("\n\n\n\n", "\n\n");
        cleaned = cleaned.replace("\n\n\n", "\n\n");

        // Fix spacing around headers
        cleaned = cleaned
            .lines()
            .map(|line| {
                if line.starts_with('#') {
                    line.trim().to_string()
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        cleaned
    }
}

/// Initialize lint tables
pub fn init_lint_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS lint_results (
            wiki_id TEXT NOT NULL,
            issue_type TEXT NOT NULL,
            description TEXT,
            fixed INTEGER DEFAULT 0,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (wiki_id, issue_type)
        );
        ",
    )?;
    Ok(())
}

/// Save lint result
pub fn save_lint_result(conn: &Connection, result: &LintResult) -> Result<()> {
    for issue in &result.issues {
        let (issue_type, description) = match issue {
            LintIssue::DuplicateContent { similar_id, .. } => (
                "duplicate".to_string(),
                format!("Duplicate of {}", similar_id),
            ),
            LintIssue::ShortContent { length } => (
                "short".to_string(),
                format!("Content too short: {} chars", length),
            ),
            LintIssue::BrokenLink { link } => {
                ("broken_link".to_string(), format!("Broken link: {}", link))
            }
            LintIssue::EmptySection { section_name } => (
                "empty_section".to_string(),
                format!("Empty section: {}", section_name),
            ),
            LintIssue::FormattingIssue { description } => {
                ("formatting".to_string(), description.clone())
            }
        };

        conn.execute(
            "INSERT OR REPLACE INTO lint_results (wiki_id, issue_type, description)
             VALUES (?1, ?2, ?3)",
            params![result.wiki_id, issue_type, description],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_links() {
        let linter = WikiLinter::new();
        let content = "See also 47cf1f2e and 38b2a11c for more info.";

        let links = linter.find_links(content);
        assert_eq!(links.len(), 2);
        assert!(links.contains(&"47cf1f2e".to_string()));
    }

    #[test]
    fn test_content_hash() {
        let linter = WikiLinter::new();
        let content1 = "Hello World";
        let content2 = "hello   world"; // Different spacing, same content

        assert_eq!(linter.content_hash(content1), linter.content_hash(content2));
    }

    #[test]
    fn test_clean_content() {
        let linter = WikiLinter::new();
        let messy = "## Header\n\n\n\nText";
        let cleaned = linter.clean_content(messy);

        assert!(!cleaned.contains("\n\n\n"));
    }

    #[test]
    fn test_find_empty_sections() {
        let linter = WikiLinter::new();
        let content = "# Section 1\n\n## Empty\n\n# Section 2\n\nSome content";

        let empty = linter.find_empty_sections(content);
        assert!(empty.contains(&"Empty".to_string()));
    }
}
