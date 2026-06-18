//! Synthesis page generation
//!
//! Combines related wiki pages into summary pages by topic/entity.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// Synthesis page combining related content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisPage {
    pub id: String,
    pub title: String,
    pub topic: String,
    pub sources: Vec<String>,
    pub summary: String,
    pub key_points: Vec<String>,
    pub entities: Vec<String>,
    pub related_topics: Vec<String>,
}

/// Synthesis generator creates summary pages
pub struct SynthesisGenerator;

impl SynthesisGenerator {
    pub fn new() -> Self {
        Self
    }

    /// Generate synthesis page for an entity/topic
    pub fn generate_synthesis(&self, conn: &Connection, topic: &str) -> Result<SynthesisPage> {
        // Get all wiki pages related to this topic
        let wiki_pages = self.get_pages_by_entity(conn, topic)?;

        if wiki_pages.is_empty() {
            anyhow::bail!("No wiki pages found for topic: {}", topic);
        }

        // Extract content from all pages
        let summaries: Vec<String> = wiki_pages
            .iter()
            .filter_map(|page| self.extract_summary(&page.1))
            .collect();

        // Generate synthesis
        let summary = self.synthesize_summaries(&summaries);
        let key_points = self.extract_key_points(&summaries);
        let entities = self.extract_common_entities(&wiki_pages);
        let related_topics = self.find_related_topics(conn, topic)?;

        let title = format!("{}: Overview", topic);

        Ok(SynthesisPage {
            id: format!("synth_{}", topic.to_lowercase().replace(' ', "_")),
            title,
            topic: topic.to_string(),
            sources: wiki_pages.iter().map(|p| p.0.clone()).collect(),
            summary,
            key_points,
            entities,
            related_topics,
        })
    }

    /// Get wiki pages by entity
    fn get_pages_by_entity(
        &self,
        conn: &Connection,
        entity_name: &str,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = conn.prepare(
            "SELECT wp.id, wp.content
             FROM wiki_pages wp
             JOIN wiki_entities we ON wp.id = we.wiki_id
             JOIN entities e ON we.entity_id = e.id
             WHERE e.name = ?1
             ORDER BY we.count DESC",
        )?;

        let pages = stmt
            .query_map(params![entity_name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(pages)
    }

    /// Extract summary from wiki page content
    fn extract_summary(&self, content: &str) -> Option<String> {
        // Get first meaningful paragraph
        content
            .split("\n\n---\n\n")
            .next()?
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .into()
    }

    /// Synthesize multiple summaries into one
    fn synthesize_summaries(&self, summaries: &[String]) -> String {
        if summaries.is_empty() {
            return "No content available.".to_string();
        }

        let mut synthesized = String::from("## Overview\n\n");
        synthesized.push_str("This topic is covered across multiple sources:\n\n");

        for (i, summary) in summaries.iter().enumerate() {
            synthesized.push_str(&format!("### Source {}\n\n{}\n\n", i + 1, summary));
        }

        synthesized
    }

    /// Extract key points from summaries
    fn extract_key_points(&self, summaries: &[String]) -> Vec<String> {
        let mut points = Vec::new();

        for summary in summaries {
            // Look for bullet points or numbered lists
            for line in summary.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("- ")
                    || trimmed.starts_with("* ")
                    || trimmed.starts_with("1. ")
                    || trimmed.starts_with("2. ")
                {
                    points.push(trimmed.to_string());
                }
            }

            // Look for sentences with "important", "key", "note"
            for line in summary.lines() {
                let lower = line.to_lowercase();
                if lower.contains("important")
                    || lower.contains("key point")
                    || lower.contains("note:")
                {
                    points.push(line.trim().to_string());
                }
            }
        }

        // Deduplicate
        points.sort();
        points.dedup();
        points.truncate(10);

        points
    }

    /// Extract common entities from pages
    fn extract_common_entities(&self, pages: &[(String, String)]) -> Vec<String> {
        let mut entity_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for (_, content) in pages {
            // Simple entity extraction (common terms)
            for word in content.split_whitespace() {
                if word.len() > 4
                    && word
                        .chars()
                        .next()
                        .map(|c| c.is_uppercase())
                        .unwrap_or(false)
                {
                    *entity_counts.entry(word.to_string()).or_insert(0) += 1;
                }
            }
        }

        // Get top entities
        let mut entities: Vec<_> = entity_counts
            .into_iter()
            .filter(|(_, count)| *count >= 2)
            .collect();

        entities.sort_by(|a, b| b.1.cmp(&a.1));
        entities.truncate(10);
        entities.into_iter().map(|(name, _)| name).collect()
    }

    /// Find related topics
    fn find_related_topics(&self, conn: &Connection, topic: &str) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT e.name
             FROM entities e
             JOIN entity_relations er ON e.id = er.to_entity
             JOIN entities e_from ON er.from_entity = e_from.id
             WHERE e_from.name = ?1
             LIMIT 10",
        )?;

        let topics = stmt
            .query_map(params![topic], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(topics)
    }

    /// Render synthesis page as markdown
    #[expect(dead_code)]
    pub fn render_markdown(&self, page: &SynthesisPage) -> String {
        let mut md = format!(
            "# {}\n\n**Topic:** {}\n\n**Sources:** {}\n\n",
            page.title,
            page.topic,
            page.sources.join(", ")
        );

        md.push_str(&page.summary);
        md.push_str("\n\n## Key Points\n\n");

        for point in &page.key_points {
            md.push_str(&format!("{}\n", point));
        }

        if !page.entities.is_empty() {
            md.push_str("\n## Related Entities\n\n");
            for entity in &page.entities {
                md.push_str(&format!("- {}\n", entity));
            }
        }

        if !page.related_topics.is_empty() {
            md.push_str("\n## Related Topics\n\n");
            for topic in &page.related_topics {
                md.push_str(&format!("- {}\n", topic));
            }
        }

        md
    }

    /// Generate synthesis for top entities
    pub fn generate_top_syntheses(conn: &Connection, limit: usize) -> Result<Vec<SynthesisPage>> {
        let mut stmt = conn.prepare(
            "SELECT name, COUNT(*) as page_count
             FROM entities e
             JOIN wiki_entities we ON e.id = we.entity_id
             GROUP BY e.name
             ORDER BY page_count DESC
             LIMIT ?1",
        )?;

        let entities: Vec<String> = stmt
            .query_map(params![limit as i64], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut syntheses = Vec::new();
        let generator = Self::new();

        for entity in entities {
            match generator.generate_synthesis(conn, &entity) {
                Ok(synth) => syntheses.push(synth),
                Err(_) => continue,
            }
        }

        Ok(syntheses)
    }
}

/// Initialize synthesis tables
pub fn init_synthesis_tables(conn: &Connection) -> Result<()> {
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

/// Save synthesis page to database
pub fn save_synthesis_page(conn: &Connection, page: &SynthesisPage) -> Result<()> {
    let key_points_json = serde_json::to_string(&page.key_points)?;
    let entities_json = serde_json::to_string(&page.entities)?;
    let related_json = serde_json::to_string(&page.related_topics)?;
    let sources_json = serde_json::to_string(&page.sources)?;

    conn.execute(
        "INSERT OR REPLACE INTO synthesis_pages
         (id, title, topic, summary, key_points, entities, related_topics, sources)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            page.id,
            page.title,
            page.topic,
            page.summary,
            key_points_json,
            entities_json,
            related_json,
            sources_json,
        ],
    )?;

    Ok(())
}

/// Get synthesis page by topic
#[expect(dead_code)]
pub fn get_synthesis_by_topic(conn: &Connection, topic: &str) -> Result<Option<SynthesisPage>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_summary() {
        let generator = SynthesisGenerator::new();
        let content = "# Title\n\nThis is a summary.\n\nMore content here.";

        let summary = generator.extract_summary(content);
        assert!(summary.is_some());
        assert!(summary.unwrap().contains("summary"));
    }

    #[test]
    fn test_synthesize_summaries() {
        let generator = SynthesisGenerator::new();
        let summaries = vec![
            "First point about the topic.".to_string(),
            "Second point about the topic.".to_string(),
        ];

        let result = generator.synthesize_summaries(&summaries);
        assert!(result.contains("Overview"));
        assert!(result.contains("Source 1"));
        assert!(result.contains("Source 2"));
    }

    #[test]
    fn test_extract_key_points() {
        let generator = SynthesisGenerator::new();
        let summaries = vec![
            "- Important point one\n- Another key point".to_string(),
            "Note: This is crucial information.".to_string(),
        ];

        let points = generator.extract_key_points(&summaries);
        assert!(!points.is_empty());
        assert!(points.iter().any(|p| p.contains("Important")));
    }

    #[test]
    fn test_render_markdown() {
        let generator = SynthesisGenerator::new();
        let page = SynthesisPage {
            id: "test".to_string(),
            title: "Test Overview".to_string(),
            topic: "test".to_string(),
            sources: vec!["abc123".to_string()],
            summary: "Test summary".to_string(),
            key_points: vec!["- Point 1".to_string()],
            entities: vec!["Entity1".to_string()],
            related_topics: vec!["Related".to_string()],
        };

        let md = generator.render_markdown(&page);
        assert!(md.contains("# Test Overview"));
        assert!(md.contains("## Key Points"));
        assert!(md.contains("## Related Entities"));
    }
}
