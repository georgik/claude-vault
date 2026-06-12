//! Ingest pipeline for end-to-end wiki generation
//!
//! Ties all phases together: import → categorize → score → wiki → entities → synthesis → lint

use anyhow::Result;
use rusqlite::Connection;

/// Pipeline configuration
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub min_quality: u8,
    pub deduplicate: bool,
    pub fix_formatting: bool,
    pub generate_synthesis: bool,
    pub synthesis_limit: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            min_quality: 3,
            deduplicate: true,
            fix_formatting: true,
            generate_synthesis: true,
            synthesis_limit: 10,
        }
    }
}

/// Pipeline result statistics
#[derive(Debug, Clone)]
pub struct PipelineStats {
    pub sessions_processed: usize,
    pub wiki_pages_generated: usize,
    pub entities_extracted: usize,
    pub synthesis_pages: usize,
    pub lint_issues: usize,
    pub duplicates_removed: usize,
}

/// Wiki generation pipeline
pub struct WikiPipeline;

impl WikiPipeline {
    pub fn new() -> Self {
        Self
    }

    /// Run the full pipeline
    pub fn run(&self, conn: &Connection, config: &PipelineConfig) -> Result<PipelineStats> {
        println!("Starting wiki generation pipeline...");

        // Initialize all tables
        self.init_tables(conn)?;

        // Get all sessions
        let session_ids = self.get_all_sessions(conn)?;
        let total_sessions = session_ids.len();

        println!("Processing {} sessions...", total_sessions);

        let mut wiki_count = 0;
        let entity_count;

        // Process each session
        for (i, session_id) in session_ids.iter().enumerate() {
            println!(
                "[{}/{}] Processing {}...",
                i + 1,
                total_sessions,
                &session_id[..8.min(session_id.len())]
            );

            // Categorize
            if let Err(e) = self.categorize_session(conn, session_id) {
                eprintln!("  Categorization failed: {}", e);
            }

            // Score quality
            let quality_score = match self.score_session(conn, session_id) {
                Ok(score) => score,
                Err(e) => {
                    eprintln!("  Scoring failed: {}", e);
                    continue;
                }
            };

            // Skip low-quality sessions
            if quality_score < config.min_quality {
                println!(
                    "  Skipped (quality {} < {})",
                    quality_score, config.min_quality
                );
                continue;
            }

            // Generate wiki page
            if let Err(e) = self.generate_wiki_page(conn, session_id) {
                eprintln!("  Wiki generation failed: {}", e);
                continue;
            }
            wiki_count += 1;
        }

        println!("\nGenerated {} wiki pages", wiki_count);

        // Extract entities
        println!("Extracting entities...");
        entity_count = self.extract_entities(conn)?;
        println!("Extracted {} entities", entity_count);

        // Generate synthesis pages
        let synth_count = if config.generate_synthesis {
            println!("Generating synthesis pages...");
            self.generate_synthesis(conn, config.synthesis_limit)?
        } else {
            0
        };
        println!("Generated {} synthesis pages", synth_count);

        // Lint and cleanup
        let lint_issues = self.lint_wiki(conn)?;

        // Deduplicate if requested
        let dup_count = if config.deduplicate {
            println!("Deduplicating wiki pages...");
            self.deduplicate_wiki(conn)?
        } else {
            0
        };

        // Fix formatting if requested
        if config.fix_formatting {
            println!("Fixing formatting issues...");
            self.fix_formatting(conn)?;
        }

        Ok(PipelineStats {
            sessions_processed: total_sessions,
            wiki_pages_generated: wiki_count,
            entities_extracted: entity_count,
            synthesis_pages: synth_count,
            lint_issues,
            duplicates_removed: dup_count,
        })
    }

    fn init_tables(&self, conn: &Connection) -> Result<()> {
        crate::categorize::init_categories(conn)?;
        crate::categorize::init_message_categories(conn)?;
        crate::quality::init_quality_tables(conn)?;
        crate::wiki::init_wiki_tables(conn)?;
        crate::entities::init_entity_tables(conn)?;
        crate::synthesis::init_synthesis_tables(conn)?;
        crate::lint::init_lint_tables(conn)?;
        Ok(())
    }

    fn get_all_sessions(&self, conn: &Connection) -> Result<Vec<String>> {
        let mut stmt = conn.prepare("SELECT DISTINCT session_id FROM sessions")?;
        let sessions = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    fn categorize_session(&self, conn: &Connection, session_id: &str) -> Result<()> {
        let categorizer = crate::categorize::Categorizer::new();
        let result = categorizer.categorize_session(conn, session_id)?;
        crate::categorize::save_category_result(conn, session_id, &result)?;
        Ok(())
    }

    fn score_session(&self, conn: &Connection, session_id: &str) -> Result<u8> {
        let scorer = crate::quality::QualityScorer::new();
        let result = scorer.score_session(conn, session_id)?;
        crate::quality::update_session_quality(conn, session_id)?;
        Ok(result.score.value())
    }

    fn generate_wiki_page(&self, conn: &Connection, session_id: &str) -> Result<()> {
        let scorer = crate::quality::QualityScorer::new();
        let quality_result = scorer.score_session(conn, session_id)?;

        let categorizer = crate::categorize::Categorizer::new();
        let cat_result = categorizer.categorize_session(conn, session_id)?;

        let generator = crate::wiki::WikiGenerator::new();
        let page = generator.generate_page(
            conn,
            session_id,
            &quality_result,
            &[cat_result.primary_category, cat_result.subcategory],
            &cat_result.tags,
        )?;

        crate::wiki::save_wiki_page(conn, &page)?;
        Ok(())
    }

    fn extract_entities(&self, conn: &Connection) -> Result<usize> {
        let extractor = crate::entities::EntityExtractor::new();
        let mut stmt = conn.prepare("SELECT id, content FROM wiki_pages")?;

        let pages: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut total = 0;

        for (wiki_id, content) in pages {
            let entities = extractor.extract(&content);
            for entity in &entities {
                let entity_id = crate::entities::save_entity(conn, entity)?;
                crate::entities::link_entity_to_wiki(conn, &wiki_id, entity_id, entity.count)?;
                total += 1;
            }
        }

        Ok(total)
    }

    fn generate_synthesis(&self, conn: &Connection, limit: usize) -> Result<usize> {
        let syntheses = crate::synthesis::SynthesisGenerator::generate_top_syntheses(conn, limit)?;

        for synth in &syntheses {
            crate::synthesis::save_synthesis_page(conn, synth)?;
        }

        Ok(syntheses.len())
    }

    fn lint_wiki(&self, conn: &Connection) -> Result<usize> {
        let linter = crate::lint::WikiLinter::new();
        let results = linter.lint_all(conn)?;

        for result in &results {
            crate::lint::save_lint_result(conn, result)?;
        }

        Ok(results.len())
    }

    fn deduplicate_wiki(&self, conn: &Connection) -> Result<usize> {
        let linter = crate::lint::WikiLinter::new();
        let to_remove = linter.deduplicate(conn)?;

        for id in &to_remove {
            conn.execute("DELETE FROM wiki_pages WHERE id = ?1", [id])?;
        }

        Ok(to_remove.len())
    }

    fn fix_formatting(&self, conn: &Connection) -> Result<usize> {
        let linter = crate::lint::WikiLinter::new();
        linter.fix_formatting(conn)
    }

    /// Print pipeline statistics
    pub fn print_stats(&self, stats: &PipelineStats) {
        println!("\n=== Pipeline Results ===");
        println!("Sessions processed: {}", stats.sessions_processed);
        println!("Wiki pages generated: {}", stats.wiki_pages_generated);
        println!("Entities extracted: {}", stats.entities_extracted);
        println!("Synthesis pages: {}", stats.synthesis_pages);
        println!("Lint issues: {}", stats.lint_issues);
        println!("Duplicates removed: {}", stats.duplicates_removed);
    }
}

/// Run quick pipeline (score + wiki only)
pub fn run_quick_pipeline(conn: &Connection) -> Result<PipelineStats> {
    println!("Running quick pipeline (score + wiki only)...");

    crate::quality::init_quality_tables(conn)?;
    crate::wiki::init_wiki_tables(conn)?;

    let mut stmt = conn.prepare("SELECT DISTINCT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut wiki_count = 0;
    let total_sessions = session_ids.len();

    for session_id in session_ids {
        let scorer = crate::quality::QualityScorer::new();
        if scorer.score_session(conn, &session_id).is_ok() {
            crate::quality::update_session_quality(conn, &session_id)?;
        }

        let generator = crate::wiki::WikiGenerator::new();
        if generator
            .generate_page(
                conn,
                &session_id,
                &crate::quality::QualityResult {
                    score: crate::quality::QualityScore(3),
                    reasoning: "Quick".to_string(),
                    metrics: crate::quality::QualityMetrics {
                        code_blocks: 0,
                        resolution_count: 0,
                        technical_depth: 3,
                        actionability: 3,
                        explanation_quality: 3,
                    },
                },
                &["General".to_string()],
                &["wiki".to_string()],
            )
            .is_ok()
        {
            wiki_count += 1;
        }
    }

    Ok(PipelineStats {
        sessions_processed: total_sessions,
        wiki_pages_generated: wiki_count,
        entities_extracted: 0,
        synthesis_pages: 0,
        lint_issues: 0,
        duplicates_removed: 0,
    })
}

/// Run pipeline for a themed wiki with keyword filtering
pub fn run_themed_pipeline(
    vault_conn: &Connection,
    theme_id: &str,
    keywords: &[String],
    config: &PipelineConfig,
) -> Result<PipelineStats> {
    println!("Starting themed wiki generation for '{}'...", theme_id);

    // Get theme wiki path
    let theme = crate::db::get_theme(theme_id)?;
    let wiki_path = std::path::PathBuf::from(&theme.wiki_db);

    // Open or create theme wiki database
    let wiki_conn = crate::db::open_db(&wiki_path)?;

    // Initialize all tables in wiki database
    crate::categorize::init_categories(&wiki_conn)?;
    crate::categorize::init_message_categories(&wiki_conn)?;
    crate::quality::init_quality_tables(&wiki_conn)?;
    crate::wiki::init_wiki_tables(&wiki_conn)?;
    crate::entities::init_entity_tables(&wiki_conn)?;
    crate::synthesis::init_synthesis_tables(&wiki_conn)?;
    crate::lint::init_lint_tables(&wiki_conn)?;

    // Filter sessions by keywords
    let session_ids = if keywords.is_empty() {
        crate::db::filter_sessions_by_keywords(vault_conn, &[])?
    } else {
        println!("Filtering sessions by keywords: {}", keywords.join(", "));
        crate::db::filter_sessions_by_keywords(vault_conn, keywords)?
    };

    let total_sessions = session_ids.len();
    println!(
        "Processing {} sessions matching theme keywords...",
        total_sessions
    );

    if total_sessions == 0 {
        println!("No sessions found matching theme keywords.");
        return Ok(PipelineStats {
            sessions_processed: 0,
            wiki_pages_generated: 0,
            entities_extracted: 0,
            synthesis_pages: 0,
            lint_issues: 0,
            duplicates_removed: 0,
        });
    }

    let mut wiki_count = 0;

    // Process each session
    for (i, session_id) in session_ids.iter().enumerate() {
        println!(
            "[{}/{}] Processing {}...",
            i + 1,
            total_sessions,
            &session_id[..8.min(session_id.len())]
        );

        // Get messages from vault
        let messages = crate::db::get_session_messages_for_wiki(vault_conn, session_id)?;

        if messages.is_empty() {
            println!("  Skipped (no messages)");
            continue;
        }

        // Categorize
        let categorizer = crate::categorize::Categorizer::new();
        let cat_result = match categorizer.categorize_session_vault(vault_conn, session_id) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  Categorization failed: {}", e);
                continue;
            }
        };

        // Score quality
        let scorer = crate::quality::QualityScorer::new();
        let quality_result = match scorer.score_session_vault(vault_conn, session_id) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  Scoring failed: {}", e);
                continue;
            }
        };

        // Skip low-quality sessions
        if quality_result.score.value() < config.min_quality {
            println!(
                "  Skipped (quality {} < {})",
                quality_result.score.value(),
                config.min_quality
            );
            continue;
        }

        // Generate and save wiki page to themed wiki
        let generator = crate::wiki::WikiGenerator::new();
        let page = generator.generate_page_from_messages(
            &messages,
            session_id,
            &quality_result,
            &[cat_result.primary_category, cat_result.subcategory],
            &cat_result.tags,
        )?;

        crate::wiki::save_wiki_page(&wiki_conn, &page)?;
        wiki_count += 1;
    }

    println!("\nGenerated {} wiki pages", wiki_count);

    // Extract entities
    println!("Extracting entities...");
    let entity_count = extract_entities_from_wiki(&wiki_conn)?;
    println!("Extracted {} entities", entity_count);

    // Generate synthesis pages
    let synth_count = if config.generate_synthesis {
        println!("Generating synthesis pages...");
        generate_synthesis_from_wiki(&wiki_conn, config.synthesis_limit)?
    } else {
        0
    };
    println!("Generated {} synthesis pages", synth_count);

    // Lint and cleanup
    let lint_issues = lint_wiki_db(&wiki_conn)?;

    // Deduplicate if requested
    let dup_count = if config.deduplicate {
        println!("Deduplicating wiki pages...");
        deduplicate_wiki_db(&wiki_conn)?
    } else {
        0
    };

    // Fix formatting if requested
    if config.fix_formatting {
        println!("Fixing formatting issues...");
        fix_formatting_wiki_db(&wiki_conn)?;
    }

    Ok(PipelineStats {
        sessions_processed: total_sessions,
        wiki_pages_generated: wiki_count,
        entities_extracted: entity_count,
        synthesis_pages: synth_count,
        lint_issues,
        duplicates_removed: dup_count,
    })
}

/// Incrementally update themed wiki with a single session
/// Accepts either session_id (or "latest" for global most recent) or project (resolves to that project's current session)
pub fn increment_themed_wiki(
    vault_conn: &Connection,
    theme_id: &str,
    session_id: Option<&str>,
    project: Option<&str>,
) -> Result<PipelineStats> {
    // Resolve session_id: explicit, "latest", or via project
    let resolved_session_id = match (session_id, project) {
        (Some(sid), None) => {
            if sid == "latest" {
                println!("Resolving 'latest' to most recent global session...");
                crate::db::get_latest_session(vault_conn, None)?
            } else {
                sid.to_string()
            }
        }
        (None, Some(proj)) => {
            println!("Resolving project '{}' to current session...", proj);
            crate::db::get_latest_session(vault_conn, Some(proj))?
        }
        (Some(sid), Some(_proj)) => {
            println!("Warning: both session_id and project provided, using session_id");
            if sid == "latest" {
                crate::db::get_latest_session(vault_conn, None)?
            } else {
                sid.to_string()
            }
        }
        (None, None) => {
            anyhow::bail!("Either session_id or project must be provided");
        }
    };

    println!(
        "Incrementally updating themed wiki '{}' with session {}...",
        theme_id,
        &resolved_session_id[..8.min(resolved_session_id.len())]
    );

    // Get theme wiki path
    let theme = crate::db::get_theme(theme_id)?;
    let wiki_path = std::path::PathBuf::from(&theme.wiki_db);

    // Open existing wiki database
    let wiki_conn = crate::db::open_db(&wiki_path)?;

    // Get messages from vault
    let messages = crate::db::get_session_messages_for_wiki(vault_conn, &resolved_session_id)?;

    if messages.is_empty() {
        println!("No messages found for session.");
        return Ok(PipelineStats {
            sessions_processed: 0,
            wiki_pages_generated: 0,
            entities_extracted: 0,
            synthesis_pages: 0,
            lint_issues: 0,
            duplicates_removed: 0,
        });
    }

    // Categorize
    let categorizer = crate::categorize::Categorizer::new();
    let cat_result = categorizer.categorize_session_vault(vault_conn, &resolved_session_id)?;

    // Score quality
    let scorer = crate::quality::QualityScorer::new();
    let quality_result = scorer.score_session_vault(vault_conn, &resolved_session_id)?;

    // Generate and save wiki page
    let generator = crate::wiki::WikiGenerator::new();
    let page = generator.generate_page_from_messages(
        &messages,
        &resolved_session_id,
        &quality_result,
        &[cat_result.primary_category, cat_result.subcategory],
        &cat_result.tags,
    )?;

    crate::wiki::save_wiki_page(&wiki_conn, &page)?;

    println!("Generated 1 wiki page");

    // Extract entities for this page
    let extractor = crate::entities::EntityExtractor::new();
    let entities = extractor.extract(&page.content);
    let mut entity_count = 0;
    for entity in &entities {
        let entity_id = crate::entities::save_entity(&wiki_conn, entity)?;
        crate::entities::link_entity_to_wiki(&wiki_conn, &page.id, entity_id, entity.count)?;
        entity_count += 1;
    }
    println!("Extracted {} entities", entity_count);

    Ok(PipelineStats {
        sessions_processed: 1,
        wiki_pages_generated: 1,
        entities_extracted: entity_count,
        synthesis_pages: 0,
        lint_issues: 0,
        duplicates_removed: 0,
    })
}

fn extract_entities_from_wiki(conn: &Connection) -> Result<usize> {
    let extractor = crate::entities::EntityExtractor::new();
    let mut stmt = conn.prepare("SELECT id, content FROM wiki_pages")?;

    let pages: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut total = 0;

    for (wiki_id, content) in pages {
        let entities = extractor.extract(&content);
        for entity in &entities {
            let entity_id = crate::entities::save_entity(conn, entity)?;
            crate::entities::link_entity_to_wiki(conn, &wiki_id, entity_id, entity.count)?;
            total += 1;
        }
    }

    Ok(total)
}

fn generate_synthesis_from_wiki(conn: &Connection, limit: usize) -> Result<usize> {
    let syntheses = crate::synthesis::SynthesisGenerator::generate_top_syntheses(conn, limit)?;

    for synth in &syntheses {
        crate::synthesis::save_synthesis_page(conn, synth)?;
    }

    Ok(syntheses.len())
}

fn lint_wiki_db(conn: &Connection) -> Result<usize> {
    let linter = crate::lint::WikiLinter::new();
    let results = linter.lint_all(conn)?;

    for result in &results {
        crate::lint::save_lint_result(conn, result)?;
    }

    Ok(results.len())
}

fn deduplicate_wiki_db(conn: &Connection) -> Result<usize> {
    let linter = crate::lint::WikiLinter::new();
    let to_remove = linter.deduplicate(conn)?;

    for id in &to_remove {
        conn.execute("DELETE FROM wiki_pages WHERE id = ?1", [id])?;
    }

    Ok(to_remove.len())
}

fn fix_formatting_wiki_db(conn: &Connection) -> Result<usize> {
    let linter = crate::lint::WikiLinter::new();
    linter.fix_formatting(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PipelineConfig::default();
        assert_eq!(config.min_quality, 3);
        assert!(config.deduplicate);
    }

    #[test]
    fn test_custom_config() {
        let config = PipelineConfig {
            min_quality: 4,
            deduplicate: false,
            fix_formatting: false,
            generate_synthesis: false,
            synthesis_limit: 5,
        };
        assert_eq!(config.min_quality, 4);
        assert!(!config.deduplicate);
    }

    #[test]
    fn test_pipeline_stats() {
        let stats = PipelineStats {
            sessions_processed: 10,
            wiki_pages_generated: 8,
            entities_extracted: 15,
            synthesis_pages: 3,
            lint_issues: 2,
            duplicates_removed: 1,
        };
        assert_eq!(stats.sessions_processed, 10);
        assert_eq!(stats.wiki_pages_generated, 8);
    }
}
