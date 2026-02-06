//! Memory System - Production-grade 3-layer architecture
//! 
//! Layer 1: Event Store - Immutable audit trail
//! Layer 2: Graph Database - Relationships and dependencies  
//! Layer 3: Vector Store - Semantic search and embeddings

pub mod event_store;
pub mod graph;
pub mod semantic;
pub mod types;
pub mod consolidation;

pub use event_store::MemoryEvent;

use crate::error::Result;
use std::path::PathBuf;
use std::collections::HashMap;
use std::time::{SystemTime, Duration};

/// The unified memory system that combines all three layers
pub struct MemorySystem {
    /// Layer 1: Immutable event log
    event_store: event_store::EventStore,
    /// Layer 2: Graph relationships
    graph: std::sync::Mutex<graph::GraphMemory>,
    /// Layer 3: Vector embeddings
    vector: semantic::VectorMemory,
    /// Storage directory
    storage_path: PathBuf,
    /// Session ID for tracking
    session_id: String,
}

impl MemorySystem {
    pub fn new(storage_path: PathBuf) -> Result<Self> {
        let session_id = format!("session_{}", uuid::Uuid::new_v4());
        
        Ok(Self {
            event_store: event_store::EventStore::new(storage_path.join("events"))?,
            graph: std::sync::Mutex::new(graph::GraphMemory::new(storage_path.join("graph"))?),
            vector: semantic::VectorMemory::new(storage_path.join("vector"))?,
            storage_path,
            session_id,
        })
    }

    /// Initialize the memory system for a project
    pub async fn init_project(&self, project_path: &PathBuf, project_name: &str) -> Result<()> {
        // Log project initialization
        self.event_store.log_event(MemoryEvent::ProjectInit {
            project_name: project_name.to_string(),
            project_path: project_path.clone(),
            timestamp: SystemTime::now(),
        }).await?;

        // Add project entity to graph
        self.graph.lock().unwrap().add_entity(graph::Entity {
            id: project_name.to_string(),
            entity_type: "project".to_string(),
            properties: HashMap::from([
                ("path".to_string(), project_path.to_string_lossy().to_string()),
                ("name".to_string(), project_name.to_string()),
            ]),
        }).await?;

        println!("✓ Memory system initialized for project: {}", project_name);
        Ok(())
    }

    /// Record a user interaction (episodic memory)
    pub async fn record_interaction(
        &mut self, 
        query: &str, 
        response: &str,
        tools_used: Vec<String>,
    ) -> Result<()> {
        // Store in event log
        self.event_store.log_event(MemoryEvent::Interaction {
            session_id: self.session_id.clone(),
            query: query.to_string(),
            response: response.to_string(),
            tools_used,
            timestamp: SystemTime::now(),
        }).await?;

        // Index in vector store for semantic search
        self.vector.index_document(
            &format!("interaction_{}", uuid::Uuid::new_v4()),
            &format!("Query: {} Response: {}", query, response),
            HashMap::from([
                ("type".to_string(), "interaction".to_string()),
                ("session".to_string(), self.session_id.clone()),
            ]),
        ).await?;

        Ok(())
    }

    /// Store a fact about the user or project (semantic memory)
    pub async fn remember_fact(
        &mut self,
        entity: &str,
        fact_type: &str,
        value: &str,
    ) -> Result<()> {
        // Store in graph as entity property
        self.graph.lock().unwrap().update_entity_property(
            entity,
            fact_type,
            value,
        ).await?;

        // Also store in semantic memory for search
        let fact_text = format!("{} {}: {}", entity, fact_type, value);
        self.vector.index_document(
            &format!("fact_{}", uuid::Uuid::new_v4()),
            &fact_text,
            HashMap::from([
                ("type".to_string(), "fact".to_string()),
                ("entity".to_string(), entity.to_string()),
                ("fact_type".to_string(), fact_type.to_string()),
            ]),
        ).await?;

        // Log the fact
        self.event_store.log_event(MemoryEvent::FactStored {
            entity: entity.to_string(),
            fact_type: fact_type.to_string(),
            value: value.to_string(),
            timestamp: SystemTime::now(),
        }).await?;

        Ok(())
    }

    /// Store a learned procedure (procedural memory)
    pub async fn remember_procedure(
        &mut self,
        name: &str,
        steps: Vec<String>,
        context: &str,
    ) -> Result<()> {
        let procedure = types::Procedure {
            name: name.to_string(),
            steps,
            context: context.to_string(),
            created_at: SystemTime::now(),
            success_count: 0,
        };

        // Store in event log
        self.event_store.log_event(MemoryEvent::ProcedureLearned {
            procedure_name: name.to_string(),
            context: context.to_string(),
            timestamp: SystemTime::now(),
        }).await?;

        // Index for retrieval
        let proc_text = format!("Procedure {} for {}: {}", 
            name, context, procedure.steps.join(", "));
        self.vector.index_document(
            &format!("proc_{}", name),
            &proc_text,
            HashMap::from([
                ("type".to_string(), "procedure".to_string()),
                ("name".to_string(), name.to_string()),
            ]),
        ).await?;

        Ok(())
    }

    /// Search for relevant memories
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<types::MemoryResult>> {
        let mut results = Vec::new();

        // Search vector store (semantic similarity)
        let semantic_results = self.vector.search(query, limit).await?;
        for result in semantic_results {
            results.push(types::MemoryResult::Semantic {
                content: result.text,
                score: result.score,
                metadata: result.metadata,
            });
        }

        // Search graph (relationships)
        let graph_results = self.graph.lock().unwrap().query_entities(query).await?;
        for entity in graph_results {
            results.push(types::MemoryResult::Graph {
                entity: entity.id,
                entity_type: entity.entity_type,
                properties: entity.properties,
            });
        }

        // Get recent events (episodic)
        let events = self.event_store.get_recent_events(limit).await?;
        for event in events {
            results.push(types::MemoryResult::Episodic { event });
        }

        // Sort by relevance (simplified)
        results.sort_by(|a, b| {
            let score_a = match a {
                types::MemoryResult::Semantic { score, .. } => *score,
                _ => 0.5,
            };
            let score_b = match b {
                types::MemoryResult::Semantic { score, .. } => *score,
                _ => 0.5,
            };
            score_b.partial_cmp(&score_a).unwrap()
        });

        Ok(results.into_iter().take(limit).collect())
    }

    /// Get context for the AI (combines all relevant memories)
    pub async fn get_context_for_query(
        &self,
        query: &str,
    ) -> Result<types::ContextBundle> {
        let relevant_memories = self.search(query, 10).await?;
        
        // Get project facts
        let project_facts = self.graph.lock().unwrap().get_project_facts().await?;
        
        // Get recent procedures
        let procedures = self.vector.search("procedure workflow", 5).await?;

        Ok(types::ContextBundle {
            query: query.to_string(),
            relevant_memories,
            project_facts,
            recent_procedures: procedures.into_iter()
                .map(|r| r.text)
                .collect(),
            session_id: self.session_id.clone(),
        })
    }

    /// Run memory consolidation (cleanup old/summarize)
    pub async fn consolidate(&mut self) -> Result<consolidation::ConsolidationReport> {
        consolidation::run_consolidation(self).await
    }

    /// Get memory statistics
    pub fn get_stats(&self) -> types::MemoryStats {
        types::MemoryStats {
            events_count: self.event_store.len(),
            graph_entities: self.graph.lock().unwrap().entity_count(),
            vector_documents: self.vector.document_count(),
            session_id: self.session_id.clone(),
            total_memories: self.event_store.len(),
            size_bytes: 0, // TODO: Calculate actual size
            last_updated: std::time::SystemTime::now(),
        }
    }
}

use uuid;
