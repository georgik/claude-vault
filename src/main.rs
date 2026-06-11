mod categorize;
mod db;
mod entities;
mod import;
mod lint;
mod merge;
mod pipeline;
mod quality;
mod synthesis;
mod wiki;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use rusqlite::params;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "claude-vault")]
#[command(about = "Archive Claude Code conversations into SQLite with FTS5 full-text search")]
#[command(version)]
struct Cli {
    /// Path to the SQLite database file
    #[arg(long, env = "CLAUDE_VAULT_DB")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Import all JSONL files from ~/.claude/projects/
    Import {
        /// Path to Claude config directory (default: ~/.claude)
        #[arg(long)]
        claude_dir: Option<PathBuf>,
    },
    /// Import a single JSONL session file
    ImportFile {
        /// Path to the JSONL file
        path: PathBuf,
        /// Project name
        #[arg(long)]
        project: Option<String>,
    },
    /// Search conversations using full-text search
    Search {
        /// Search query (FTS5 syntax)
        query: String,
        /// Maximum number of results
        #[arg(short, long, default_value = "10")]
        limit: usize,
        /// Filter by project name (substring match)
        #[arg(short, long)]
        project: Option<String>,
        /// Filter by role (user or assistant)
        #[arg(short, long)]
        role: Option<String>,
        /// Only show results after this date (e.g. 2024-01-01)
        #[arg(long)]
        since: Option<String>,
        /// Only show results before this date (e.g. 2024-12-31)
        #[arg(long)]
        until: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Include tool_use lines in results (hidden by default)
        #[arg(long)]
        include_tools: bool,
    },
    /// Export a session in various formats
    Export {
        /// Session ID or prefix (e.g. "47cf1f2e")
        #[arg(required_unless_present = "last")]
        session_id: Option<String>,
        /// Export the Nth most recent session (1 = latest, 2 = second latest, ...)
        #[arg(long, default_missing_value = "1", num_args = 0..=1, value_name = "N")]
        last: Option<usize>,
        /// Output format
        #[arg(short, long, default_value = "markdown")]
        format: ExportFormat,
    },
    /// List sessions
    List {
        /// Number of sessions to show (0 = all)
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
        /// Filter by project name (substring match)
        #[arg(short, long)]
        project: Option<String>,
        /// Only show sessions after this date (e.g. 2024-01-01)
        #[arg(long)]
        since: Option<String>,
        /// Only show sessions before this date (e.g. 2024-12-31)
        #[arg(long)]
        until: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Delete a session and all its messages
    Delete {
        /// Session ID or prefix
        session_id: String,
        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Show database statistics
    Stats,
    /// Verify database integrity
    Verify,
    /// Categorize conversations
    Categorize {
        /// Categorize all conversations
        #[arg(long)]
        all: bool,
        /// Categorize a specific session
        #[arg(long, conflicts_with = "all")]
        session: Option<String>,
        /// Show category tree
        #[arg(long)]
        tree: bool,
    },
    /// Score conversation quality
    Score {
        /// Score all conversations
        #[arg(long)]
        all: bool,
        /// Score a specific session
        #[arg(long, conflicts_with = "all")]
        session: Option<String>,
        /// Show top sessions by quality
        #[arg(long)]
        top: bool,
        /// Minimum quality threshold (1-5)
        #[arg(long, default_value = "3")]
        min: u8,
    },
    /// Wiki generation
    Wiki {
        /// Generate wiki pages for all sessions
        #[arg(long)]
        all: bool,
        /// Generate wiki page for specific session
        #[arg(long, conflicts_with = "all")]
        session: Option<String>,
        /// Show high-quality wiki pages
        #[arg(long)]
        top: bool,
        /// Export wiki page to file
        #[arg(long)]
        export: Option<PathBuf>,
        /// Export all wiki pages to directory
        #[arg(long)]
        export_all: Option<PathBuf>,
        /// Minimum quality threshold (1-5)
        #[arg(long, default_value = "3")]
        min: u8,
    },
    /// Entity extraction
    Entities {
        /// Extract entities from wiki pages
        #[arg(long)]
        extract: bool,
        /// Show entities for specific wiki page
        #[arg(long)]
        wiki: Option<String>,
        /// Find related pages by entity
        #[arg(long)]
        find: Option<String>,
        /// Show entity relationships
        #[arg(long)]
        relations: bool,
    },
    /// Synthesis page generation
    Synthesize {
        /// Generate synthesis for all topics
        #[arg(long)]
        all: bool,
        /// Generate synthesis for specific topic
        #[arg(long, conflicts_with = "all")]
        topic: Option<String>,
        /// Generate synthesis for top entities
        #[arg(long)]
        top: bool,
        /// Number of top entities
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Lint and clean wiki pages
    Lint {
        /// Lint all wiki pages
        #[arg(long)]
        all: bool,
        /// Lint specific wiki page
        #[arg(long, conflicts_with = "all")]
        page: Option<String>,
        /// Deduplicate wiki pages
        #[arg(long)]
        deduplicate: bool,
        /// Fix formatting issues
        #[arg(long)]
        fix: bool,
    },
    /// Run full wiki generation pipeline
    Pipeline {
        /// Quick pipeline (score + wiki only)
        #[arg(long)]
        quick: bool,
        /// Minimum quality threshold (1-5)
        #[arg(long, default_value = "3")]
        min_quality: u8,
        /// Skip deduplication
        #[arg(long)]
        no_dedup: bool,
        /// Skip synthesis generation
        #[arg(long)]
        no_synthesis: bool,
        /// Synthesis limit
        #[arg(long, default_value = "10")]
        synthesis_limit: usize,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
    /// Merge multiple databases
    Merge {
        /// Source database files to merge
        #[arg(required = true, num_args = 1..)]
        sources: Vec<PathBuf>,
        /// Output database file (default: overwrites current DB)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Show detailed statistics
        #[arg(long)]
        verbose: bool,
    },
}

#[derive(Clone, ValueEnum)]
enum ExportFormat {
    Markdown,
    Json,
    Text,
}

fn format_project_name(raw: &str) -> String {
    db::format_project_name(raw)
}

/// Remove lines that start with `[tool_use: ` from content.
fn strip_tool_lines(content: &str) -> String {
    content
        .lines()
        .filter(|line| !line.starts_with("[tool_use: "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn default_db_path() -> Result<PathBuf> {
    let data_dir = dirs::data_dir().context("Could not determine data directory")?;
    let vault_dir = data_dir.join("claude-vault");
    std::fs::create_dir_all(&vault_dir)?;
    Ok(vault_dir.join("vault.db"))
}

fn default_claude_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".claude"))
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    // Handle completions before opening DB (no DB needed)
    if let Commands::Completions { shell } = &cli.command {
        let mut cmd = Cli::command();
        generate(*shell, &mut cmd, "claude-vault", &mut std::io::stdout());
        return Ok(());
    }

    let db_path = match cli.db {
        Some(p) => p,
        None => default_db_path()?,
    };

    let conn = db::open_db(&db_path)?;

    match cli.command {
        Commands::Import { claude_dir } => {
            let claude_dir = match claude_dir {
                Some(d) => d,
                None => default_claude_dir()?,
            };
            import::import_all(&conn, &claude_dir)?;
        }
        Commands::ImportFile { path, project } => {
            let project = project.unwrap_or_else(|| "unknown".to_string());
            let stats = import::import_jsonl_file(&conn, &path, &project)?;
            println!(
                "Imported {} messages ({} skipped, {} filtered, {} errors)",
                stats.imported, stats.skipped, stats.filtered, stats.errors
            );
        }
        Commands::Search {
            query,
            limit,
            project,
            role,
            since,
            until,
            json,
            include_tools,
        } => {
            let results = db::search(
                &conn,
                &query,
                limit,
                project.as_deref(),
                role.as_deref(),
                since.as_deref(),
                until.as_deref(),
            )?;
            let strip = !include_tools;
            if json {
                let json_results: Vec<serde_json::Value> = results
                    .iter()
                    .filter_map(|r| {
                        let content = if strip {
                            strip_tool_lines(&r.content)
                        } else {
                            r.content.clone()
                        };
                        if strip && content.trim().is_empty() {
                            return None;
                        }
                        Some(serde_json::json!({
                            "session_id": r.session_id,
                            "project": format_project_name(&r.project),
                            "role": r.role,
                            "content": content,
                            "timestamp": r.timestamp,
                        }))
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_results)?);
                return Ok(());
            }
            if results.is_empty() {
                println!("No results found.");
                return Ok(());
            }
            let mut printed = 0;
            for r in results.iter() {
                let content = if strip {
                    strip_tool_lines(&r.content)
                } else {
                    r.content.clone()
                };
                if strip && content.trim().is_empty() {
                    continue;
                }
                if printed > 0 {
                    println!("---");
                }
                let project = format_project_name(&r.project);
                println!(
                    "[{}] {} | {} | {}",
                    r.role,
                    project,
                    r.session_id,
                    r.timestamp.as_deref().unwrap_or("unknown")
                );
                let content: String = if content.chars().count() > 300 {
                    let truncated: String = content.chars().take(300).collect();
                    format!("{truncated}...")
                } else {
                    content
                };
                println!("{}", content);
                printed += 1;
            }
            if printed == 0 {
                println!("No results found.");
            }
        }
        Commands::Export {
            session_id,
            last,
            format,
        } => {
            let resolved_id = match (session_id, last) {
                (_, Some(0)) => bail!("--last must be at least 1"),
                (_, Some(n)) => db::nth_recent_session_id(&conn, n.saturating_sub(1))?,
                (Some(prefix), None) => db::resolve_session_id(&conn, &prefix)?,
                (None, None) => bail!("Specify a session ID or use --last"),
            };
            let messages = db::get_session_messages(&conn, &resolved_id)?;
            if messages.is_empty() {
                bail!("No messages found for session: {resolved_id}");
            }
            let session_id = &resolved_id;
            let project = format_project_name(&messages[0].project);
            match format {
                ExportFormat::Markdown => {
                    println!("# Session: {session_id}");
                    println!("**Project:** {project}  ");
                    if let Some(ts) = &messages[0].timestamp {
                        println!("**Started:** {ts}  ");
                    }
                    println!();
                    for m in &messages {
                        let role_label = match m.role.as_str() {
                            "user" => "User",
                            "assistant" => "Assistant",
                            _ => &m.role,
                        };
                        println!("## {role_label}");
                        if let Some(ts) = &m.timestamp {
                            println!("*{ts}*\n");
                        }
                        println!("{}\n", m.content);
                    }
                }
                ExportFormat::Json => {
                    let json_messages: Vec<serde_json::Value> = messages
                        .iter()
                        .map(|m| {
                            serde_json::json!({
                                "role": m.role,
                                "content": m.content,
                                "timestamp": m.timestamp,
                            })
                        })
                        .collect();
                    let output = serde_json::json!({
                        "session_id": session_id,
                        "project": project,
                        "messages": json_messages,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                ExportFormat::Text => {
                    println!("Session: {session_id}");
                    println!("Project: {project}");
                    println!();
                    for m in &messages {
                        let ts = m.timestamp.as_deref().unwrap_or("");
                        println!("[{}] {} {}", m.role, ts, m.content);
                        println!();
                    }
                }
            }
        }
        Commands::List {
            limit,
            project,
            since,
            until,
            json,
        } => {
            let limit = if limit == 0 { usize::MAX } else { limit };
            let sessions = db::list_sessions(
                &conn,
                limit,
                project.as_deref(),
                since.as_deref(),
                until.as_deref(),
            )?;
            if json {
                let json_sessions: Vec<serde_json::Value> = sessions
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "session_id": s.session_id,
                            "project": format_project_name(&s.project),
                            "started_at": s.started_at,
                            "message_count": s.message_count,
                            "first_user_message": s.first_user_message,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json_sessions)?);
                return Ok(());
            }
            if sessions.is_empty() {
                println!("No sessions found.");
                return Ok(());
            }
            println!(
                "{:<10} {:<22} {:>5}  {:<25} PREVIEW",
                "ID", "DATE", "MSGS", "PROJECT"
            );
            println!("{}", "-".repeat(100));
            for s in &sessions {
                let ts = s
                    .started_at
                    .as_deref()
                    .unwrap_or("unknown")
                    .get(..19)
                    .unwrap_or("unknown");
                let short_id = &s.session_id[..8.min(s.session_id.len())];
                let project = format_project_name(&s.project);
                let project_display: String = if project.chars().count() > 25 {
                    project.chars().take(22).collect::<String>() + "..."
                } else {
                    project
                };
                let preview = s
                    .first_user_message
                    .as_deref()
                    .unwrap_or("")
                    .replace('\n', " ");
                let preview: String = if preview.chars().count() > 50 {
                    preview.chars().take(50).collect::<String>() + "..."
                } else {
                    preview
                };
                println!(
                    "{:<10} {:<22} {:>5}  {:<25} {}",
                    short_id, ts, s.message_count, project_display, preview
                );
            }
            println!("\nExport: claude-vault export <ID>");
        }
        Commands::Delete { session_id, yes } => {
            let resolved_id = db::resolve_session_id(&conn, &session_id)?;
            let messages = db::get_session_messages(&conn, &resolved_id)?;
            let msg_count = messages.len();
            let project = messages
                .first()
                .map(|m| format_project_name(&m.project))
                .unwrap_or_default();

            if !yes {
                eprintln!("Delete session {resolved_id} ({project}, {msg_count} messages)? [y/N] ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let deleted = db::delete_session(&conn, &resolved_id)?;
            println!("Deleted session {resolved_id} ({deleted} messages removed)");
        }
        Commands::Stats => {
            let (sessions, messages) = db::stats(&conn)?;
            println!("Database: {}", db_path.display());
            println!("Sessions: {}", sessions);
            println!("Messages: {}", messages);
        }
        Commands::Verify => {
            db::verify(&conn)?;
        }
        Commands::Categorize { all, session, tree } => {
            if tree {
                show_category_tree(&conn)?;
            } else if all {
                categorize_all(&conn)?;
            } else if let Some(sid) = session {
                categorize_session(&conn, &sid)?;
            } else {
                categorize_all(&conn)?;
            }
        }
        Commands::Score {
            all,
            session,
            top,
            min,
        } => {
            quality::init_quality_tables(&conn)?;
            if top {
                show_top_quality(&conn, min)?;
            } else if all {
                score_all(&conn)?;
            } else if let Some(sid) = session {
                score_session(&conn, &sid)?;
            } else {
                score_all(&conn)?;
            }
        }
        Commands::Wiki {
            all,
            session,
            top,
            export,
            export_all,
            min,
        } => {
            wiki::init_wiki_tables(&conn)?;
            if top {
                show_top_wiki(&conn, min)?;
            } else if let Some(dir) = export_all {
                export_all_wiki(&conn, &dir, min)?;
            } else if all {
                generate_all_wiki(&conn)?;
            } else if let Some(sid) = session {
                generate_wiki_page(&conn, &sid, export)?;
            } else {
                generate_all_wiki(&conn)?;
            }
        }
        Commands::Entities {
            extract,
            wiki,
            find,
            relations,
        } => {
            entities::init_entity_tables(&conn)?;
            if extract {
                extract_all_entities(&conn)?;
            } else if let Some(wiki_id) = wiki {
                show_wiki_entities(&conn, &wiki_id)?;
            } else if let Some(entity_name) = find {
                find_related_pages(&conn, &entity_name)?;
            } else if relations {
                show_entity_relations(&conn)?;
            } else {
                extract_all_entities(&conn)?;
            }
        }
        Commands::Synthesize {
            all,
            topic,
            top,
            limit,
        } => {
            synthesis::init_synthesis_tables(&conn)?;
            if top {
                generate_top_syntheses(&conn, limit)?;
            } else if let Some(t) = topic {
                generate_synthesis_topic(&conn, &t)?;
            } else if all {
                generate_all_syntheses(&conn, limit)?;
            } else {
                generate_top_syntheses(&conn, limit)?;
            }
        }
        Commands::Lint {
            all,
            page,
            deduplicate,
            fix,
        } => {
            lint::init_lint_tables(&conn)?;
            if deduplicate {
                deduplicate_wiki(&conn)?;
            } else if fix {
                fix_formatting(&conn)?;
            } else if let Some(p) = page {
                lint_wiki_page(&conn, &p)?;
            } else if all {
                lint_all_wiki(&conn)?;
            } else {
                lint_all_wiki(&conn)?;
            }
        }
        Commands::Pipeline {
            quick,
            min_quality,
            no_dedup,
            no_synthesis,
            synthesis_limit,
        } => {
            if quick {
                run_quick_pipeline(&conn)?;
            } else {
                let config = pipeline::PipelineConfig {
                    min_quality,
                    deduplicate: !no_dedup,
                    fix_formatting: true,
                    generate_synthesis: !no_synthesis,
                    synthesis_limit,
                };
                run_full_pipeline(&conn, &config)?;
            }
        }
        Commands::Merge {
            sources,
            output,
            verbose,
        } => {
            if let Some(out_path) = output {
                run_merge_to_file(&db_path, &sources, &out_path, verbose)?;
            } else {
                run_merge_into(&conn, &sources, verbose)?;
            }
        }
        Commands::Completions { .. } => unreachable!(),
    }

    Ok(())
}

fn categorize_session(conn: &rusqlite::Connection, session_id: &str) -> anyhow::Result<()> {
    categorize::init_categories(conn)?;
    categorize::init_message_categories(conn)?;

    let categorizer = categorize::Categorizer::new();
    let result = categorizer.categorize_session(conn, session_id)?;

    println!("Title: {}", result.title);
    println!(
        "Category: {} / {}",
        result.primary_category, result.subcategory
    );
    println!("Content Type: {:?}", result.content_type);
    println!("Tags: {}", result.tags.join(", "));
    println!("Confidence: {:.2}", result.confidence);

    categorize::save_category_result(conn, session_id, &result)?;

    Ok(())
}

fn categorize_all(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    categorize::init_categories(conn)?;
    categorize::init_message_categories(conn)?;

    let mut stmt = conn.prepare("SELECT DISTINCT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    println!("Categorizing {} sessions...", session_ids.len());

    let categorizer = categorize::Categorizer::new();
    let mut processed = 0;
    let mut skipped = 0;

    for session_id in session_ids {
        match categorizer.categorize_session(conn, &session_id) {
            Ok(result) => {
                let _ = categorize::save_category_result(conn, &session_id, &result);
                processed += 1;
            }
            Err(e) => {
                eprintln!("Skipping {}: {}", session_id, e);
                skipped += 1;
            }
        }
    }

    println!("Categorized {} sessions ({} skipped)", processed, skipped);

    Ok(())
}

fn show_category_tree(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    categorize::init_categories(conn)?;

    let categories = categorize::get_category_tree(conn)?;

    println!("Category Tree:");
    println!();

    for cat in &categories {
        let indent = if cat.parent_id.is_some() { "  " } else { "" };
        println!("{}{} ({} docs)", indent, cat.name, cat.document_count);
    }

    Ok(())
}

fn score_session(conn: &rusqlite::Connection, session_id: &str) -> anyhow::Result<()> {
    quality::init_quality_tables(conn)?;

    let scorer = quality::QualityScorer::new();
    let result = scorer.score_session(conn, session_id)?;

    println!("Quality Score: {}/5", result.score.value());
    println!("Reasoning: {}", result.reasoning);
    println!("Metrics:");
    println!("  Code blocks: {}", result.metrics.code_blocks);
    println!("  Resolutions: {}", result.metrics.resolution_count);
    println!("  Technical depth: {}/5", result.metrics.technical_depth);
    println!("  Actionability: {}/5", result.metrics.actionability);
    println!("  Explanation: {}/5", result.metrics.explanation_quality);

    Ok(())
}

fn score_all(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    quality::init_quality_tables(conn)?;

    let mut stmt = conn.prepare("SELECT DISTINCT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    println!("Scoring {} sessions...", session_ids.len());

    let scorer = quality::QualityScorer::new();
    let mut processed = 0;
    let mut skipped = 0;

    for session_id in session_ids {
        match scorer.score_session(conn, &session_id) {
            Ok(_result) => {
                // Update session average quality
                let _ = quality::update_session_quality(conn, &session_id);
                processed += 1;
            }
            Err(e) => {
                eprintln!("Skipping {}: {}", session_id, e);
                skipped += 1;
            }
        }
    }

    println!("Scored {} sessions ({} skipped)", processed, skipped);

    Ok(())
}

fn show_top_quality(conn: &rusqlite::Connection, min: u8) -> anyhow::Result<()> {
    quality::init_quality_tables(conn)?;

    let sessions = quality::get_high_quality_sessions(conn, min, 20)?;

    println!("Top Sessions (quality >= {}):", min);
    println!();

    for (session_id, avg_quality) in sessions {
        println!(
            "  {} - {:.1}/5",
            &session_id[..8.min(session_id.len())],
            avg_quality
        );
    }

    Ok(())
}

fn generate_wiki_page(
    conn: &rusqlite::Connection,
    session_id: &str,
    export: Option<PathBuf>,
) -> anyhow::Result<()> {
    wiki::init_wiki_tables(conn)?;

    let generator = wiki::WikiGenerator::new();

    // Get quality result
    let scorer = quality::QualityScorer::new();
    let quality_result = scorer.score_session(conn, session_id)?;

    // Get categories
    let categorizer = categorize::Categorizer::new();
    let cat_result = categorizer.categorize_session(conn, session_id)?;

    // Generate wiki page
    let page = generator.generate_page(
        conn,
        session_id,
        &quality_result,
        &[cat_result.primary_category, cat_result.subcategory],
        &cat_result.tags,
    )?;

    // Save to database
    wiki::save_wiki_page(conn, &page)?;

    println!("Wiki page generated: {}", page.title);
    println!("Quality: {}/5", page.frontmatter.quality_score);
    println!("Categories: {}", page.frontmatter.categories.join(", "));

    // Export to file if requested
    if let Some(path) = export {
        let markdown = generator.render_markdown(&page);
        std::fs::write(&path, markdown)?;
        println!("Exported to: {}", path.display());
    }

    Ok(())
}

fn generate_all_wiki(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    wiki::init_wiki_tables(conn)?;
    categorize::init_categories(conn)?;
    quality::init_quality_tables(conn)?;

    let mut stmt = conn.prepare("SELECT DISTINCT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    println!("Generating {} wiki pages...", session_ids.len());

    let generator = wiki::WikiGenerator::new();
    let scorer = quality::QualityScorer::new();
    let _categorizer = categorize::Categorizer::new();
    let mut processed = 0;
    let mut skipped = 0;

    for session_id in session_ids {
        match generator.generate_page(
            conn,
            &session_id,
            &scorer.score_session(conn, &session_id)?,
            &["Technology".to_string(), "General".to_string()],
            &["wiki".to_string()],
        ) {
            Ok(page) => {
                let _ = wiki::save_wiki_page(conn, &page);
                processed += 1;
            }
            Err(e) => {
                eprintln!("Skipping {}: {}", session_id, e);
                skipped += 1;
            }
        }
    }

    println!("Generated {} wiki pages ({} skipped)", processed, skipped);

    Ok(())
}

fn show_top_wiki(conn: &rusqlite::Connection, min: u8) -> anyhow::Result<()> {
    wiki::init_wiki_tables(conn)?;

    let pages = wiki::get_high_quality_pages(conn, min, 20)?;

    println!("Top Wiki Pages (quality >= {}):", min);
    println!();

    for page in pages {
        println!("  {} - {}/5", page.title, page.frontmatter.quality_score);
    }

    Ok(())
}

fn export_all_wiki(
    conn: &rusqlite::Connection,
    target_dir: &PathBuf,
    min_quality: u8,
) -> anyhow::Result<()> {
    use std::fs;

    // Create target directory
    fs::create_dir_all(target_dir)?;

    // Get all wiki pages
    let all_pages = wiki::get_all_wiki_pages(conn)?;
    let pages: Vec<_> = all_pages
        .into_iter()
        .filter(|p| p.frontmatter.quality_score >= min_quality)
        .collect();

    println!(
        "Exporting {} wiki pages to {}...",
        pages.len(),
        target_dir.display()
    );

    let generator = wiki::WikiGenerator::new();
    let mut exported = 0;
    let mut skipped = 0;

    for page in pages {
        // Create safe filename from title
        let safe_title = page
            .title
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect::<String>()
            .to_lowercase();

        // Determine category directory
        let category = page
            .frontmatter
            .categories
            .first()
            .map(|s| s.as_str())
            .unwrap_or("general");

        // Sanitize category for filesystem
        let safe_category = category
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect::<String>()
            .to_lowercase();

        // Create category subdirectory
        let category_dir = target_dir.join(&safe_category);
        fs::create_dir_all(&category_dir)?;

        // Write markdown file
        let file_path = category_dir.join(format!("{}.md", safe_title));
        let markdown = generator.render_markdown(&page);

        match fs::write(&file_path, markdown) {
            Ok(_) => {
                println!("  Exported: {}", file_path.display());
                exported += 1;
            }
            Err(e) => {
                eprintln!("  Failed to write {}: {}", file_path.display(), e);
                skipped += 1;
            }
        }
    }

    println!("\nExported {} pages ({} skipped)", exported, skipped);

    Ok(())
}

fn extract_all_entities(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    entities::init_entity_tables(conn)?;
    wiki::init_wiki_tables(conn)?;

    let mut stmt = conn.prepare("SELECT id, content FROM wiki_pages")?;
    let wiki_pages: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    println!(
        "Extracting entities from {} wiki pages...",
        wiki_pages.len()
    );

    let extractor = entities::EntityExtractor::new();
    let mut total_entities = 0;

    for (wiki_id, content) in wiki_pages {
        let extracted = extractor.extract(&content);
        for entity in &extracted {
            let entity_id = entities::save_entity(conn, entity)?;
            entities::link_entity_to_wiki(conn, &wiki_id, entity_id, entity.count)?;
            total_entities += 1;
        }
    }

    println!("Extracted {} total entities", total_entities);

    Ok(())
}

fn show_wiki_entities(conn: &rusqlite::Connection, wiki_id: &str) -> anyhow::Result<()> {
    entities::init_entity_tables(conn)?;

    let entities_list = entities::get_wiki_entities(conn, wiki_id)?;

    println!("Entities for wiki page {}:", wiki_id);
    println!();

    for entity in entities_list {
        println!(
            "  {} ({:?}) - {} occurrences",
            entity.name, entity.entity_type, entity.count
        );
    }

    Ok(())
}

fn find_related_pages(conn: &rusqlite::Connection, entity_name: &str) -> anyhow::Result<()> {
    entities::init_entity_tables(conn)?;

    let wiki_ids = entities::get_related_by_entity(conn, entity_name, 20)?;

    println!("Wiki pages mentioning '{}':", entity_name);
    println!();

    for wiki_id in wiki_ids {
        println!("  {}", &wiki_id[..8.min(wiki_id.len())]);
    }

    Ok(())
}

fn show_entity_relations(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    entities::init_entity_tables(conn)?;

    let mut stmt = conn.prepare(
        "SELECT e1.name, e2.name, er.relation_type, er.confidence
         FROM entity_relations er
         JOIN entities e1 ON er.from_entity = e1.id
         JOIN entities e2 ON er.to_entity = e2.id
         LIMIT 50",
    )?;

    let relations = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f32>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    println!("Entity Relationships:");
    println!();

    for (from, to, rel_type, conf) in relations {
        println!("  {} --[{}]--> {} ({:.1})", from, rel_type, to, conf);
    }

    Ok(())
}

fn generate_synthesis_topic(conn: &rusqlite::Connection, topic: &str) -> anyhow::Result<()> {
    synthesis::init_synthesis_tables(&conn)?;
    entities::init_entity_tables(&conn)?;

    let generator = synthesis::SynthesisGenerator::new();
    let page = generator.generate_synthesis(conn, topic)?;

    synthesis::save_synthesis_page(conn, &page)?;

    println!("Synthesis page: {}", page.title);
    println!("Sources: {}", page.sources.len());
    println!("Key points: {}", page.key_points.len());

    Ok(())
}

fn generate_top_syntheses(conn: &rusqlite::Connection, limit: usize) -> anyhow::Result<()> {
    synthesis::init_synthesis_tables(&conn)?;
    entities::init_entity_tables(&conn)?;

    let syntheses = synthesis::SynthesisGenerator::generate_top_syntheses(conn, limit)?;

    println!("Generating {} synthesis pages...", syntheses.len());

    for synth in &syntheses {
        synthesis::save_synthesis_page(conn, synth)?;
        println!("  {} (sources: {})", synth.topic, synth.sources.len());
    }

    Ok(())
}

fn generate_all_syntheses(conn: &rusqlite::Connection, _limit: usize) -> anyhow::Result<()> {
    synthesis::init_synthesis_tables(&conn)?;
    entities::init_entity_tables(&conn)?;

    let mut stmt = conn.prepare("SELECT name FROM entities")?;
    let entities: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    println!("Generating synthesis for {} entities...", entities.len());

    let generator = synthesis::SynthesisGenerator::new();
    let mut processed = 0;
    let mut skipped = 0;

    for entity in entities {
        match generator.generate_synthesis(conn, &entity) {
            Ok(page) => {
                let _ = synthesis::save_synthesis_page(conn, &page);
                processed += 1;
            }
            Err(_) => skipped += 1,
        }
    }

    println!(
        "Generated {} synthesis pages ({} skipped)",
        processed, skipped
    );

    Ok(())
}

fn lint_wiki_page(conn: &rusqlite::Connection, wiki_id: &str) -> anyhow::Result<()> {
    lint::init_lint_tables(&conn)?;

    let linter = lint::WikiLinter::new();
    let result = linter.lint_page(conn, wiki_id)?;

    println!("Lint results for {}:", wiki_id);
    println!("Issues found: {}", result.issues.len());

    for issue in &result.issues {
        match issue {
            lint::LintIssue::DuplicateContent { similar_id, .. } => {
                println!("  - Duplicate content: similar to {}", similar_id);
            }
            lint::LintIssue::ShortContent { length } => {
                println!("  - Short content: {} characters", length);
            }
            lint::LintIssue::BrokenLink { link } => {
                println!("  - Broken link: {}", link);
            }
            lint::LintIssue::EmptySection { section_name } => {
                println!("  - Empty section: {}", section_name);
            }
            lint::LintIssue::FormattingIssue { description } => {
                println!("  - Formatting issue: {}", description);
            }
        }
    }

    lint::save_lint_result(conn, &result)?;

    Ok(())
}

fn lint_all_wiki(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    lint::init_lint_tables(&conn)?;

    let linter = lint::WikiLinter::new();
    let results = linter.lint_all(conn)?;

    println!("Linted {} wiki pages with issues", results.len());

    for result in results {
        println!("  {} - {} issues", result.wiki_id, result.issues.len());
        lint::save_lint_result(conn, &result)?;
    }

    Ok(())
}

fn deduplicate_wiki(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    let linter = lint::WikiLinter::new();
    let to_remove = linter.deduplicate(conn)?;

    if to_remove.is_empty() {
        println!("No duplicates found.");
        return Ok(());
    }

    println!("Found {} duplicate pages:", to_remove.len());

    for id in &to_remove {
        println!("  {}", id);
    }

    // Remove duplicates
    for id in &to_remove {
        conn.execute("DELETE FROM wiki_pages WHERE id = ?1", params![id])?;
    }

    println!("Removed {} duplicate pages", to_remove.len());

    Ok(())
}

fn fix_formatting(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    let linter = lint::WikiLinter::new();
    let fixed = linter.fix_formatting(conn)?;

    println!("Fixed formatting in {} pages", fixed);

    Ok(())
}

fn run_full_pipeline(
    conn: &rusqlite::Connection,
    config: &pipeline::PipelineConfig,
) -> anyhow::Result<()> {
    let pipeline = pipeline::WikiPipeline::new();
    let stats = pipeline.run(conn, config)?;
    pipeline.print_stats(&stats);
    Ok(())
}

fn run_quick_pipeline(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    let stats = pipeline::run_quick_pipeline(conn)?;
    println!("\n=== Quick Pipeline Results ===");
    println!("Sessions processed: {}", stats.sessions_processed);
    println!("Wiki pages generated: {}", stats.wiki_pages_generated);
    Ok(())
}

fn run_merge_into(
    target: &rusqlite::Connection,
    sources: &[PathBuf],
    verbose: bool,
) -> anyhow::Result<()> {
    println!("Merging {} databases into current DB...", sources.len());

    let source_paths: Vec<&std::path::Path> = sources.iter().map(|p| p.as_path()).collect();
    let stats = merge::merge_databases(target, &source_paths)?;

    if verbose {
        merge::print_stats(&stats);
    } else {
        println!("Merge complete:");
        println!("  Sessions: {} added", stats.sessions_added);
        println!("  Messages: {} added", stats.messages_added);
    }

    Ok(())
}

fn run_merge_to_file(
    current_db_path: &PathBuf,
    sources: &[PathBuf],
    output: &PathBuf,
    verbose: bool,
) -> anyhow::Result<()> {
    println!("Creating new merged database at {}...", output.display());

    // Create new database (open_db initializes schema)
    let target = db::open_db(output)?;

    // Merge current DB first
    let stats1 = merge::merge_single(&target, current_db_path)?;
    if verbose {
        println!("From current DB:");
        println!("  Sessions: {} added", stats1.sessions_added);
        println!("  Messages: {} added", stats1.messages_added);
    }

    // Then merge all additional sources
    let source_paths: Vec<&std::path::Path> = sources.iter().map(|p| p.as_path()).collect();
    let stats2 = merge::merge_databases(&target, &source_paths)?;

    if verbose {
        println!("From source files:");
        merge::print_stats(&stats2);
    } else {
        println!("Merge complete:");
        println!(
            "  Sessions: {} added",
            stats1.sessions_added + stats2.sessions_added
        );
        println!(
            "  Messages: {} added",
            stats1.messages_added + stats2.messages_added
        );
    }

    println!("Merged database written to: {}", output.display());

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

    #[test]
    fn test_strip_tool_lines_removes_tool_use() {
        let input = "Here is my plan\n[tool_use: Edit] {\"file\":\"foo.rs\"}\nDone editing.";
        assert_eq!(strip_tool_lines(input), "Here is my plan\nDone editing.");
    }

    #[test]
    fn test_strip_tool_lines_preserves_plain_text() {
        let input = "Just a normal message\nwith multiple lines";
        assert_eq!(strip_tool_lines(input), input);
    }

    #[test]
    fn test_strip_tool_lines_all_tools_becomes_empty() {
        let input = "[tool_use: Bash] {\"command\":\"ls\"}\n[tool_use: Edit] {\"file\":\"x\"}";
        assert_eq!(strip_tool_lines(input), "");
    }

    #[test]
    fn test_strip_tool_lines_preserves_brackets_in_text() {
        let input = "Use [this] syntax for arrays\n[tool_use: Write] {\"file\":\"x\"}\nEnd";
        assert_eq!(strip_tool_lines(input), "Use [this] syntax for arrays\nEnd");
    }

    #[test]
    fn test_strip_tool_lines_empty_input() {
        assert_eq!(strip_tool_lines(""), "");
    }

    #[test]
    fn test_strip_tool_lines_single_tool_line() {
        let input = "[tool_use: Bash] {\"command\":\"cargo test\"}";
        assert_eq!(strip_tool_lines(input), "");
    }
}
