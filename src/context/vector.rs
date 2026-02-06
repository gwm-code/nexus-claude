/// Vector store integration — re-exports from the real memory::semantic layer
///
/// The production vector store lives in `memory::semantic::VectorMemory`.
/// This module provides a thin re-export so existing `context::vector` imports
/// continue to resolve.

pub use crate::memory::semantic::{VectorMemory, SearchResult};
