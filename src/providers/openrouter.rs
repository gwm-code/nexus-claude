use crate::config::ProviderConfig;
use crate::error::{NexusError, Result};
use crate::providers::{CompletionRequest, CompletionResponse, Message, Provider, ProviderInfo, Role, Usage};
use async_trait::async_trait;
use reqwest::Client;
use serde_json;

pub struct OpenRouterProvider {
    api_key: Option<String>,
    base_url: String,
    client: Client,
    default_model: String,
    authenticated: bool,
}

impl OpenRouterProvider {
    pub fn new(config: &ProviderConfig) -> Self {
        let api_key = config.api_key.clone();
        let authenticated = api_key.is_some();
        let base_url = config.base_url.clone().unwrap_or_else(|| {
            "https://openrouter.ai/api/v1".to_string()
        });
        
        Self {
            api_key,
            base_url,
            client: Client::new(),
            default_model: config.default_model.clone().unwrap_or_else(|| {
                "anthropic/claude-3.5-sonnet".to_string()
            }),
            authenticated,
        }
    }

    pub fn static_info() -> ProviderInfo {
        ProviderInfo {
            name: "openrouter".to_string(),
            display_name: "OpenRouter".to_string(),
            supports_oauth: false,
            default_model: "openrouter/auto".to_string(),
            available_models: vec![
                "openrouter/auto".to_string(),           // Auto-select best available model
                "openrouter/auto:free".to_string(),      // Auto-select best free model
                "anthropic/claude-3.5-sonnet".to_string(),
                "anthropic/claude-3-opus".to_string(),
                "google/gemini-pro-1.5".to_string(),
                "google/gemini-flash-1.5".to_string(),
                "meta-llama/llama-3.1-405b-instruct".to_string(),
                "openai/gpt-4o".to_string(),
                "openai/gpt-4o-mini".to_string(),
                "deepseek/deepseek-chat".to_string(),
                "mistralai/mistral-large".to_string(),
            ],
        }
    }

    pub async fn fetch_available_models(&self) -> Result<Vec<String>> {
        let response = self.client
            .get(format!("{}/models", self.base_url))
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(NexusError::ApiRequest(format!(
                "OpenRouter API error: {}",
                error_text
            )));
        }

        let data: serde_json::Value = response.json().await?;
        let models: Vec<String> = data["data"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
            .collect();

        Ok(models)
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn info(&self) -> ProviderInfo {
        Self::static_info()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        if !self.is_authenticated() {
            return Err(NexusError::Authentication(
                "OpenRouter API key not configured".to_string()
            ));
        }

        let api_key = self.api_key.as_ref().unwrap();
        
        let body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature.unwrap_or(0.7),
            "max_tokens": request.max_tokens,
            "stream": request.stream.unwrap_or(false),
        });

        let response = self.client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(NexusError::ApiRequest(format!(
                "OpenRouter API error: {}",
                error_text
            )));
        }

        let data: serde_json::Value = response.json().await?;
        
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        
        let finish_reason = data["choices"][0]["finish_reason"]
            .as_str()
            .map(|s| s.to_string());

        let usage = if let Some(usage) = data.get("usage") {
            Some(Usage {
                prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                completion_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
                total_tokens: usage["total_tokens"].as_u64().unwrap_or(0) as u32,
            })
        } else {
            None
        };

        Ok(CompletionResponse {
            id: data["id"].as_str().unwrap_or("unknown").to_string(),
            model: request.model,
            content,
            finish_reason,
            usage,
            tool_calls: None,
        })
    }

    async fn authenticate(&mut self) -> Result<()> {
        // OpenRouter uses API key authentication
        Ok(())
    }

    async fn refresh_auth(&mut self) -> Result<()> {
        // API keys don't expire
        Ok(())
    }

    fn is_authenticated(&self) -> bool {
        self.authenticated
    }
}
