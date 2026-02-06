use crate::config::ProviderConfig;
use crate::error::{NexusError, Result};
use crate::providers::{CompletionRequest, CompletionResponse, Message, Provider, ProviderInfo, Role, Usage};
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

pub struct ClaudeProvider {
    api_key: Option<String>,
    oauth_token: Option<String>,
    oauth_refresh_token: Option<String>,
    oauth_client_id: Option<String>,
    oauth_client_secret: Option<String>,
    base_url: String,
    client: Client,
    default_model: String,
    version: String,
}

impl ClaudeProvider {
    // Anthropic OAuth endpoints (for enterprise/Workspace OAuth)
    const ANTHROPIC_AUTH_URL: &'static str = "https://console.anthropic.com/oauth/authorize";
    const ANTHROPIC_TOKEN_URL: &'static str = "https://api.anthropic.com/oauth/token";
    const REDIRECT_URL: &'static str = "http://localhost:8081/callback";
    const SCOPES: &'static [&'static str] = &[
        "message_batches:write",
        "api_keys:read",
    ];

    pub fn new(config: &ProviderConfig) -> Self {
        Self {
            api_key: config.api_key.clone(),
            oauth_token: config.oauth_token.clone(),
            oauth_refresh_token: config.oauth_refresh_token.clone(),
            oauth_client_id: config.oauth_client_id.clone(),
            oauth_client_secret: config.oauth_client_secret.clone(),
            base_url: config.base_url.clone().unwrap_or_else(|| {
                "https://api.anthropic.com".to_string()
            }),
            client: Client::new(),
            default_model: config.default_model.clone().unwrap_or_else(|| {
                "claude-3-5-sonnet-20241022".to_string()
            }),
            version: "2023-06-01".to_string(),
        }
    }

    pub fn static_info() -> ProviderInfo {
        ProviderInfo {
            name: "claude".to_string(),
            display_name: "Claude (Anthropic)".to_string(),
            supports_oauth: true, // Now supports OAuth for enterprise
            default_model: "claude-3-5-sonnet-20241022".to_string(),
            available_models: vec![
                "claude-3-5-sonnet-20241022".to_string(),
                "claude-3-opus-20240229".to_string(),
                "claude-3-haiku-20240307".to_string(),
                "claude-3-sonnet-20240229".to_string(),
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
            .set_auth_uri(AuthUrl::new(Self::ANTHROPIC_AUTH_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid auth URL: {}", e)))?)
            .set_token_uri(TokenUrl::new(Self::ANTHROPIC_TOKEN_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid token URL: {}", e)))?)
            .set_redirect_uri(RedirectUrl::new(Self::REDIRECT_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid redirect URL: {}", e)))?);

        let (auth_url, _csrf_token) = client
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new(Self::SCOPES[0].to_string()))
            .add_scope(Scope::new(Self::SCOPES[1].to_string()))
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
            .set_auth_uri(AuthUrl::new(Self::ANTHROPIC_AUTH_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid auth URL: {}", e)))?)
            .set_token_uri(TokenUrl::new(Self::ANTHROPIC_TOKEN_URL.to_string())
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
        
        println!("Opening browser for Anthropic OAuth authorization...");
        println!("If browser doesn't open, visit this URL manually: {}", auth_url);
        
        // Try to open browser
        if let Err(_) = open::that(&auth_url) {
            println!("Could not open browser automatically.");
        }

        // Start local server to receive callback
        let listener = TcpListener::bind("127.0.0.1:8081")
            .map_err(|e| NexusError::OAuth(format!("Failed to bind callback server: {}", e)))?;

        println!("Waiting for OAuth callback on http://localhost:8081/callback...");

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
            .set_auth_uri(AuthUrl::new(Self::ANTHROPIC_AUTH_URL.to_string())
                .map_err(|e| NexusError::OAuth(format!("Invalid auth URL: {}", e)))?)
            .set_token_uri(TokenUrl::new(Self::ANTHROPIC_TOKEN_URL.to_string())
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
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    _ => "user",
                },
                "content": msg.content
            })
        }).collect()
    }

    fn extract_system_message(&self, messages: &[Message]) -> Option<String> {
        messages.iter()
            .find(|m| m.role == Role::System)
            .map(|m| m.content.clone())
    }

    fn get_auth_headers(&self) -> Result<Vec<(String, String)>> {
        let mut headers = vec![];
        
        if let Some(token) = &self.oauth_token {
            headers.push(("Authorization".to_string(), format!("Bearer {}", token)));
        } else if let Some(key) = &self.api_key {
            headers.push(("x-api-key".to_string(), key.clone()));
        } else {
            return Err(NexusError::Authentication(
                "Claude API key or OAuth token not configured".to_string()
            ));
        }

        headers.push(("anthropic-version".to_string(), self.version.clone()));
        
        Ok(headers)
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn info(&self) -> ProviderInfo {
        Self::static_info()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let system_message = self.extract_system_message(&request.messages);
        let messages = self.convert_messages(
            &request.messages.iter()
                .filter(|m| m.role != Role::System)
                .cloned()
                .collect::<Vec<_>>()
        );

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "temperature": request.temperature.unwrap_or(0.7),
        });

        if let Some(system) = system_message {
            body["system"] = serde_json::json!(system);
        }

        if request.stream == Some(true) {
            body["stream"] = serde_json::json!(true);
        }

        let headers = self.get_auth_headers()?;
        
        let mut req = self.client
            .post(format!("{}/v1/messages", self.base_url))
            .header("Content-Type", "application/json");

        for (key, value) in headers {
            req = req.header(&key, &value);
        }

        let response = req
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(NexusError::ApiRequest(format!(
                "Claude API error: {}",
                error_text
            )));
        }

        let data: serde_json::Value = response.json().await?;
        
        let content = data["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let finish_reason = data["stop_reason"]
            .as_str()
            .map(|s| s.to_string());

        let usage = if let Some(usage) = data.get("usage") {
            Some(Usage {
                prompt_tokens: usage["input_tokens"].as_u64().unwrap_or(0) as u32,
                completion_tokens: usage["output_tokens"].as_u64().unwrap_or(0) as u32,
                total_tokens: usage["input_tokens"].as_u64().unwrap_or(0) as u32 + 
                           usage["output_tokens"].as_u64().unwrap_or(0) as u32,
            })
        } else {
            None
        };

        Ok(CompletionResponse {
            id: data["id"].as_str().unwrap_or("unknown").to_string(),
            model: data["model"].as_str().unwrap_or(&request.model).to_string(),
            content,
            finish_reason,
            usage,
            tool_calls: None,
        })
    }

    async fn authenticate(&mut self) -> Result<()> {
        // If we have an API key, we're authenticated (simplest method)
        if self.api_key.is_some() {
            return Ok(());
        }

        // If we have an OAuth token, check if it's still valid
        if self.oauth_token.is_some() {
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

impl ClaudeProvider {
    pub fn get_refresh_token(&self) -> Option<&String> {
        self.oauth_refresh_token.as_ref()
    }
}
