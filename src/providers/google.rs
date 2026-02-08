// Google Code Assist Provider
// Uses cloudcode-pa.googleapis.com/v1internal API (same as gemini-cli)
// Supports OAuth with Google One AI Pro subscription

use crate::config::ProviderConfig;
use crate::error::{NexusError, Result};
use crate::providers::{
    CompletionRequest, CompletionResponse, Message, ModelInfo, Provider, ProviderInfo,
    StreamChunk, Usage,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

const CODE_ASSIST_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const CODE_ASSIST_API_VERSION: &str = "v1internal";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Gemini credentials file format (compatible with ~/.gemini/oauth_creds.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiCredentials {
    access_token: String,
    refresh_token: Option<String>,
    scope: Option<String>,
    token_type: Option<String>,
    expiry_date: Option<u64>, // milliseconds since epoch
}

pub struct GoogleProvider {
    oauth_token: Mutex<Option<String>>,
    oauth_refresh_token: Option<String>,
    project_id: Mutex<Option<String>>,
    client: Client,
    default_model: String,
    gemini_creds_path: PathBuf,
}

impl GoogleProvider {
    pub fn new(config: &ProviderConfig) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let gemini_creds_path = PathBuf::from(&home).join(".gemini/oauth_creds.json");

        // Try loading tokens from config first, then fall back to gemini-cli creds file
        let (token, refresh) = Self::load_tokens(config, &gemini_creds_path);

        Self {
            oauth_token: Mutex::new(token),
            oauth_refresh_token: refresh,
            project_id: Mutex::new(None),
            client: Client::new(),
            default_model: config
                .default_model
                .clone()
                .unwrap_or_else(|| "gemini-2.5-flash".to_string()),
            gemini_creds_path,
        }
    }

    fn load_tokens(config: &ProviderConfig, creds_path: &PathBuf) -> (Option<String>, Option<String>) {
        // 1. Try from nexus config
        if config.oauth_token.is_some() {
            return (config.oauth_token.clone(), config.oauth_refresh_token.clone());
        }

        // 2. Try from ~/.gemini/oauth_creds.json (gemini-cli format)
        if let Ok(contents) = std::fs::read_to_string(creds_path) {
            if let Ok(creds) = serde_json::from_str::<GeminiCredentials>(&contents) {
                // Check if token is expired
                let expired = creds.expiry_date.map_or(false, |exp| {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    now_ms > exp
                });

                if !expired {
                    return (Some(creds.access_token), creds.refresh_token);
                } else {
                    // Token expired but we have refresh token
                    return (None, creds.refresh_token);
                }
            }
        }

        (None, None)
    }

    fn base_url(&self) -> String {
        format!("{}/{}", CODE_ASSIST_ENDPOINT, CODE_ASSIST_API_VERSION)
    }

    fn get_token(&self) -> Result<String> {
        let guard = self.oauth_token.lock().map_err(|e| {
            NexusError::Authentication(format!("Failed to lock token: {}", e))
        })?;
        guard.clone().ok_or_else(|| {
            NexusError::Authentication(
                "No OAuth token. Run 'nexus oauth authorize google' or authenticate with gemini-cli first.".to_string(),
            )
        })
    }

    fn set_token(&self, token: String) {
        if let Ok(mut guard) = self.oauth_token.lock() {
            *guard = Some(token);
        }
    }

    fn get_project_id(&self) -> Option<String> {
        self.project_id.lock().ok().and_then(|g| g.clone())
    }

    fn set_project_id(&self, id: String) {
        if let Ok(mut guard) = self.project_id.lock() {
            *guard = Some(id);
        }
    }

    /// Refresh the OAuth token using the refresh_token
    async fn refresh_token_internal(&self) -> Result<String> {
        let refresh_token = self.oauth_refresh_token.as_ref().ok_or_else(|| {
            NexusError::OAuth("No refresh token available".to_string())
        })?;

        let (client_id, client_secret) = Self::get_oauth_credentials()?;

        let form_body = format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
            urlencoding::encode(refresh_token),
            urlencoding::encode(&client_id),
            urlencoding::encode(&client_secret),
        );

        let resp = self
            .client
            .post(GOOGLE_TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            return Err(NexusError::OAuth(format!("Token refresh failed: {}", err)));
        }

        let data: serde_json::Value = resp.json().await?;
        let new_token = data["access_token"]
            .as_str()
            .ok_or_else(|| NexusError::OAuth("No access_token in refresh response".to_string()))?
            .to_string();

        // Update in-memory token
        self.set_token(new_token.clone());

        // Update gemini creds file if it exists
        if self.gemini_creds_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&self.gemini_creds_path) {
                if let Ok(mut creds) = serde_json::from_str::<serde_json::Value>(&contents) {
                    creds["access_token"] = serde_json::Value::String(new_token.clone());
                    if let Some(expires_in) = data["expires_in"].as_u64() {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        creds["expiry_date"] =
                            serde_json::Value::Number((now_ms + expires_in * 1000).into());
                    }
                    let _ = std::fs::write(
                        &self.gemini_creds_path,
                        serde_json::to_string_pretty(&creds).unwrap_or_default(),
                    );
                }
            }
        }

        Ok(new_token)
    }

    /// Get a valid token, refreshing if needed
    async fn ensure_token(&self) -> Result<String> {
        match self.get_token() {
            Ok(token) => Ok(token),
            Err(_) => {
                // Token missing or expired, try refresh
                self.refresh_token_internal().await
            }
        }
    }

    /// Call loadCodeAssist to get the project ID and verify subscription
    async fn setup_code_assist(&self) -> Result<String> {
        // Return cached project ID if available
        if let Some(id) = self.get_project_id() {
            return Ok(id);
        }

        let token = self.ensure_token().await?;
        let url = format!("{}:loadCodeAssist", self.base_url());

        let body = serde_json::json!({
            "metadata": {
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            // If 401, try refreshing token and retry once
            if status.as_u16() == 401 {
                let new_token = self.refresh_token_internal().await?;
                let resp2 = self
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", new_token))
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await?;
                if !resp2.status().is_success() {
                    let err2 = resp2.text().await.unwrap_or_default();
                    return Err(NexusError::ApiRequest(format!(
                        "Code Assist setup failed: {}",
                        err2
                    )));
                }
                let data: serde_json::Value = resp2.json().await?;
                return self.extract_project_id(&data);
            }
            return Err(NexusError::ApiRequest(format!(
                "Code Assist setup failed ({}): {}",
                status, err
            )));
        }

        let data: serde_json::Value = resp.json().await?;
        self.extract_project_id(&data)
    }

    fn extract_project_id(&self, data: &serde_json::Value) -> Result<String> {
        // Try cloudaicompanionProject first (for already-onboarded users)
        if let Some(project) = data["cloudaicompanionProject"].as_str() {
            self.set_project_id(project.to_string());
            return Ok(project.to_string());
        }

        // If not onboarded yet, check if there are allowed tiers
        if data["currentTier"].is_null() && data.get("allowedTiers").is_some() {
            return Err(NexusError::Authentication(
                "Google Code Assist account not yet onboarded. Please run gemini-cli once to complete setup.".to_string(),
            ));
        }

        Err(NexusError::ApiRequest(
            "Could not determine Code Assist project ID from setup response".to_string(),
        ))
    }

    fn convert_messages(messages: &[Message]) -> Vec<serde_json::Value> {
        // Separate system messages from conversation
        let mut contents = Vec::new();

        for msg in messages {
            match msg.role {
                crate::providers::Role::System => {
                    // System messages become the first user message or are prepended
                    // Code Assist doesn't have a separate system role in contents
                    // We'll handle it via systemInstruction in the request
                    continue;
                }
                crate::providers::Role::User => {
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [{"text": &msg.content}]
                    }));
                }
                crate::providers::Role::Assistant => {
                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": [{"text": &msg.content}]
                    }));
                }
                crate::providers::Role::Tool => {
                    // Tool responses need to be formatted as functionResponse
                    let function_name = msg.name.as_deref().unwrap_or("unknown");
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": function_name,
                                "response": {
                                    "result": &msg.content
                                }
                            }
                        }]
                    }));
                }
                _ => {
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [{"text": &msg.content}]
                    }));
                }
            }
        }

        contents
    }

    fn extract_system_instruction(messages: &[Message]) -> Option<serde_json::Value> {
        let system_msgs: Vec<&Message> = messages
            .iter()
            .filter(|m| m.role == crate::providers::Role::System)
            .collect();

        if system_msgs.is_empty() {
            return None;
        }

        let combined: String = system_msgs
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        // Code Assist systemInstruction should NOT have a role field
        Some(serde_json::json!({
            "parts": [{"text": combined}]
        }))
    }

    /// Build the Code Assist request envelope
    fn build_request(
        model: &str,
        project: &str,
        messages: &[Message],
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        tools: Option<&[crate::executor::tools::Tool]>,
    ) -> serde_json::Value {
        let mut contents = Self::convert_messages(messages);
        let system_instruction = Self::extract_system_instruction(messages);

        // Ensure we have at least one content message (API requires non-empty contents)
        if contents.is_empty() {
            contents.push(serde_json::json!({
                "role": "user",
                "parts": [{"text": ""}]
            }));
        }

        let user_prompt_id = uuid::Uuid::new_v4().to_string();

        let mut gen_config = serde_json::json!({});
        if let Some(temp) = temperature {
            gen_config["temperature"] = serde_json::json!(temp);
        }
        if let Some(max) = max_tokens {
            gen_config["maxOutputTokens"] = serde_json::json!(max);
        }

        let mut request = serde_json::json!({
            "contents": contents,
            "generationConfig": gen_config,
        });

        if let Some(sys) = system_instruction {
            request["systemInstruction"] = sys;
        }

        // Add tools if provided (Gemini native function calling)
        if let Some(tools_list) = tools {
            let function_declarations: Vec<serde_json::Value> = tools_list.iter().map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            }).collect();

            request["tools"] = serde_json::json!([{
                "function_declarations": function_declarations
            }]);
        }

        serde_json::json!({
            "model": model,
            "project": project,
            "user_prompt_id": user_prompt_id,
            "request": request,
        })
    }

    /// Build URL with optional query parameters
    fn build_url(&self, method: &str, query_params: Option<&[(&str, &str)]>) -> String {
        let mut url = format!("{}:{}", self.base_url(), method);
        if let Some(params) = query_params {
            if !params.is_empty() {
                url.push('?');
                let qs: Vec<String> = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                    .collect();
                url.push_str(&qs.join("&"));
            }
        }
        url
    }

    /// Make a request with auto-retry on 401 (expired token)
    async fn request_with_retry(
        &self,
        method: &str,
        body: &serde_json::Value,
        query_params: Option<&[(&str, &str)]>,
    ) -> Result<reqwest::Response> {
        let token = self.ensure_token().await?;
        let url = self.build_url(method, query_params);

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;

        if resp.status().as_u16() == 401 {
            // Token expired, refresh and retry
            let new_token = self.refresh_token_internal().await?;
            return Ok(self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", new_token))
                .header("Content-Type", "application/json")
                .json(body)
                .send()
                .await?);
        }

        Ok(resp)
    }

    pub fn static_info() -> ProviderInfo {
        ProviderInfo {
            name: "google".to_string(),
            display_name: "Google Gemini (Code Assist)".to_string(),
            supports_oauth: true,
            default_model: "gemini-2.5-flash".to_string(),
            available_models: vec![
                "gemini-2.5-pro".to_string(),
                "gemini-2.5-flash".to_string(),
                "gemini-2.5-flash-lite".to_string(),
            ],
        }
    }

    pub fn get_refresh_token(&self) -> Option<&String> {
        self.oauth_refresh_token.as_ref()
    }

    /// Get Google OAuth credentials (extracted from gemini-cli or fallback to known public values)
    fn get_oauth_credentials() -> Result<(String, String)> {
        // Try to find gemini-cli installation
        let gemini_path = std::process::Command::new("which")
            .arg("gemini")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string());

        if let Some(path) = gemini_path {
            if !path.is_empty() {
                // Follow symlink to actual installation
                if let Ok(real_path) = std::fs::read_link(&path) {
                    // Try multiple possible locations for oauth2.js
                    let possible_paths = vec![
                        // Nested in gemini-cli's node_modules (most common)
                        real_path.parent()
                            .and_then(|p| p.parent())
                            .map(|p| p.join("node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js")),
                        // Direct sibling package
                        real_path.parent()
                            .and_then(|p| p.parent())
                            .and_then(|p| p.parent())
                            .map(|p| p.join("@google/gemini-cli-core/dist/src/code_assist/oauth2.js")),
                    ];

                    for oauth_path in possible_paths.into_iter().flatten() {
                        if let Ok(content) = std::fs::read_to_string(&oauth_path) {
                                // Extract credentials using simple string matching
                                if let Some(id_start) = content.find("apps.googleusercontent.com") {
                                    if let Some(id_line_start) = content[..id_start].rfind('\n') {
                                        let id_line = &content[id_line_start..id_start + 30];
                                        if let Some(id_match) = id_line.split('\'').nth(1).or_else(|| id_line.split('"').nth(1)) {
                                            if let Some(secret_start) = content.find("GOCSPX-") {
                                                if let Some(secret_line_start) = content[..secret_start].rfind('\n') {
                                                    let secret_line = &content[secret_line_start..secret_start + 50];
                                                    if let Some(secret_match) = secret_line.split('\'').nth(1).or_else(|| secret_line.split('"').nth(1)) {
                                                        return Ok((id_match.to_string(), secret_match.to_string()));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                }
            }
        }

        // Require gemini-cli to be installed for Google OAuth
        Err(NexusError::OAuth(
            "Could not extract OAuth credentials from gemini-cli installation.\n\
             Please ensure gemini-cli is installed: npm install -g @google/gemini-cli".to_string()
        ))
    }

    /// Check if preview features are enabled via the experiments API
    async fn check_preview_enabled(&self) -> Result<bool> {
        const ENABLE_PREVIEW_FLAG_ID: u32 = 45740196;

        let project = match self.get_project_id() {
            Some(id) => id,
            None => return Ok(false),
        };

        let token = match self.ensure_token().await {
            Ok(t) => t,
            Err(_) => return Ok(false),
        };

        let body = serde_json::json!({
            "project": project,
            "metadata": {
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });

        let url = format!("{}:listExperiments", self.base_url());
        let resp = match self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Ok(false),
        };

        if !resp.status().is_success() {
            return Ok(false);
        }

        let data: serde_json::Value = match resp.json().await {
            Ok(d) => d,
            Err(_) => return Ok(false),
        };

        // Check flags array for ENABLE_PREVIEW flag
        if let Some(flags) = data["flags"].as_array() {
            for flag in flags {
                if flag["flagId"].as_u64() == Some(ENABLE_PREVIEW_FLAG_ID as u64) {
                    return Ok(flag["boolValue"].as_bool().unwrap_or(false));
                }
            }
        }

        Ok(false)
    }
}

#[async_trait]
impl Provider for GoogleProvider {
    fn info(&self) -> ProviderInfo {
        Self::static_info()
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let project = self.setup_code_assist().await?;
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        let body = Self::build_request(
            &model,
            &project,
            &request.messages,
            request.temperature,
            request.max_tokens,
            request.tools.as_deref(),
        );

        let resp = self.request_with_retry("generateContent", &body, None).await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            return Err(NexusError::ApiRequest(format!(
                "Code Assist API error ({}): {}",
                status, err
            )));
        }

        let data: serde_json::Value = resp.json().await?;

        // Code Assist wraps response in {"response": {...}, "traceId": "..."}
        let response = &data["response"];

        let parts = &response["candidates"][0]["content"]["parts"];

        // Extract text content and tool calls from parts
        let mut content = String::new();
        let mut tool_calls_list = Vec::new();

        if let Some(parts_array) = parts.as_array() {
            for part in parts_array {
                // Text part
                if let Some(text) = part["text"].as_str() {
                    content.push_str(text);
                }

                // Function call part
                if let Some(func_call) = part.get("functionCall") {
                    if let Some(name) = func_call["name"].as_str() {
                        let args = func_call["args"].clone();
                        tool_calls_list.push(crate::executor::tools::ToolCall {
                            id: format!("call_{}", uuid::Uuid::new_v4()),
                            name: name.to_string(),
                            arguments: args,
                        });
                    }
                }
            }
        }

        let finish_reason = response["candidates"][0]["finishReason"]
            .as_str()
            .map(|s| s.to_string());

        let usage = if let Some(u) = response.get("usageMetadata") {
            Some(Usage {
                prompt_tokens: u["promptTokenCount"].as_u64().unwrap_or(0) as u32,
                completion_tokens: u["candidatesTokenCount"].as_u64().unwrap_or(0) as u32,
                total_tokens: u["totalTokenCount"].as_u64().unwrap_or(0) as u32,
            })
        } else {
            None
        };

        let id = data["traceId"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok(CompletionResponse {
            id,
            model,
            content,
            finish_reason,
            usage,
            tool_calls: if tool_calls_list.is_empty() { None } else { Some(tool_calls_list) },
        })
    }

    async fn complete_stream(
        &self,
        request: CompletionRequest,
        tx: tokio::sync::mpsc::Sender<StreamChunk>,
    ) -> Result<()> {
        let project = self.setup_code_assist().await?;
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            request.model.clone()
        };

        let body = Self::build_request(
            &model,
            &project,
            &request.messages,
            request.temperature,
            request.max_tokens,
            request.tools.as_deref(),
        );

        let resp = self
            .request_with_retry("streamGenerateContent", &body, Some(&[("alt", "sse")]))
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            return Err(NexusError::ApiRequest(format!(
                "Code Assist streaming error ({}): {}",
                status, err
            )));
        }

        // Parse SSE stream
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut data_lines: Vec<String> = Vec::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            let text = String::from_utf8_lossy(&chunk);
            buffer.push_str(&text);

            // Process complete lines
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.starts_with("data: ") {
                    data_lines.push(line[6..].trim().to_string());
                } else if line.is_empty() && !data_lines.is_empty() {
                    // Empty line = end of SSE event, parse accumulated data
                    let json_str = data_lines.join("\n");
                    data_lines.clear();

                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&json_str) {
                        let response = &data["response"];

                        // Extract text delta
                        if let Some(text) =
                            response["candidates"][0]["content"]["parts"][0]["text"].as_str()
                        {
                            if !text.is_empty() {
                                let _ = tx.send(StreamChunk::ContentDelta(text.to_string())).await;
                            }
                        }

                        // Extract usage if present (usually in last chunk)
                        if let Some(u) = response.get("usageMetadata") {
                            if u["totalTokenCount"].as_u64().unwrap_or(0) > 0 {
                                let _ = tx
                                    .send(StreamChunk::Usage(Usage {
                                        prompt_tokens: u["promptTokenCount"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as u32,
                                        completion_tokens: u["candidatesTokenCount"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as u32,
                                        total_tokens: u["totalTokenCount"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as u32,
                                    }))
                                    .await;
                            }
                        }
                    }
                }
            }
        }

        let _ = tx.send(StreamChunk::Done).await;
        Ok(())
    }

    async fn list_available_models(&self) -> Result<Vec<ModelInfo>> {
        // Ensure we have project ID (needed for experiments check)
        let _ = self.setup_code_assist().await;

        // Check if preview features are enabled via experiments API
        let preview_enabled = self.check_preview_enabled().await.unwrap_or(false);

        let mut models = vec![
            ModelInfo {
                id: "gemini-2.5-pro".to_string(),
                name: "Gemini 2.5 Pro".to_string(),
                description: Some("Most capable Gemini 2.5 model".to_string()),
                context_length: Some(1048576),
                pricing: None,
                supports_vision: true,
                supports_streaming: true,
                supports_function_calling: true,
            },
            ModelInfo {
                id: "gemini-2.5-flash".to_string(),
                name: "Gemini 2.5 Flash".to_string(),
                description: Some("Fast and efficient Gemini 2.5 model".to_string()),
                context_length: Some(1048576),
                pricing: None,
                supports_vision: true,
                supports_streaming: true,
                supports_function_calling: true,
            },
            ModelInfo {
                id: "gemini-2.5-flash-lite".to_string(),
                name: "Gemini 2.5 Flash Lite".to_string(),
                description: Some("Lightweight Gemini 2.5 model".to_string()),
                context_length: Some(1048576),
                pricing: None,
                supports_vision: true,
                supports_streaming: true,
                supports_function_calling: true,
            },
        ];

        // Add Gemini 3 preview models if enabled
        if preview_enabled {
            models.extend(vec![
                ModelInfo {
                    id: "gemini-3-pro-preview".to_string(),
                    name: "Gemini 3 Pro (Preview)".to_string(),
                    description: Some("Next-gen Gemini Pro with extended thinking (preview)".to_string()),
                    context_length: Some(1048576),
                    pricing: None,
                    supports_vision: true,
                    supports_streaming: true,
                    supports_function_calling: true,
                },
                ModelInfo {
                    id: "gemini-3-flash-preview".to_string(),
                    name: "Gemini 3 Flash (Preview)".to_string(),
                    description: Some("Next-gen Gemini Flash with extended thinking (preview)".to_string()),
                    context_length: Some(1048576),
                    pricing: None,
                    supports_vision: true,
                    supports_streaming: true,
                    supports_function_calling: true,
                },
            ]);
        }

        Ok(models)
    }

    async fn authenticate(&mut self) -> Result<()> {
        // Try to get/refresh a token
        match self.ensure_token().await {
            Ok(_) => {
                // Also verify Code Assist access
                self.setup_code_assist().await?;
                Ok(())
            }
            Err(e) => Err(NexusError::Authentication(format!(
                "Google OAuth not configured. Either:\n\
                 1. Run 'gemini' CLI to authenticate (tokens saved to ~/.gemini/oauth_creds.json)\n\
                 2. Run 'nexus oauth authorize google'\n\
                 Error: {}",
                e
            ))),
        }
    }

    async fn refresh_auth(&mut self) -> Result<()> {
        self.refresh_token_internal().await?;
        Ok(())
    }

    fn is_authenticated(&self) -> bool {
        self.oauth_token
            .lock()
            .ok()
            .map(|g| g.is_some())
            .unwrap_or(false)
            || self.oauth_refresh_token.is_some()
    }
}
