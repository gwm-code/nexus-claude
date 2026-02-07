use crate::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod claude;
pub mod google;
pub mod model_capabilities;
pub mod opencode;
pub mod openrouter;
pub mod retry;
pub mod token_budget;

/// A chunk from a streaming completion response
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// Incremental content delta
    ContentDelta(String),
    /// Usage information (sent at the end)
    Usage(Usage),
    /// Stream is done
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Function,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_params: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub id: String,
    pub model: String,
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<crate::executor::tools::ToolCall>>,
}

impl CompletionResponse {
    pub fn new(id: String, model: String, content: String) -> Self {
        Self {
            id,
            model,
            content,
            finish_reason: None,
            usage: None,
            tool_calls: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: String,
    pub display_name: String,
    pub supports_oauth: bool,
    pub default_model: String,
    pub available_models: Vec<String>,
}

/// Detailed information about a specific model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ModelPricing>,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_function_calling: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Price per million prompt tokens (USD)
    pub prompt: Option<f64>,
    /// Price per million completion tokens (USD)
    pub completion: Option<f64>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn info(&self) -> ProviderInfo;

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;

    /// Stream a completion response. Default wraps `complete()` as a single chunk.
    async fn complete_stream(
        &self,
        request: CompletionRequest,
        tx: tokio::sync::mpsc::Sender<StreamChunk>,
    ) -> Result<()> {
        let resp = self.complete(request).await?;
        let _ = tx.send(StreamChunk::ContentDelta(resp.content)).await;
        if let Some(usage) = resp.usage {
            let _ = tx.send(StreamChunk::Usage(usage)).await;
        }
        let _ = tx.send(StreamChunk::Done).await;
        Ok(())
    }

    /// Fetch available models from the provider's API
    /// Returns detailed model information including pricing, context limits, etc.
    /// Default implementation returns an empty list (provider should override)
    async fn list_available_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(Vec::new())
    }

    async fn authenticate(&mut self) -> Result<()>;

    async fn refresh_auth(&mut self) -> Result<()>;

    fn is_authenticated(&self) -> bool;
}

pub fn create_provider(
    provider_type: &crate::config::ProviderType,
    config: &crate::config::ProviderConfig,
) -> Result<Box<dyn Provider>> {
    use crate::config::ProviderType;
    
    match provider_type {
        ProviderType::Opencode => Ok(Box::new(opencode::OpencodeProvider::new(config))),
        ProviderType::Openrouter => Ok(Box::new(openrouter::OpenRouterProvider::new(config))),
        ProviderType::Google => Ok(Box::new(google::GoogleProvider::new(config))),
        ProviderType::Claude => Ok(Box::new(claude::ClaudeProvider::new(config))),
    }
}

pub fn create_provider_arc(
    provider_type: &crate::config::ProviderType,
    config: &crate::config::ProviderConfig,
) -> Result<std::sync::Arc<dyn Provider + Send + Sync>> {
    use crate::config::ProviderType;
    
    match provider_type {
        ProviderType::Opencode => Ok(std::sync::Arc::new(opencode::OpencodeProvider::new(config))),
        ProviderType::Openrouter => Ok(std::sync::Arc::new(openrouter::OpenRouterProvider::new(config))),
        ProviderType::Google => Ok(std::sync::Arc::new(google::GoogleProvider::new(config))),
        ProviderType::Claude => Ok(std::sync::Arc::new(claude::ClaudeProvider::new(config))),
    }
}

pub fn list_available_providers() -> Vec<ProviderInfo> {
    vec![
        opencode::OpencodeProvider::static_info(),
        openrouter::OpenRouterProvider::static_info(),
        google::GoogleProvider::static_info(),
        claude::ClaudeProvider::static_info(),
    ]
}
