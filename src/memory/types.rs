//! Memory Types - Data structures for the 5 memory types
//!
//! Episodic, Semantic, Procedural, Factual, Working

use super::event_store::MemoryEvent;
use std::collections::HashMap;

/// The 5 types of memory results
#[derive(Debug, Clone)]
pub enum MemoryResult {
    /// Episodic: Specific past events with timestamps
    Episodic { event: MemoryEvent },
    /// Semantic: General knowledge and facts
    Semantic {
        content: String,
        score: f32,
        metadata: HashMap<String, String>,
    },
    /// Graph-based: Entities and relationships
    Graph {
        entity: String,
        entity_type: String,
        properties: HashMap<String, String>,
    },
}

/// A learned procedure (procedural memory)
#[derive(Debug, Clone)]
pub struct Procedure {
    pub name: String,
    pub steps: Vec<String>,
    pub context: String,
    pub created_at: std::time::SystemTime,
    pub success_count: u32,
}

/// Context bundle sent to AI
#[derive(Debug, Clone)]
pub struct ContextBundle {
    pub query: String,
    pub relevant_memories: Vec<MemoryResult>,
    pub project_facts: HashMap<String, String>,
    pub recent_procedures: Vec<String>,
    pub session_id: String,
}

impl ContextBundle {
    /// Format the context for the AI
    pub fn format_for_llm(&self) -> String {
        let mut context = String::new();

        // Project facts
        if !self.project_facts.is_empty() {
            context.push_str("## Project Facts:\n");
            for (key, value) in &self.project_facts {
                context.push_str(&format!("- {}: {}\n", key, value));
            }
            context.push('\n');
        }

        // Relevant memories
        if !self.relevant_memories.is_empty() {
            context.push_str("## Relevant Context:\n");
            for memory in &self.relevant_memories {
                match memory {
                    MemoryResult::Episodic { event } => {
                        match event {
                            crate::memory::MemoryEvent::Interaction { query, response, .. } => {
                                if !query.is_empty() {
                                    context.push_str(&format!("- Q: {}\n  A: {}\n", query, response));
                                } else {
                                    // Keyword search results (no query, just content)
                                    context.push_str(&format!("- {}\n", response));
                                }
                            }
                            _ => {
                                context.push_str(&format!("- Past event: {:?}\n", event));
                            }
                        }
                    }
                    MemoryResult::Semantic { content, .. } => {
                        context.push_str(&format!("- {}\n", content));
                    }
                    MemoryResult::Graph {
                        entity, properties, ..
                    } => {
                        context.push_str(&format!("- Entity {}: {:?}\n", entity, properties));
                    }
                }
            }
            context.push('\n');
        }

        // Recent procedures
        if !self.recent_procedures.is_empty() {
            context.push_str("## Known Procedures:\n");
            for proc in &self.recent_procedures {
                context.push_str(&format!("- {}\n", proc));
            }
            context.push('\n');
        }

        context
    }
}

/// Memory system statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub events_count: usize,
    pub graph_entities: usize,
    pub vector_documents: usize,
    pub session_id: String,
    pub total_memories: usize,
    pub size_bytes: u64,
    pub last_updated: std::time::SystemTime,
}

impl MemoryStats {
    pub fn format(&self) -> String {
        format!(
            "Memory Stats:\n\
            - Events: {}\n\
            - Graph Entities: {}\n\
            - Vector Documents: {}\n\
            - Session: {}",
            self.events_count, self.graph_entities, self.vector_documents, self.session_id
        )
    }
}
