use crate::config::ProviderConfig;
use crate::error::{NexusError, Result};
use crate::providers::{CompletionRequest, CompletionResponse, Message, ModelInfo, ModelPricing, Provider, ProviderInfo, Usage};
use async_trait::async_trait;
use oauth2::{
    AuthUrl, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope,
    TokenResponse, TokenUrl, RefreshToken,
    basic::BasicClient,
    AuthorizationCode, reqwest as oauth2_reqwest,
};
use reqwest::Client;
use serde_json;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

pub struct GoogleProvider {
    api_key: Option<String>,
    oauth_token: Option<String>,
    oauth_refresh_token: Option<String>,
    oauth_client_id: Option<String>,
    oauth_client_secret: Option<String>,
    base_url: String,
    client: Client,
    default_model: String,
}

impl GoogleProvider {
    const GOOGLE_AUTH_URL: &'static str = "https://accounts.google.com/o/oauth2/v2/auth";
    const GOOGLE_TOKEN_URL: &'static str = "https://oauth2.googleapis.com/token";
    const REDIRECT_URL: &'static str = "http://localhost:8080/callback";
    const SCOPES: &'static [&'static str] = &[
        "https://www.googleapis.com/auth/generative-language",
    ];

    pub fn new(config: &ProviderConfig) -> Self {
        Self {
            api_key: config.api_key.clone(),
            oauth_token: config.oauth_token.clone(),
            oauth_refresh_token: config.oauth_refresh_token.clone(),
            oauth_client_id: config.oauth_client_id.clone(),
            oauth_client_secret: config.oauth_client_secret.clone(),
            base_url: config.base_url.clone().unwrap_or_else(|| {
                "https://generativelanguage.googleapis.com/v1beta".to_string()
            }),
            client: Client::new(),
            default_model: config.default_model.clone().unwrap_or_else(|| {
                "gemini-1.5-pro".to_string()
            }),
        }
    }

    pub fn static_info() -> ProviderInfo {
        ProviderInfo {
            name: "google".to_string(),
            display_name: "Google Gemini".to_string(),
            supports_oauth: true,
            default_model: "gemini-1.5-pro".to_string(),
            available_models: vec![
                "gemini-1.5-pro".to_string(),
                "gemini-1.5-flash".to_string(),
                "gemini-1.0-pro".to_string(),
                "gemini-1.0-pro-vision".to_string(),
            ],
        }
    }

    pub fn generate_auth_url(&self) -> Result<String> {
        let client_id = self.oauth_client_id.as_ref().ok_or_else(|| {
            NexusError::OAuth("OAuth client_id not configured".to_string())
        })?;
        let client_secret = self.oauth_client_secret.as_ref().ok_or_else(|| {
            NexusError::OAuth("OAuth client_secret not configured".to_string())
        })?;

        let client = BasicClient::new(ClientId::new(client_id.clone()))
            .set_client_secret(ClientSecret::new(client_secret.clone()))
            .set_auth_uri(AuthUrl::new(Self::GOOGLE_AUTH_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid auth URL: {}", e)))?)
            .set_token_uri(TokenUrl::new(Self::GOOGLE_TOKEN_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid token URL: {}", e)))?)
            .set_redirect_uri(RedirectUrl::new(Self::REDIRECT_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid redirect URL: {}", e)))?);

        let (auth_url, _csrf_token) = client
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new(Self::SCOPES[0].to_string()))
            .add_scope(Scope::new("openid".to_string()))
            .url();

        Ok(auth_url.to_string())
    }

    pub async fn exchange_code(&mut self, code: &str) -> Result<()> {
        let client_id = self.oauth_client_id.as_ref().ok_or_else(|| {
            NexusError::OAuth("OAuth client_id not configured".to_string())
        })?;
        let client_secret = self.oauth_client_secret.as_ref().ok_or_else(|| {
            NexusError::OAuth("OAuth client_secret not configured".to_string())
        })?;

        let client = BasicClient::new(ClientId::new(client_id.clone()))
            .set_client_secret(ClientSecret::new(client_secret.clone()))
            .set_auth_uri(AuthUrl::new(Self::GOOGLE_AUTH_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid auth URL: {}", e)))?)
            .set_token_uri(TokenUrl::new(Self::GOOGLE_TOKEN_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid token URL: {}", e)))?)
            .set_redirect_uri(RedirectUrl::new(Self::REDIRECT_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid redirect URL: {}", e)))?);

        let http_client = oauth2_reqwest::ClientBuilder::new()
            .redirect(oauth2_reqwest::redirect::Policy::none())
            .build()
            .expect("Client should build");

        let token = client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .request_async(&http_client)
            .await
            .map_err(|e| NexusError::OAuth(format!("Token exchange failed: {}", e)))?;

        self.oauth_token = Some(token.access_token().secret().clone());
        
        if let Some(refresh) = token.refresh_token() {
            self.oauth_refresh_token = Some(refresh.secret().clone());
        }

        Ok(())
    }

    pub async fn perform_full_oauth_flow(&mut self) -> Result<()> {
        let auth_url = self.generate_auth_url()?;
        
        println!("Opening browser for Google OAuth authorization...");
        println!("If browser doesn't open, visit this URL manually: {}", auth_url);
        
        // Try to open browser
        if let Err(_) = open::that(&auth_url) {
            println!("Could not open browser automatically.");
        }

        // Start local server to receive callback
        let listener = TcpListener::bind("127.0.0.1:8080")
            .map_err(|e| NexusError::OAuth(format!("Failed to bind callback server: {}", e)))?;

        println!("Waiting for OAuth callback on http://localhost:8080/callback...");

        let (mut stream, _) = listener.accept()
            .map_err(|e| NexusError::OAuth(format!("Failed to accept connection: {}", e)))?;

        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line)
            .map_err(|e| NexusError::OAuth(format!("Failed to read request: {}", e)))?;

        // Extract code from query string
        let code = line.split_whitespace()
            .nth(1)
            .and_then(|path| {
                let parts: Vec<&str> = path.split("?").collect();
                if parts.len() > 1 {
                    parts[1].split("&")
                        .find(|param| param.starts_with("code="))
                        .map(|param| &param[5..])
                        .map(|code| urlencoding::decode(code).ok())
                        .flatten()
                        .map(|decoded| decoded.to_string())
                } else {
                    None
                }
            })
            .ok_or_else(|| NexusError::OAuth("Authorization code not found in callback".to_string()))?;

        // Send response
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
            <html><body><h1>Authorization Successful</h1><p>You can close this window.</p></body></html>";
        stream.write_all(response.as_bytes())
            .map_err(|e| NexusError::OAuth(format!("Failed to send response: {}", e)))?;

        // Exchange code for token
        self.exchange_code(&code).await?;
        
        println!("OAuth authorization completed successfully!");
        
        Ok(())
    }

    pub async fn refresh_oauth_token(&mut self) -> Result<()> {
        let refresh_token = self.oauth_refresh_token.as_ref().ok_or_else(|| {
            NexusError::OAuth("No refresh token available".to_string())
        })?;

        let client_id = self.oauth_client_id.as_ref().ok_or_else(|| {
            NexusError::OAuth("OAuth client_id not configured".to_string())
        })?;
        let client_secret = self.oauth_client_secret.as_ref().ok_or_else(|| {
            NexusError::OAuth("OAuth client_secret not configured".to_string())
        })?;

        let client = BasicClient::new(ClientId::new(client_id.clone()))
            .set_client_secret(ClientSecret::new(client_secret.clone()))
            .set_auth_uri(AuthUrl::new(Self::GOOGLE_AUTH_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid auth URL: {}", e)))?)
            .set_token_uri(TokenUrl::new(Self::GOOGLE_TOKEN_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid token URL: {}", e)))?)
            .set_redirect_uri(RedirectUrl::new(Self::REDIRECT_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid redirect URL: {}", e)))?);

        let http_client = oauth2_reqwest::ClientBuilder::new()
            .redirect(oauth2_reqwest::redirect::Policy::none())
            .build()
            .expect("Client should build");

        let token = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.clone()))
            .request_async(&http_client)
            .await
            .map_err(|e| NexusError::OAuth(format!("Token refresh failed: {}", e)))?;

        self.oauth_token = Some(token.access_token().secret().clone());
        
        // Update refresh token if a new one was provided
        if let Some(new_refresh) = token.refresh_token() {
            self.oauth_refresh_token = Some(new_refresh.secret().clone());
        }

        Ok(())
    }

    fn convert_messages(&self, messages: &[Message]) -> Vec<serde_json::Value> {
        messages.iter().map(|msg| {
            serde_json::json!({
                "role": match msg.role {
                    crate::providers::Role::User => "user",
                    crate::providers::Role::Assistant => "model",
                    _ => "user",
                },
                "parts": [{"text": msg.content}]
            })
        }).collect()
    }

    fn get_auth_header(&self) -> Result<(String, String)> {
        if let Some(token) = &self.oauth_token {
            Ok(("Authorization".to_string(), format!("Bearer {}", token)))
        } else if let Some(key) = &self.api_key {
            // For API key, we'll add it as query param in the URL
            Ok(("X-API-Key".to_string(), key.clone()))
        } else {
            Err(NexusError::Authentication(
                "Google API key or OAuth token not configured".to_string()
            ))
        }
    }
}

#[async_trait]
impl Provider for GoogleProvider {
    fn info(&self) -> ProviderInfo {
        Self::static_info()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let contents = self.convert_messages(&request.messages);
        
        let body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "temperature": request.temperature.unwrap_or(0.7),
                "maxOutputTokens": request.max_tokens,
            }
        });

        let model_name = if request.model.starts_with("models/") {
            request.model.clone()
        } else {
            format!("models/{}", request.model)
        };

        // Build request with appropriate auth
        let (auth_header, auth_value) = self.get_auth_header()?;
        
        let url = if auth_header == "X-API-Key" {
            format!("{}/{}/generateContent?key={}", self.base_url, model_name, auth_value)
        } else {
            format!("{}/{}/generateContent", self.base_url, model_name)
        };

        let mut req = self.client
            .post(&url)
            .header("Content-Type", "application/json");

        if auth_header == "Authorization" {
            req = req.header(&auth_header, &auth_value);
        }

        let response = req
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(NexusError::ApiRequest(format!(
                "Google API error: {}",
                error_text
            )));
        }

        let data: serde_json::Value = response.json().await?;
        
        let content = data["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let finish_reason = data["candidates"][0]["finishReason"]
            .as_str()
            .map(|s| s.to_string());

        let usage = if let Some(usage) = data.get("usageMetadata") {
            Some(Usage {
                prompt_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0) as u32,
                completion_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0) as u32,
                total_tokens: usage["totalTokenCount"].as_u64().unwrap_or(0) as u32,
            })
        } else {
            None
        };

        Ok(CompletionResponse {
            id: data["candidates"][0]["content"]["role"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            model: request.model,
            content,
            finish_reason,
            usage,
            tool_calls: None,
        })
    }

    async fn list_available_models(&self) -> Result<Vec<ModelInfo>> {
        // Google's models API: https://generativelanguage.googleapis.com/v1beta/models
        let mut url = format!("{}/models", self.base_url);

        // Add authentication (API key or OAuth token)
        let response = if let Some(api_key) = &self.api_key {
            url.push_str(&format!("?key={}", api_key));
            self.client.get(&url).send().await?
        } else if let Some(oauth_token) = &self.oauth_token {
            self.client
                .get(&url)
                .header("Authorization", format!("Bearer {}", oauth_token))
                .send()
                .await?
        } else {
            return Err(NexusError::Authentication(
                "Google API key or OAuth token required to list models".to_string()
            ));
        };

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(NexusError::ApiRequest(format!(
                "Google API error: {}",
                error_text
            )));
        }

        let data: serde_json::Value = response.json().await?;
        let models: Vec<ModelInfo> = data["models"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| {
                let full_name = m["name"].as_str()?;
                // Extract model ID from "models/gemini-3.0-pro" format
                let id = full_name.strip_prefix("models/")?.to_string();
                let display_name = m["displayName"].as_str().unwrap_or(&id).to_string();
                let description = m["description"].as_str().map(|s| s.to_string());

                // Parse context length (inputTokenLimit)
                let context_length = m["inputTokenLimit"].as_u64().map(|n| n as u32);

                // Check if model supports vision (multimodal)
                let supports_vision = m["supportedGenerationMethods"]
                    .as_array()
                    .map(|methods| {
                        methods.iter().any(|method| {
                            method.as_str().map(|s| s.contains("vision") || s.contains("multimodal")).unwrap_or(false)
                        })
                    })
                    .unwrap_or(id.contains("vision") || id.contains("pro")); // Fallback heuristic

                // Gemini models support streaming
                let supports_streaming = m["supportedGenerationMethods"]
                    .as_array()
                    .map(|methods| {
                        methods.iter().any(|method| {
                            method.as_str().map(|s| s.contains("streamGenerateContent")).unwrap_or(false)
                        })
                    })
                    .unwrap_or(true); // Most Gemini models support streaming

                // Gemini models support function calling
                let supports_function_calling = m["supportedGenerationMethods"]
                    .as_array()
                    .map(|methods| {
                        methods.iter().any(|method| {
                            method.as_str().map(|s| s.contains("generateContent")).unwrap_or(false)
                        })
                    })
                    .unwrap_or(true);

                Some(ModelInfo {
                    id,
                    name: display_name,
                    description,
                    context_length,
                    pricing: None, // Google doesn't expose pricing in models API
                    supports_vision,
                    supports_streaming,
                    supports_function_calling,
                })
            })
            .collect();

        Ok(models)
    }

    async fn authenticate(&mut self) -> Result<()> {
        // If we have an API key, we're authenticated
        if self.api_key.is_some() {
            return Ok(());
        }

        // If we have an OAuth token, check if it's still valid
        if self.oauth_token.is_some() {
            // Try to use it - if it fails, refresh
            return Ok(());
        }

        // Perform full OAuth flow
        self.perform_full_oauth_flow().await
    }

    async fn refresh_auth(&mut self) -> Result<()> {
        // API keys don't expire
        if self.api_key.is_some() {
            return Ok(());
        }

        // Refresh OAuth token
        if self.oauth_token.is_some() && self.oauth_refresh_token.is_some() {
            return self.refresh_oauth_token().await;
        }

        // If we have OAuth client but no tokens, do full flow
        if self.oauth_client_id.is_some() && self.oauth_client_secret.is_some() {
            return self.perform_full_oauth_flow().await;
        }

        Err(NexusError::Authentication(
            "No authentication method available".to_string()
        ))
    }

    fn is_authenticated(&self) -> bool {
        self.api_key.is_some() || self.oauth_token.is_some()
    }
}

impl GoogleProvider {
    pub fn get_refresh_token(&self) -> Option<&String> {
        self.oauth_refresh_token.as_ref()
    }
}
