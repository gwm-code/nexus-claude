use crate::config::ProviderConfig;
use crate::error::{NexusError, Result};
use crate::providers::{CompletionRequest, CompletionResponse, ModelInfo, ModelPricing, Provider, ProviderInfo, StreamChunk, Usage};
use async_trait::async_trait;
use reqwest::Client;
use serde_json;

pub struct OpencodeProvider {
    api_key: Option<String>,
    base_url: String,
    client: Client,
    default_model: String,
    authenticated: bool,
}

impl OpencodeProvider {
    pub fn new(config: &ProviderConfig) -> Self {
        let api_key = config.api_key.clone();
        let authenticated = api_key.is_some();
        let base_url = config.base_url.clone().unwrap_or_else(|| {
            "https://opencode.ai/zen/v1".to_string()
        });
        
        Self {
            api_key,
            base_url,
            client: Client::new(),
            default_model: config.default_model.clone().unwrap_or_else(|| {
                "kimi-k2.5".to_string()
            }),
            authenticated,
        }
    }

    pub fn static_info() -> ProviderInfo {
        ProviderInfo {
            name: "opencode".to_string(),
            display_name: "OpenCode Zen".to_string(),
            supports_oauth: false,
            default_model: "kimi-k2.5".to_string(),
            available_models: vec![
                "kimi-k2.5".to_string(),
                "kimi-k2.5-free".to_string(),
                "kimi-k2".to_string(),
                "kimi-k2-thinking".to_string(),
                "gpt-5.2".to_string(),
                "gpt-5.2-codex".to_string(),
                "gpt-5.1".to_string(),
                "gpt-5.1-codex".to_string(),
                "gpt-5".to_string(),
                "gpt-5-nano".to_string(),
                "claude-sonnet-4-5".to_string(),
                "claude-sonnet-4".to_string(),
                "claude-haiku-4-5".to_string(),
                "claude-opus-4-5".to_string(),
                "gemini-3-pro".to_string(),
                "gemini-3-flash".to_string(),
                "glm-4.7".to_string(),
                "glm-4.7-free".to_string(),
                "qwen3-coder".to_string(),
                "minimax-m2.1".to_string(),
            ],
        }
    }
}

#[async_trait]
impl Provider for OpencodeProvider {
    fn info(&self) -> ProviderInfo {
        Self::static_info()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        if !self.is_authenticated() {
            return Err(NexusError::Authentication(
                "OpenCode API key not configured".to_string()
            ));
        }

        let api_key = self.api_key.as_ref().unwrap();
        
        // Use model ID as-is (already plain format without prefix)
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
                "OpenCode API error: {}",
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

    async fn complete_stream(
        &self,
        request: CompletionRequest,
        tx: tokio::sync::mpsc::Sender<StreamChunk>,
    ) -> Result<()> {
        if !self.is_authenticated() {
            return Err(NexusError::Authentication("OpenCode API key not configured".into()));
        }

        let api_key = self.api_key.as_ref().unwrap();
        let body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature.unwrap_or(0.7),
            "max_tokens": request.max_tokens,
            "stream": true,
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
            return Err(NexusError::ApiRequest(format!("OpenCode streaming error: {}", error_text)));
        }

        // Parse SSE stream
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| NexusError::ApiRequest(e.to_string()))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // Process complete SSE lines
            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.starts_with("data: ") {
                    let data = &line[6..];
                    if data == "[DONE]" {
                        let _ = tx.send(StreamChunk::Done).await;
                        return Ok(());
                    }
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(delta) = json["choices"][0]["delta"]["content"].as_str() {
                            if !delta.is_empty() {
                                let _ = tx.send(StreamChunk::ContentDelta(delta.to_string())).await;
                            }
                        }
                        if let Some(usage) = json.get("usage") {
                            let _ = tx.send(StreamChunk::Usage(Usage {
                                prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                                completion_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
                                total_tokens: usage["total_tokens"].as_u64().unwrap_or(0) as u32,
                            })).await;
                        }
                    }
                }
            }
        }

        let _ = tx.send(StreamChunk::Done).await;
        Ok(())
    }

    async fn list_available_models(&self) -> Result<Vec<ModelInfo>> {
        // OpenCode/Zen doesn't have a public models API
        // Providing comprehensive hardcoded list of available models
        Ok(vec![
            ModelInfo {
                id: "kimi-k2.5".to_string(),
                name: "Kimi K2.5".to_string(),
                description: Some("Latest Kimi model with enhanced capabilities".to_string()),
                context_length: Some(128000),
                pricing: None,
                supports_vision: false,
                supports_streaming: true,
                supports_function_calling: true,
            },
            ModelInfo {
                id: "kimi-k2.5-free".to_string(),
                name: "Kimi K2.5 Free".to_string(),
                description: Some("Free tier Kimi K2.5".to_string()),
                context_length: Some(128000),
                pricing: Some(ModelPricing {
                    prompt: Some(0.0),
                    completion: Some(0.0),
                }),
                supports_vision: false,
                supports_streaming: true,
                supports_function_calling: true,
            },
            ModelInfo {
                id: "deepseek-v3".to_string(),
                name: "DeepSeek V3".to_string(),
                description: Some("DeepSeek's latest coding-focused model".to_string()),
                context_length: Some(64000),
                pricing: None,
                supports_vision: false,
                supports_streaming: true,
                supports_function_calling: true,
            },
            ModelInfo {
                id: "qwen3-coder".to_string(),
                name: "Qwen 3 Coder".to_string(),
                description: Some("Qwen's specialized coding model".to_string()),
                context_length: Some(32000),
                pricing: None,
                supports_vision: false,
                supports_streaming: true,
                supports_function_calling: true,
            },
            ModelInfo {
                id: "glm-4.7".to_string(),
                name: "GLM 4.7".to_string(),
                description: Some("ChatGLM latest generation".to_string()),
                context_length: Some(128000),
                pricing: None,
                supports_vision: false,
                supports_streaming: true,
                supports_function_calling: true,
            },
            ModelInfo {
                id: "glm-4.7-free".to_string(),
                name: "GLM 4.7 Free".to_string(),
                description: Some("Free tier GLM 4.7".to_string()),
                context_length: Some(128000),
                pricing: Some(ModelPricing {
                    prompt: Some(0.0),
                    completion: Some(0.0),
                }),
                supports_vision: false,
                supports_streaming: true,
                supports_function_calling: true,
            },
        ])
    }

    async fn authenticate(&mut self) -> Result<()> {
        Ok(())
    }

    async fn refresh_auth(&mut self) -> Result<()> {
        // API keys don't expire, so no refresh needed
        Ok(())
    }

    fn is_authenticated(&self) -> bool {
        self.authenticated
    }
}
