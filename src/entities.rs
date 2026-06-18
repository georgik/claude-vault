//! Entity extraction and cross-references
//!
//! Extracts entities (technologies, concepts, people) from wiki content.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Entity type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Technology,
    Concept,
    Person,
    Library,
    Framework,
    Language,
    Tool,
    Algorithm,
    Pattern,
    Other(String),
}

/// Extracted entity with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub entity_type: EntityType,
    pub count: usize,
    pub aliases: Vec<String>,
    pub description: Option<String>,
}

/// Entity extractor analyzes content for entities
pub struct EntityExtractor;

impl EntityExtractor {
    pub fn new() -> Self {
        Self
    }

    /// Extract entities from content
    pub fn extract(&self, content: &str) -> Vec<Entity> {
        let mut entities = HashMap::new();
        let content_lower = content.to_lowercase();

        // Technology entities
        for tech in self.get_technologies() {
            if content_lower.contains(&tech.to_lowercase()) {
                let count = content_lower.matches(&tech.to_lowercase()).count();
                entities.insert(
                    tech.clone(),
                    Entity {
                        name: tech,
                        entity_type: EntityType::Technology,
                        count,
                        aliases: vec![],
                        description: None,
                    },
                );
            }
        }

        // Language entities
        for lang in self.get_languages() {
            if content_lower.contains(&lang.to_lowercase()) {
                let count = content_lower.matches(&lang.to_lowercase()).count();
                entities.insert(
                    lang.clone(),
                    Entity {
                        name: lang,
                        entity_type: EntityType::Language,
                        count,
                        aliases: vec![],
                        description: None,
                    },
                );
            }
        }

        // Concept entities
        for concept in self.get_concepts() {
            if content_lower.contains(&concept.to_lowercase()) {
                let count = content_lower.matches(&concept.to_lowercase()).count();
                entities.insert(
                    concept.clone(),
                    Entity {
                        name: concept,
                        entity_type: EntityType::Concept,
                        count,
                        aliases: vec![],
                        description: None,
                    },
                );
            }
        }

        // Pattern entities
        for pattern in self.get_patterns() {
            if content_lower.contains(&pattern.to_lowercase()) {
                let count = content_lower.matches(&pattern.to_lowercase()).count();
                entities.insert(
                    pattern.clone(),
                    Entity {
                        name: pattern,
                        entity_type: EntityType::Pattern,
                        count,
                        aliases: vec![],
                        description: None,
                    },
                );
            }
        }

        entities.into_values().collect()
    }

    fn get_technologies(&self) -> Vec<String> {
        vec![
            // Databases
            "postgresql".to_string(),
            "mysql".to_string(),
            "sqlite".to_string(),
            "redis".to_string(),
            "mongodb".to_string(),
            "elasticsearch".to_string(),
            // Cloud
            "aws".to_string(),
            "gcp".to_string(),
            "azure".to_string(),
            // Containers
            "docker".to_string(),
            "kubernetes".to_string(),
            "podman".to_string(),
            // Web
            "nginx".to_string(),
            "apache".to_string(),
            "nodejs".to_string(),
            "deno".to_string(),
            "bun".to_string(),
            // Tools
            "git".to_string(),
            "github".to_string(),
            "gitlab".to_string(),
            "terraform".to_string(),
            "ansible".to_string(),
        ]
    }

    fn get_languages(&self) -> Vec<String> {
        vec![
            "rust".to_string(),
            "python".to_string(),
            "javascript".to_string(),
            "typescript".to_string(),
            "go".to_string(),
            "java".to_string(),
            "c++".to_string(),
            "csharp".to_string(),
            "ruby".to_string(),
            "php".to_string(),
            "swift".to_string(),
            "kotlin".to_string(),
            "scala".to_string(),
            "elixir".to_string(),
            "haskell".to_string(),
            "zig".to_string(),
            "nim".to_string(),
            "julia".to_string(),
        ]
    }

    fn get_concepts(&self) -> Vec<String> {
        vec![
            "async".to_string(),
            "concurrency".to_string(),
            "parallelism".to_string(),
            "mutex".to_string(),
            "semaphore".to_string(),
            "channel".to_string(),
            "memory management".to_string(),
            "garbage collection".to_string(),
            "reference counting".to_string(),
            "error handling".to_string(),
            "exception".to_string(),
            "panic".to_string(),
            "type system".to_string(),
            "static typing".to_string(),
            "dynamic typing".to_string(),
            "generics".to_string(),
            "polymorphism".to_string(),
            "inheritance".to_string(),
            "encapsulation".to_string(),
            "abstraction".to_string(),
            "api".to_string(),
            "rest".to_string(),
            "graphql".to_string(),
            "grpc".to_string(),
            "authentication".to_string(),
            "authorization".to_string(),
            "encryption".to_string(),
            "hashing".to_string(),
            "caching".to_string(),
            "load balancing".to_string(),
            "rate limiting".to_string(),
        ]
    }

    fn get_patterns(&self) -> Vec<String> {
        vec![
            "singleton".to_string(),
            "factory".to_string(),
            "observer".to_string(),
            "strategy".to_string(),
            "adapter".to_string(),
            "decorator".to_string(),
            "builder".to_string(),
            "repository".to_string(),
            "mvc".to_string(),
            "mvvm".to_string(),
            "flux".to_string(),
            "redux".to_string(),
            "reactor".to_string(),
            "actor model".to_string(),
            "csp".to_string(),
            "map-reduce".to_string(),
        ]
    }
}

/// Initialize entity tables
pub fn init_entity_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS entities (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            entity_type TEXT NOT NULL,
            description TEXT,
            aliases TEXT
        );

        CREATE TABLE IF NOT EXISTS wiki_entities (
            wiki_id TEXT NOT NULL,
            entity_id INTEGER NOT NULL,
            count INTEGER DEFAULT 1,
            PRIMARY KEY (wiki_id, entity_id),
            FOREIGN KEY (wiki_id) REFERENCES wiki_pages(id),
            FOREIGN KEY (entity_id) REFERENCES entities(id)
        );

        CREATE TABLE IF NOT EXISTS entity_relations (
            from_entity INTEGER NOT NULL,
            to_entity INTEGER NOT NULL,
            relation_type TEXT NOT NULL,
            confidence REAL DEFAULT 0.5,
            FOREIGN KEY (from_entity) REFERENCES entities(id),
            FOREIGN KEY (to_entity) REFERENCES entities(id)
        );

        CREATE INDEX IF NOT EXISTS idx_entity_type ON entities(entity_type);
        CREATE INDEX IF NOT EXISTS idx_wiki_entity ON wiki_entities(entity_id);
        ",
    )?;
    Ok(())
}

/// Save entity to database
pub fn save_entity(conn: &Connection, entity: &Entity) -> Result<i64> {
    let aliases_json = serde_json::to_string(&entity.aliases)?;

    conn.execute(
        "INSERT OR IGNORE INTO entities (name, entity_type, description, aliases)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            entity.name,
            format!("{:?}", entity.entity_type),
            entity.description,
            aliases_json,
        ],
    )?;

    let id: i64 = conn
        .query_row(
            "SELECT id FROM entities WHERE name = ?1",
            params![entity.name],
            |row| row.get(0),
        )
        .unwrap();

    Ok(id)
}

/// Link entity to wiki page
pub fn link_entity_to_wiki(
    conn: &Connection,
    wiki_id: &str,
    entity_id: i64,
    count: usize,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO wiki_entities (wiki_id, entity_id, count)
         VALUES (?1, ?2, ?3)",
        params![wiki_id, entity_id, count as i64],
    )?;
    Ok(())
}

/// Get all entities for a wiki page
pub fn get_wiki_entities(conn: &Connection, wiki_id: &str) -> Result<Vec<Entity>> {
    let mut stmt = conn.prepare(
        "SELECT e.name, e.entity_type, we.count, e.aliases
         FROM entities e
         JOIN wiki_entities we ON e.id = we.entity_id
         WHERE we.wiki_id = ?1
         ORDER BY we.count DESC",
    )?;

    let entities = stmt
        .query_map(params![wiki_id], |row| {
            let aliases_json: String = row.get(3)?;
            let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_default();

            Ok(Entity {
                name: row.get(0)?,
                entity_type: parse_entity_type(&row.get::<_, String>(1)?),
                count: row.get(2)?,
                aliases,
                description: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entities)
}

/// Get related wiki pages by entity
pub fn get_related_by_entity(
    conn: &Connection,
    entity_name: &str,
    limit: usize,
) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT wiki_id
         FROM wiki_entities we
         JOIN entities e ON we.entity_id = e.id
         WHERE e.name = ?1
         ORDER BY we.count DESC
         LIMIT ?2",
    )?;

    let wiki_ids = stmt
        .query_map(params![entity_name, limit as i64], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(wiki_ids)
}

fn parse_entity_type(s: &str) -> EntityType {
    match s {
        "Technology" => EntityType::Technology,
        "Concept" => EntityType::Concept,
        "Person" => EntityType::Person,
        "Library" => EntityType::Library,
        "Framework" => EntityType::Framework,
        "Language" => EntityType::Language,
        "Tool" => EntityType::Tool,
        "Algorithm" => EntityType::Algorithm,
        "Pattern" => EntityType::Pattern,
        _ => EntityType::Other(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_technologies() {
        let extractor = EntityExtractor::new();
        let content = "Using Docker and Kubernetes for deployment.";

        let entities = extractor.extract(content);
        assert!(!entities.is_empty());
    }

    #[test]
    fn test_extract_languages() {
        let extractor = EntityExtractor::new();
        let content = "Writing Rust code with async/await.";

        let entities = extractor.extract(content);
        assert!(entities.iter().any(|e| e.name == "rust"));
    }

    #[test]
    fn test_extract_concepts() {
        let extractor = EntityExtractor::new();
        let content = "Implementing mutex for concurrency control.";

        let entities = extractor.extract(content);
        assert!(entities.iter().any(|e| e.name == "mutex"));
    }

    #[test]
    fn test_entity_type_parse() {
        assert!(matches!(
            parse_entity_type("Technology"),
            EntityType::Technology
        ));
        assert!(matches!(parse_entity_type("Pattern"), EntityType::Pattern));
    }
}
