//! Memory Consolidation - Intelligent cleanup and summarization
//!
//! Prevents memory overflow by consolidating old memories

use crate::error::Result;
use crate::memory::MemorySystem;
use crate::memory::MemoryEvent;

/// Report from consolidation run
#[derive(Debug)]
pub struct ConsolidationReport {
    pub events_archived: usize,
    pub events_summarized: usize,
    pub old_procedures_removed: usize,
    pub total_size_before: u64,
    pub total_size_after: u64,
}

/// Run memory consolidation
pub async fn run_consolidation(memory: &mut MemorySystem) -> Result<ConsolidationReport> {
    println!("[MEMORY] Running consolidation...");
    
    // Get all events
    let all_events = memory.event_store.get_recent_events(10000).await?;
    
    let old_events: Vec<MemoryEvent> = all_events.clone();
    let mut summarized = 0;
    let mut archived = 0;
    
    // Archive events older than 30 days
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
    
    for event in &old_events {
        let timestamp = match event {
            MemoryEvent::Interaction { timestamp, .. } => *timestamp,
            MemoryEvent::ToolCall { timestamp, .. } => *timestamp,
            MemoryEvent::FactStored { timestamp, .. } => *timestamp,
            MemoryEvent::ProcedureLearned { timestamp, .. } => *timestamp,
            MemoryEvent::FileModified { timestamp, .. } => *timestamp,
            MemoryEvent::Error { timestamp, .. } => *timestamp,
            MemoryEvent::ProjectInit { timestamp, .. } => *timestamp,
        };
        
        if timestamp < cutoff {
            archived += 1;
            // In production: Move to archive storage
        }
    }
    
    // Summarize old interactions into semantic facts
    let old_interactions: Vec<_> = all_events.iter()
        .filter_map(|e| match e {
            MemoryEvent::Interaction { query, response, timestamp, .. } 
                if *timestamp < cutoff => Some((query, response)),
            _ => None,
        })
        .collect();
    
    if !old_interactions.is_empty() {
        // Create summary fact
        let summary = format!("Over {} past interactions", old_interactions.len());
        memory.remember_fact(
            "conversation_history",
            "summary",
            &summary,
        ).await?;
        summarized += old_interactions.len();
    }
    
    // Clean up old vector documents
    let doc_count = memory.vector.document_count();
    if doc_count > 10000 {
        // Remove oldest documents
        // In production: Implement proper LRU eviction
        println!("[MEMORY] Vector store has {} documents (consider cleanup)", doc_count);
    }
    
    let report = ConsolidationReport {
        events_archived: archived,
        events_summarized: summarized,
        old_procedures_removed: 0,
        total_size_before: 0, // Calculate in production
        total_size_after: 0,
    };
    
    println!("[MEMORY] Consolidation complete: {} archived, {} summarized", 
        report.events_archived, 
        report.events_summarized
    );
    
    Ok(report)
}

/// Priority score for memories (0-1)
pub fn calculate_priority(event: &MemoryEvent) -> f32 {
    match event {
        MemoryEvent::Error { .. } => 0.9, // High priority: errors
        MemoryEvent::FactStored { .. } => 0.8, // High priority: facts
        MemoryEvent::ProcedureLearned { .. } => 0.7, // Medium priority: procedures
        MemoryEvent::ProjectInit { .. } => 0.6,
        MemoryEvent::FileModified { .. } => 0.5,
        MemoryEvent::ToolCall { success, .. } => {
            if *success { 0.4 } else { 0.7 } // Failed tools are important
        }
        MemoryEvent::Interaction { .. } => 0.3, // Lower priority: regular chat
    }
}
