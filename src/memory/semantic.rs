//! Vector Memory - Layer 3: Semantic search and embeddings
//!
//! Stores text embeddings for similarity search

use crate::error::{NexusError, Result};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;

/// Vector-based semantic memory
pub struct VectorMemory {
    storage_path: PathBuf,
    documents: Vec<Document>,
    next_id: usize,
}

/// A document in the vector store
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub text: String,
    // In production, this would be the actual embedding vector
    // For now, we use simple text-based similarity
    pub embedding: Vec<f32>,
    pub metadata: HashMap<String, String>,
}

/// Search result from vector store
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub text: String,
    pub score: f32,
    pub metadata: HashMap<String, String>,
}

impl VectorMemory {
    pub fn new(storage_path: PathBuf) -> Result<Self> {
        fs::create_dir_all(&storage_path)?;
        
        // Load existing documents
        let documents = Self::load_documents(&storage_path)?;
        let next_id = documents.len();
        
        Ok(Self {
            storage_path,
            documents,
            next_id,
        })
    }

    /// Index a new document
    pub async fn index_document(
        &mut self,
        id: &str,
        text: &str,
        metadata: HashMap<String, String>,
    ) -> Result<()> {
        // Create simple embedding (word frequency based for MVP)
        // In production, use proper embedding model like OpenAI or local
        let embedding = self.create_embedding(text);
        
        let doc = Document {
            id: id.to_string(),
            text: text.to_string(),
            embedding,
            metadata,
        };
        
        self.documents.push(doc);
        self.next_id += 1;
        
        // Save to disk
        self.save_documents().await?;
        
        Ok(())
    }

    /// Search for similar documents
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let query_embedding = self.create_embedding(query);
        
        // Calculate cosine similarity for each document
        let mut results: Vec<(Document, f32)> = self.documents.iter()
            .map(|doc| {
                let score = cosine_similarity(&query_embedding, &doc.embedding);
                (doc.clone(), score)
            })
            .collect();
        
        // Sort by score (highest first)
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        
        // Take top N
        let top_results: Vec<SearchResult> = results.into_iter()
            .take(limit)
            .map(|(doc, score)| SearchResult {
                id: doc.id,
                text: doc.text,
                score,
                metadata: doc.metadata,
            })
            .collect();
        
        Ok(top_results)
    }

    /// Get document count
    pub fn document_count(&self) -> usize {
        self.documents.len()
    }

    /// Create a simple embedding (MVP - word-based)
    /// In production, use actual embedding model
    fn create_embedding(&self, text: &str) -> Vec<f32> {
        // Simple bag-of-words approach
        // In production, use: OpenAI embeddings, sentence-transformers, etc.
        let text_lower = text.to_lowercase();
        let words: Vec<&str> = text_lower
            .split_whitespace()
            .collect();
        
        // Create a simple hash-based embedding
        // This is just for demonstration - real embeddings are much more sophisticated
        let mut embedding = vec![0.0f32; 128]; // 128-dimensional vector
        
        for word in &words {
            let hash = Self::hash_string(word);
            let idx = (hash % 128) as usize;
            embedding[idx] += 1.0;
        }
        
        // Normalize
        let magnitude = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if magnitude > 0.0 {
            for val in embedding.iter_mut() {
                *val /= magnitude;
            }
        }
        
        embedding
    }

    fn hash_string(s: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }

    // Private helpers
    fn load_documents(path: &PathBuf) -> Result<Vec<Document>> {
        let file = path.join("documents.json");
        if file.exists() {
            let content = fs::read_to_string(&file)
                .map_err(|e| NexusError::Io(e))?;
            let docs: Vec<Document> = serde_json::from_str(&content)
                .map_err(|e| NexusError::Json(e))?;
            Ok(docs)
        } else {
            Ok(vec![])
        }
    }

    async fn save_documents(&self) -> Result<()> {
        let file = self.storage_path.join("documents.json");
        let json = serde_json::to_string_pretty(&self.documents)
            .map_err(|e| NexusError::Json(e))?;
        fs::write(&file, json)
            .map_err(|e| NexusError::Io(e))?;
        Ok(())
    }
}

/// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let magnitude_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let magnitude_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    
    if magnitude_a == 0.0 || magnitude_b == 0.0 {
        return 0.0;
    }
    
    dot_product / (magnitude_a * magnitude_b)
}
