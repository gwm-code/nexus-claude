use crate::error::{NexusError, Result};
use crate::mcp::{Resource, ResourceContent};
use crate::mcp::ToolContent;
use serde_json::json;

/// Resource handler for MCP resources
pub struct ResourceHandler;

/// Resource content with metadata
#[derive(Debug, Clone)]
pub struct ResourceData {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
    pub size: usize,
    pub last_modified: Option<std::time::SystemTime>,
}

impl ResourceHandler {
    /// Create a new resource handler
    pub fn new() -> Self {
        Self
    }

    /// List available resources
    pub async fn list_resources(&self) -> Vec<Resource> {
        vec![
            Resource {
                uri: "file://workspace".to_string(),
                name: "Workspace Files".to_string(),
                description: Some("Access to files in the current workspace".to_string()),
                mime_type: Some("inode/directory".to_string()),
            },
            Resource {
                uri: "project://structure".to_string(),
                name: "Project Structure".to_string(),
                description: Some("Overview of the project structure and key files".to_string()),
                mime_type: Some("application/json".to_string()),
            },
            Resource {
                uri: "memory://recent".to_string(),
                name: "Recent Memories".to_string(),
                description: Some("Recently stored memories and context".to_string()),
                mime_type: Some("application/json".to_string()),
            },
            Resource {
                uri: "memory://all".to_string(),
                name: "All Memories".to_string(),
                description: Some("All stored memories".to_string()),
                mime_type: Some("application/json".to_string()),
            },
        ]
    }

    /// Read a resource by URI
    pub async fn read_resource(&self, uri: &str) -> Result<ResourceContent> {
        match uri {
            u if u.starts_with("file://") => self.read_file_resource(u).await,
            u if u.starts_with("project://") => self.read_project_resource(u).await,
            u if u.starts_with("memory://") => self.read_memory_resource(u).await,
            _ => Err(NexusError::Configuration(format!("Unknown resource scheme: {}", uri))),
        }
    }

    /// Subscribe to resource updates (not implemented yet)
    pub async fn subscribe(&self, _uri: &str) -> Result<()> {
        // Subscription not implemented in this version
        Ok(())
    }

    /// Unsubscribe from resource updates
    pub async fn unsubscribe(&self, _uri: &str) -> Result<()> {
        Ok(())
    }

    /// Read file:// resources
    async fn read_file_resource(&self, uri: &str) -> Result<ResourceContent> {
        let path_str = uri.strip_prefix("file://")
            .ok_or_else(|| NexusError::Configuration("Invalid file URI".to_string()))?;

        let path = std::path::PathBuf::from(path_str);
        
        // Handle special cases
        match path_str {
            "workspace" => {
                // Return workspace listing
                let entries = self.list_directory(".").await?;
                return Ok(ResourceContent {
                    uri: uri.to_string(),
                    mime_type: "text/plain".to_string(),
                    text: entries,
                });
            }
            _ => {}
        }

        // Regular file access
        if path.is_dir() {
            let entries = self.list_directory(&path.to_string_lossy()).await?;
            Ok(ResourceContent {
                uri: uri.to_string(),
                mime_type: "text/plain".to_string(),
                text: entries,
            })
        } else {
            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| NexusError::Io(e))?;
            
            let mime_type = Self::detect_mime_type(&path);
            
            Ok(ResourceContent {
                uri: uri.to_string(),
                mime_type,
                text: content,
            })
        }
    }

    /// Read project:// resources
    async fn read_project_resource(&self, uri: &str) -> Result<ResourceContent> {
        let resource_type = uri.strip_prefix("project://")
            .ok_or_else(|| NexusError::Configuration("Invalid project URI".to_string()))?;

        match resource_type {
            "structure" => {
                let structure = self.get_project_structure().await?;
                Ok(ResourceContent {
                    uri: uri.to_string(),
                    mime_type: "application/json".to_string(),
                    text: serde_json::to_string_pretty(&structure)?,
                })
            }
            "files" => {
                let files = self.list_directory(".").await?;
                Ok(ResourceContent {
                    uri: uri.to_string(),
                    mime_type: "text/plain".to_string(),
                    text: files,
                })
            }
            _ => Err(NexusError::Configuration(format!(
                "Unknown project resource: {}", resource_type
            ))),
        }
    }

    /// Read memory:// resources
    async fn read_memory_resource(&self, uri: &str) -> Result<ResourceContent> {
        let memory_type = uri.strip_prefix("memory://")
            .ok_or_else(|| NexusError::Configuration("Invalid memory URI".to_string()))?;

        match memory_type {
            "recent" => {
                // For now, return placeholder - would integrate with actual memory system
                let recent_memories = json!({
                    "memories": [],
                    "note": "Memory system integration not yet implemented"
                });
                
                Ok(ResourceContent {
                    uri: uri.to_string(),
                    mime_type: "application/json".to_string(),
                    text: serde_json::to_string_pretty(&recent_memories)?,
                })
            }
            "all" => {
                let all_memories = json!({
                    "memories": [],
                    "total": 0,
                    "note": "Memory system integration not yet implemented"
                });
                
                Ok(ResourceContent {
                    uri: uri.to_string(),
                    mime_type: "application/json".to_string(),
                    text: serde_json::to_string_pretty(&all_memories)?,
                })
            }
            _ => {
                // Try to read as specific memory ID
                Err(NexusError::Configuration(format!(
                    "Unknown memory resource: {}", memory_type
                )))
            }
        }
    }

    /// List directory contents
    async fn list_directory(&self, path: &str) -> Result<String> {
        let mut entries = Vec::new();
        let path_buf = std::path::PathBuf::from(path);

        let mut read_dir = tokio::fs::read_dir(&path_buf).await
            .map_err(|e| NexusError::Io(e))?;

        while let Some(entry) = read_dir.next_entry().await? {
            let file_type = entry.file_type().await?;
            let name = entry.file_name().to_string_lossy().to_string();
            
            let prefix = if file_type.is_dir() { "[DIR]  " } 
                        else if file_type.is_symlink() { "[LINK] " }
                        else { "[FILE] " };
            
            entries.push(format!("{}{}", prefix, name));
        }

        Ok(format!("Contents of {}:\n{}", path, entries.join("\n")))
    }

    /// Get project structure overview
    async fn get_project_structure(&self) -> Result<serde_json::Value> {
        let mut structure = serde_json::Map::new();

        // Check for common project files
        let project_files = [
            "Cargo.toml", "package.json", "pyproject.toml", 
            "setup.py", "Makefile", "README.md", "LICENSE"
        ];

        let mut root_files = Vec::new();
        for file in &project_files {
            if tokio::fs::metadata(file).await.is_ok() {
                root_files.push(file.to_string());
            }
        }

        structure.insert("root_files".to_string(), json!(root_files));

        // Get source directories
        let mut source_dirs = Vec::new();
        for dir in &["src", "lib", "app", "tests", "docs"] {
            if tokio::fs::metadata(dir).await.is_ok() {
                source_dirs.push(dir.to_string());
            }
        }

        structure.insert("source_directories".to_string(), json!(source_dirs));

        // Detect project type
        let project_type = if tokio::fs::metadata("Cargo.toml").await.is_ok() {
            "rust"
        } else if tokio::fs::metadata("package.json").await.is_ok() {
            "javascript"
        } else if tokio::fs::metadata("pyproject.toml").await.is_ok() 
            || tokio::fs::metadata("setup.py").await.is_ok() {
            "python"
        } else {
            "unknown"
        };

        structure.insert("project_type".to_string(), json!(project_type));
        structure.insert("uri".to_string(), json!("project://structure"));

        Ok(json!(structure))
    }

    /// Detect MIME type from file extension
    fn detect_mime_type(path: &std::path::Path) -> String {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| match ext.to_lowercase().as_str() {
                "rs" => "text/x-rust",
                "js" => "text/javascript",
                "ts" => "text/typescript",
                "py" => "text/x-python",
                "json" => "application/json",
                "yaml" | "yml" => "application/yaml",
                "toml" => "application/toml",
                "md" => "text/markdown",
                "txt" => "text/plain",
                "html" | "htm" => "text/html",
                "css" => "text/css",
                "xml" => "application/xml",
                "sh" => "text/x-shellscript",
                "dockerfile" => "text/x-dockerfile",
                _ => "text/plain",
            })
            .unwrap_or("text/plain")
            .to_string()
    }
}

/// Resource content wrapper for responses
#[derive(Debug, Clone)]
pub struct ResourceReadResult {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
}

impl From<ResourceContent> for ToolContent {
    fn from(content: ResourceContent) -> Self {
        ToolContent::Resource { resource: content }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mime_type_detection() {
        let handler = ResourceHandler::new();
        
        assert_eq!(
            ResourceHandler::detect_mime_type(std::path::Path::new("test.rs")),
            "text/x-rust"
        );
        
        assert_eq!(
            ResourceHandler::detect_mime_type(std::path::Path::new("test.json")),
            "application/json"
        );
        
        assert_eq!(
            ResourceHandler::detect_mime_type(std::path::Path::new("test.unknown")),
            "text/plain"
        );
    }
}
