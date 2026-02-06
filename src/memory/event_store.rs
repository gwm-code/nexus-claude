//! Event Store - Layer 1: Immutable audit trail
//! 
//! Stores every action, decision, and outcome as append-only log

use crate::error::{NexusError, Result};
use serde::{Serialize, Deserialize};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::SystemTime;

const MAX_EVENTS_IN_MEMORY: usize = 1000;

/// The immutable event store
pub struct EventStore {
    storage_path: PathBuf,
    events: VecDeque<MemoryEvent>,
    event_count: usize,
}

/// Types of memory events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MemoryEvent {
    ProjectInit {
        project_name: String,
        project_path: PathBuf,
        timestamp: SystemTime,
    },
    Interaction {
        session_id: String,
        query: String,
        response: String,
        tools_used: Vec<String>,
        timestamp: SystemTime,
    },
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
        result: serde_json::Value,
        success: bool,
        timestamp: SystemTime,
    },
    FactStored {
        entity: String,
        fact_type: String,
        value: String,
        timestamp: SystemTime,
    },
    ProcedureLearned {
        procedure_name: String,
        context: String,
        timestamp: SystemTime,
    },
    FileModified {
        path: PathBuf,
        operation: String, // create, edit, delete
        content_hash: String,
        timestamp: SystemTime,
    },
    Error {
        error_type: String,
        message: String,
        context: String,
        timestamp: SystemTime,
    },
}

impl EventStore {
    pub fn new(storage_path: PathBuf) -> Result<Self> {
        fs::create_dir_all(&storage_path)?;
        
        Ok(Self {
            storage_path,
            events: VecDeque::with_capacity(MAX_EVENTS_IN_MEMORY),
            event_count: 0,
        })
    }

    /// Log an event (immutable append)
    pub async fn log_event(&self, event: MemoryEvent) -> Result<()> {
        let event_json = serde_json::to_string(&event)
            .map_err(|e| NexusError::Json(e))?;
        
        // Append to log file (immutable)
        let log_file = self.storage_path.join("events.ndjson");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(|e| NexusError::Io(e))?;
        
        writeln!(file, "{}", event_json)
            .map_err(|e| NexusError::Io(e))?;
        
        // Also keep in memory (limited)
        // (In production, this would be a ring buffer)
        
        Ok(())
    }

    /// Get recent events
    pub async fn get_recent_events(&self, limit: usize) -> Result<Vec<MemoryEvent>> {
        let log_file = self.storage_path.join("events.ndjson");
        
        if !log_file.exists() {
            return Ok(vec![]);
        }
        
        let file = File::open(&log_file)
            .map_err(|e| NexusError::Io(e))?;
        let reader = BufReader::new(file);
        
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|e| NexusError::Io(e))?;
            if let Ok(event) = serde_json::from_str::<MemoryEvent>(&line) {
                events.push(event);
                if events.len() >= limit {
                    break;
                }
            }
        }
        
        // Reverse to get most recent first
        events.reverse();
        Ok(events)
    }

    /// Query events by type
    pub async fn query_by_type(&self, event_type: &str, limit: usize) -> Result<Vec<MemoryEvent>> {
        let all_events = self.get_recent_events(10000).await?;
        
        let filtered: Vec<MemoryEvent> = all_events.into_iter()
            .filter(|e| {
                let type_str = match e {
                    MemoryEvent::ProjectInit { .. } => "project_init",
                    MemoryEvent::Interaction { .. } => "interaction",
                    MemoryEvent::ToolCall { .. } => "tool_call",
                    MemoryEvent::FactStored { .. } => "fact_stored",
                    MemoryEvent::ProcedureLearned { .. } => "procedure_learned",
                    MemoryEvent::FileModified { .. } => "file_modified",
                    MemoryEvent::Error { .. } => "error",
                };
                type_str == event_type
            })
            .take(limit)
            .collect();
        
        Ok(filtered)
    }

    /// Get count of total events
    pub fn len(&self) -> usize {
        // Count lines in file
        let log_file = self.storage_path.join("events.ndjson");
        if !log_file.exists() {
            return 0;
        }
        
        match fs::read_to_string(&log_file) {
            Ok(content) => content.lines().count(),
            Err(_) => 0,
        }
    }
}
