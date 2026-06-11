//! Quality scoring for wiki content
//!
//! Scores conversations by value/insight density (1-5 scale).

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::db::get_session_messages;

/// Quality score (1-5 scale)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct QualityScore(pub u8);

impl QualityScore {
    /// Create a new quality score with validation
    #[expect(dead_code)]
    pub fn new(score: u8) -> Option<Self> {
        if (1..=5).contains(&score) {
            Some(QualityScore(score))
        } else {
            None
        }
    }

    /// Get the score value
    pub fn value(&self) -> u8 {
        self.0
    }

    /// Check if this score meets minimum threshold
    #[expect(dead_code)]
    pub fn meets_minimum(&self, minimum: u8) -> bool {
        self.0 >= minimum
    }
}

/// Detailed quality metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityMetrics {
    pub code_blocks: usize,
    pub resolution_count: usize,
    pub technical_depth: u8,
    pub actionability: u8,
    pub explanation_quality: u8,
}

/// Result of quality scoring a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityResult {
    pub score: QualityScore,
    pub reasoning: String,
    pub metrics: QualityMetrics,
}

/// Quality scorer analyzes conversation value
pub struct QualityScorer;

impl QualityScorer {
    pub fn new() -> Self {
        Self
    }

    /// Score a conversation session
    pub fn score_session(&self, conn: &Connection, session_id: &str) -> Result<QualityResult> {
        let messages = get_session_messages(conn, session_id)?;

        if messages.is_empty() {
            return Ok(QualityResult {
                score: QualityScore(1),
                reasoning: "No messages in session".to_string(),
                metrics: QualityMetrics {
                    code_blocks: 0,
                    resolution_count: 0,
                    technical_depth: 1,
                    actionability: 1,
                    explanation_quality: 1,
                },
            });
        }

        // Extract all assistant messages for analysis
        let assistant_messages: Vec<&str> = messages
            .iter()
            .filter(|m| m.role == "assistant")
            .map(|m| m.content.as_str())
            .collect();

        let combined_content = assistant_messages.join("\n");
        let metrics = self.compute_metrics(&combined_content);
        let score = self.compute_score(&metrics, &combined_content);
        let reasoning = self.generate_reasoning(&score, &metrics, &combined_content);

        Ok(QualityResult {
            score,
            reasoning,
            metrics,
        })
    }

    /// Compute quality metrics from content
    fn compute_metrics(&self, content: &str) -> QualityMetrics {
        // Count code blocks
        let code_blocks = content.matches("```").count() / 2;

        // Count resolution indicators
        let content_lower = content.to_lowercase();
        let resolution_indicators = ["fixed", "resolved", "solved", "implemented", "complete"];
        let resolution_count = resolution_indicators
            .iter()
            .map(|indicator| content_lower.matches(indicator).count())
            .sum::<usize>();

        // Assess technical depth
        let technical_depth = self.assess_technical_depth(content);

        // Assess actionability
        let actionability = self.assess_actionability(content);

        // Assess explanation quality
        let explanation_quality = self.assess_explanation_quality(content);

        QualityMetrics {
            code_blocks,
            resolution_count,
            technical_depth,
            actionability,
            explanation_quality,
        }
    }

    /// Assess technical depth (1-5 scale)
    fn assess_technical_depth(&self, content: &str) -> u8 {
        let content_lower = content.to_lowercase();
        let mut score = 1;

        // Technical concepts
        if content_lower.contains("algorithm")
            || content_lower.contains("optimization")
            || content_lower.contains("pattern")
        {
            score += 1;
        }

        // Implementation details
        if content_lower.contains("fn ")
            || content_lower.contains("function(")
            || content_lower.contains("class ")
            || content_lower.contains("impl ")
        {
            score += 1;
        }

        // Advanced topics
        if content_lower.contains("concurrency")
            || content_lower.contains("async")
            || content_lower.contains("memory")
            || content_lower.contains("performance")
        {
            score += 1;
        }

        // Deep technical insight
        if content_lower.contains("architecture")
            || content_lower.contains("design")
            || content_lower.contains("trade-off")
            || content_lower.contains("consideration")
        {
            score += 1;
        }

        score.min(5)
    }

    /// Assess actionability (1-5 scale)
    fn assess_actionability(&self, content: &str) -> u8 {
        let content_lower = content.to_lowercase();
        let mut score = 1;

        // Action indicators
        if content_lower.contains("do this")
            || content_lower.contains("implement")
            || content_lower.contains("add")
            || content_lower.contains("change")
        {
            score += 1;
        }

        // Step-by-step guidance
        if content_lower.contains("first")
            || content_lower.contains("then")
            || content_lower.contains("step")
            || content_lower.contains("next")
        {
            score += 1;
        }

        // Code examples
        if content_lower.contains("```") {
            score += 1;
        }

        // Complete solution
        if content_lower.contains("working")
            || content_lower.contains("complete")
            || content_lower.contains("full")
        {
            score += 1;
        }

        score.min(5)
    }

    /// Assess explanation quality (1-5 scale)
    fn assess_explanation_quality(&self, content: &str) -> u8 {
        let mut score = 1;

        // Length of explanation (indicator of detail)
        let word_count = content.split_whitespace().count();
        if word_count > 50 {
            score += 1;
        }
        if word_count > 200 {
            score += 1;
        }

        // Structural indicators
        if content.contains('\n') && content.lines().count() > 5 {
            score += 1;
        }

        // Explanation indicators
        let content_lower = content.to_lowercase();
        if content_lower.contains("because")
            || content_lower.contains("reason")
            || content_lower.contains("explain")
            || content_lower.contains("note")
        {
            score += 1;
        }

        score.min(5)
    }

    /// Compute overall score from metrics
    fn compute_score(&self, metrics: &QualityMetrics, _content: &str) -> QualityScore {
        let mut score = 1;

        // Base score from metrics
        let avg_metric =
            (metrics.technical_depth + metrics.actionability + metrics.explanation_quality) / 3;

        if avg_metric >= 2 {
            score += 1;
        }
        if avg_metric >= 3 {
            score += 1;
        }
        if avg_metric >= 4 {
            score += 1;
        }

        // Code blocks add value
        if metrics.code_blocks > 0 {
            score += 1;
        }

        // Resolutions add value
        if metrics.resolution_count > 0 {
            score += 1;
        }

        QualityScore(score.min(5))
    }

    /// Generate human-readable reasoning
    fn generate_reasoning(
        &self,
        score: &QualityScore,
        metrics: &QualityMetrics,
        _content: &str,
    ) -> String {
        let mut reasons = Vec::new();

        match score.0 {
            5 => reasons.push("Comprehensive analysis with actionable insights".to_string()),
            4 => reasons.push("Solid implementation with good explanation".to_string()),
            3 => reasons.push("Working solution with adequate documentation".to_string()),
            2 => reasons.push("Partial solution, needs follow-up".to_string()),
            1 => reasons.push("Minimal value or incomplete".to_string()),
            _ => unreachable!(),
        }

        if metrics.code_blocks > 0 {
            reasons.push(format!("Contains {} code block(s)", metrics.code_blocks));
        }

        if metrics.resolution_count > 0 {
            reasons.push(format!(
                "Shows {} resolution indicator(s)",
                metrics.resolution_count
            ));
        }

        if metrics.technical_depth >= 4 {
            reasons.push("High technical depth".to_string());
        }

        if metrics.actionability >= 4 {
            reasons.push("Highly actionable".to_string());
        }

        if metrics.explanation_quality >= 4 {
            reasons.push("Well explained".to_string());
        }

        reasons.join(". ")
    }
}

/// Initialize quality scoring tables
pub fn init_quality_tables(conn: &Connection) -> Result<()> {
    // Check if columns exist, add if not
    let has_quality_score: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'quality_score'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
        > 0;

    if !has_quality_score {
        conn.execute("ALTER TABLE messages ADD COLUMN quality_score INTEGER", [])?;
    }

    let has_avg_quality: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'avg_quality'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
        > 0;

    if !has_avg_quality {
        conn.execute("ALTER TABLE sessions ADD COLUMN avg_quality REAL", [])?;
    }

    Ok(())
}

/// Save quality score for a message
#[expect(dead_code)]
pub fn save_quality_score(conn: &Connection, message_id: i64, score: QualityScore) -> Result<()> {
    conn.execute(
        "UPDATE messages SET quality_score = ?1 WHERE id = ?2",
        params![score.0 as i32, message_id],
    )?;
    Ok(())
}

/// Update session average quality
pub fn update_session_quality(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE sessions SET avg_quality = (
            SELECT AVG(quality_score) FROM messages
            WHERE session_id = ?1 AND quality_score IS NOT NULL
        ) WHERE session_id = ?2",
        params![session_id, session_id],
    )?;
    Ok(())
}

/// Get high-quality sessions (score >= threshold)
pub fn get_high_quality_sessions(
    conn: &Connection,
    threshold: u8,
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT session_id, COALESCE(avg_quality, 0) as avg_q
         FROM sessions
         WHERE avg_quality >= ?1
         ORDER BY avg_quality DESC
         LIMIT ?2",
    )?;

    let results = stmt
        .query_map(params![threshold as i32, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_quality_score_validation() {
        assert!(QualityScore::new(0).is_none());
        assert!(QualityScore::new(1).is_some());
        assert!(QualityScore::new(5).is_some());
        assert!(QualityScore::new(6).is_none());
    }

    #[test]
    fn test_quality_score_meets_minimum() {
        let score = QualityScore(4);
        assert!(score.meets_minimum(3));
        assert!(score.meets_minimum(4));
        assert!(!score.meets_minimum(5));
    }

    #[test]
    fn test_assess_technical_depth() {
        let scorer = QualityScorer::new();

        // Simple content
        let depth1 = scorer.assess_technical_depth("hello world");
        assert_eq!(depth1, 1);

        // With function
        let depth2 = scorer.assess_technical_depth("fn main() { println!(\"hello\"); }");
        assert!(depth2 >= 2);

        // With async
        let depth3 = scorer.assess_technical_depth("async fn process() -> Result<()> { Ok(()) }");
        assert!(depth3 >= 3);
    }

    #[test]
    fn test_assess_actionability() {
        let scorer = QualityScorer::new();

        // Not actionable
        let action1 = scorer.assess_actionability("I see what you mean");
        assert_eq!(action1, 1);

        // Actionable with steps
        let action2 =
            scorer.assess_actionability("First, add this. Then, change that. Step 3: run tests.");
        assert!(action2 >= 3);

        // With code and implementation
        let action3 = scorer.assess_actionability("Implement this:\n```rust\nfn test() {}\n```");
        assert!(action3 >= 3);
    }

    #[test]
    fn test_compute_metrics() {
        let scorer = QualityScorer::new();
        let content = "Here's how to fix the async bug:\n\
            First, add await.\n\
            ```rust\nlet result = async_op().await;\n```\n\
            This resolved the race condition.";

        let metrics = scorer.compute_metrics(content);

        assert_eq!(metrics.code_blocks, 1);
        assert!(metrics.resolution_count > 0);
        assert!(metrics.actionability >= 2);
    }

    #[test]
    fn test_score_computation() {
        let scorer = QualityScorer::new();
        let metrics = QualityMetrics {
            code_blocks: 2,
            resolution_count: 1,
            technical_depth: 4,
            actionability: 4,
            explanation_quality: 3,
        };

        let content = "```fn test() {}``` Fixed the issue";
        let score = scorer.compute_score(&metrics, content);

        assert!(score.0 >= 3);
    }

    #[test]
    fn test_reasoning_generation() {
        let scorer = QualityScorer::new();
        let metrics = QualityMetrics {
            code_blocks: 1,
            resolution_count: 1,
            technical_depth: 4,
            actionability: 4,
            explanation_quality: 4,
        };

        let reasoning = scorer.generate_reasoning(&QualityScore(5), &metrics, "");

        assert!(reasoning.contains("Comprehensive"));
        assert!(reasoning.contains("code"));
    }
}
