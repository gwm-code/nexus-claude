//! Graph Memory - Layer 2: Relationships and dependencies
//!
//! Stores entities and their relationships for structured reasoning

use crate::error::{NexusError, Result};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;

/// Graph-based memory for relationships
pub struct GraphMemory {
    storage_path: PathBuf,
    entities: HashMap<String, Entity>,
    relations: Vec<Relation>,
}

/// An entity in the graph (file, function, project, user, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub entity_type: String, // "project", "file", "function", "user", "dependency"
    pub properties: HashMap<String, String>,
}

/// A relationship between entities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub from: String,
    pub to: String,
    pub relation_type: String, // "depends_on", "imports", "calls", "contains"
    pub properties: HashMap<String, String>,
}

impl GraphMemory {
    pub fn new(storage_path: PathBuf) -> Result<Self> {
        fs::create_dir_all(&storage_path)?;
        
        // Try to load existing graph
        let entities = Self::load_entities(&storage_path)?;
        let relations = Self::load_relations(&storage_path)?;
        
        Ok(Self {
            storage_path,
            entities,
            relations,
        })
    }

    /// Add an entity to the graph
    pub async fn add_entity(&mut self, entity: Entity) -> Result<()> {
        self.entities.insert(entity.id.clone(), entity);
        self.save_entities().await?;
        Ok(())
    }

    /// Add a relationship
    pub async fn add_relation(&mut self, relation: Relation) -> Result<()> {
        self.relations.push(relation);
        self.save_relations().await?;
        Ok(())
    }

    /// Update entity property
    pub async fn update_entity_property(
        &mut self,
        entity_id: &str,
        key: &str,
        value: &str,
    ) -> Result<()> {
        if let Some(entity) = self.entities.get_mut(entity_id) {
            entity.properties.insert(key.to_string(), value.to_string());
            self.save_entities().await?;
        }
        Ok(())
    }

    /// Query entities by type or properties
    pub async fn query_entities(&self, query: &str) -> Result<Vec<Entity>> {
        // Simple text search for now
        // In production, use proper graph query language
        let results: Vec<Entity> = self.entities.values()
            .filter(|e| {
                e.id.contains(query) ||
                e.entity_type.contains(query) ||
                e.properties.values().any(|v| v.contains(query))
            })
            .cloned()
            .collect();
        
        Ok(results)
    }

    /// Get all relationships for an entity
    pub async fn get_relations(&self, entity_id: &str) -> Result<Vec<Relation>> {
        let related: Vec<Relation> = self.relations.iter()
            .filter(|r| r.from == entity_id || r.to == entity_id)
            .cloned()
            .collect();
        
        Ok(related)
    }

    /// Get project facts
    pub async fn get_project_facts(&self) -> Result<HashMap<String, String>> {
        // Find project entity
        let project = self.entities.values()
            .find(|e| e.entity_type == "project");
        
        if let Some(project) = project {
            Ok(project.properties.clone())
        } else {
            Ok(HashMap::new())
        }
    }

    /// Count entities
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    // Private helpers
    fn load_entities(path: &PathBuf) -> Result<HashMap<String, Entity>> {
        let file = path.join("entities.json");
        if file.exists() {
            let content = fs::read_to_string(&file)
                .map_err(|e| NexusError::Io(e))?;
            let entities: Vec<Entity> = serde_json::from_str(&content)
                .map_err(|e| NexusError::Json(e))?;
            
            let map: HashMap<String, Entity> = entities.into_iter()
                .map(|e| (e.id.clone(), e))
                .collect();
            Ok(map)
        } else {
            Ok(HashMap::new())
        }
    }

    fn load_relations(path: &PathBuf) -> Result<Vec<Relation>> {
        let file = path.join("relations.json");
        if file.exists() {
            let content = fs::read_to_string(&file)
                .map_err(|e| NexusError::Io(e))?;
            let relations: Vec<Relation> = serde_json::from_str(&content)
                .map_err(|e| NexusError::Json(e))?;
            Ok(relations)
        } else {
            Ok(vec![])
        }
    }

    async fn save_entities(&self) -> Result<()> {
        let file = self.storage_path.join("entities.json");
        let entities: Vec<&Entity> = self.entities.values().collect();
        let json = serde_json::to_string_pretty(&entities)
            .map_err(|e| NexusError::Json(e))?;
        fs::write(&file, json)
            .map_err(|e| NexusError::Io(e))?;
        Ok(())
    }

    async fn save_relations(&self) -> Result<()> {
        let file = self.storage_path.join("relations.json");
        let json = serde_json::to_string_pretty(&self.relations)
            .map_err(|e| NexusError::Json(e))?;
        fs::write(&file, json)
            .map_err(|e| NexusError::Io(e))?;
        Ok(())
    }
}
