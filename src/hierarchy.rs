use serde::{Deserialize, Serialize};
use crate::error::{NexusError, Result};
use crate::providers::{Message, CompletionRequest, CompletionResponse};
use crate::config::ConfigManager;
use crate::providers::create_provider;
use std::path::PathBuf;
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTier {
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost_per_request: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHierarchy {
    pub heartbeat: Vec<ModelTier>,
    pub daily: Vec<ModelTier>,
    pub planning: Vec<ModelTier>,
    pub coding: Vec<ModelTier>,
    pub review: Vec<ModelTier>,
}

impl Default for ModelHierarchy {
    fn default() -> Self {
        Self::balanced_preset()
    }
}

impl ModelHierarchy {
    /// Balanced preset - good mix of speed, cost, and quality
    pub fn balanced_preset() -> Self {
        Self {
            heartbeat: vec![
                ModelTier {
                    model_id: "openrouter/auto:free".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            daily: vec![
                ModelTier {
                    model_id: "gemini-1.5-flash".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            planning: vec![
                ModelTier {
                    model_id: "gemini-1.5-pro".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
                ModelTier {
                    model_id: "claude-sonnet-4-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            coding: vec![
                ModelTier {
                    model_id: "claude-sonnet-4-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
                ModelTier {
                    model_id: "claude-opus-4-6".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            review: vec![
                ModelTier {
                    model_id: "claude-sonnet-4-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
        }
    }

    /// Budget preset - minimize costs
    pub fn budget_preset() -> Self {
        Self {
            heartbeat: vec![
                ModelTier {
                    model_id: "openrouter/auto:free".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            daily: vec![
                ModelTier {
                    model_id: "gemini-1.5-flash".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            planning: vec![
                ModelTier {
                    model_id: "gemini-1.5-flash".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
                ModelTier {
                    model_id: "gpt-4o-mini".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            coding: vec![
                ModelTier {
                    model_id: "gemini-1.5-pro".to_string(),
                    max_tokens: None,
                    max_cost_per_request: Some(0.5),
                },
            ],
            review: vec![
                ModelTier {
                    model_id: "gemini-1.5-flash".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
        }
    }

    /// Premium preset - best models, ignore cost
    pub fn premium_preset() -> Self {
        Self {
            heartbeat: vec![
                ModelTier {
                    model_id: "gemini-1.5-flash".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            daily: vec![
                ModelTier {
                    model_id: "gemini-1.5-pro".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            planning: vec![
                ModelTier {
                    model_id: "claude-opus-4-6".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
                ModelTier {
                    model_id: "o1".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            coding: vec![
                ModelTier {
                    model_id: "claude-opus-4-6".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            review: vec![
                ModelTier {
                    model_id: "claude-sonnet-4-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
        }
    }

    /// Speed preset - prioritize fast responses
    pub fn speed_preset() -> Self {
        Self {
            heartbeat: vec![
                ModelTier {
                    model_id: "openrouter/auto:free".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            daily: vec![
                ModelTier {
                    model_id: "gemini-1.5-flash".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            planning: vec![
                ModelTier {
                    model_id: "claude-haiku-3-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            coding: vec![
                ModelTier {
                    model_id: "claude-sonnet-4-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            review: vec![
                ModelTier {
                    model_id: "gemini-1.5-flash".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
        }
    }

    /// Claude-only preset - only use Claude models
    pub fn claude_only_preset() -> Self {
        Self {
            heartbeat: vec![
                ModelTier {
                    model_id: "claude-haiku-3-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            daily: vec![
                ModelTier {
                    model_id: "claude-haiku-3-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            planning: vec![
                ModelTier {
                    model_id: "claude-sonnet-4-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
                ModelTier {
                    model_id: "claude-opus-4-6".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            coding: vec![
                ModelTier {
                    model_id: "claude-sonnet-4-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
                ModelTier {
                    model_id: "claude-opus-4-6".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
            review: vec![
                ModelTier {
                    model_id: "claude-sonnet-4-5".to_string(),
                    max_tokens: None,
                    max_cost_per_request: None,
                },
            ],
        }
    }

    pub fn from_preset(name: &str) -> Option<Self> {
        match name {
            "balanced" => Some(Self::balanced_preset()),
            "budget" => Some(Self::budget_preset()),
            "premium" => Some(Self::premium_preset()),
            "speed" => Some(Self::speed_preset()),
            "claude-only" => Some(Self::claude_only_preset()),
            _ => None,
        }
    }

    pub fn get_tier(&self, category: TaskCategory, tier_index: usize) -> Option<&ModelTier> {
        let tiers = match category {
            TaskCategory::Heartbeat => &self.heartbeat,
            TaskCategory::Daily => &self.daily,
            TaskCategory::Planning => &self.planning,
            TaskCategory::Coding => &self.coding,
            TaskCategory::Review => &self.review,
        };
        tiers.get(tier_index)
    }

    pub fn save(&self, config_dir: &PathBuf) -> Result<()> {
        let hierarchy_path = config_dir.join("hierarchy.json");
        let json = serde_json::to_string_pretty(self)?;
        fs::write(hierarchy_path, json)?;
        Ok(())
    }

    pub fn load(config_dir: &PathBuf) -> Result<Self> {
        let hierarchy_path = config_dir.join("hierarchy.json");
        if !hierarchy_path.exists() {
            return Ok(Self::default());
        }
        let json = fs::read_to_string(hierarchy_path)?;
        let hierarchy = serde_json::from_str(&json)?;
        Ok(hierarchy)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskCategory {
    Heartbeat,  // Proactive checks, simple automation
    Daily,      // Simple queries, file reads, status
    Planning,   // Architecture, design, reasoning
    Coding,     // Code generation, refactoring
    Review,     // Code review, testing, validation
}

impl TaskCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskCategory::Heartbeat => "heartbeat",
            TaskCategory::Daily => "daily",
            TaskCategory::Planning => "planning",
            TaskCategory::Coding => "coding",
            TaskCategory::Review => "review",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "heartbeat" => Some(TaskCategory::Heartbeat),
            "daily" => Some(TaskCategory::Daily),
            "planning" | "plan" => Some(TaskCategory::Planning),
            "coding" | "code" => Some(TaskCategory::Coding),
            "review" => Some(TaskCategory::Review),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationPolicy {
    pub enabled: bool,
    pub max_escalations: usize,
    pub escalate_on_error: bool,
    pub escalate_on_refusal: bool,
    pub escalate_on_test_failure: bool,
    pub escalate_on_syntax_error: bool,
    pub escalate_on_low_confidence: bool,
    pub confidence_threshold: f32,
    pub daily_budget_limit: f64,
}

impl Default for EscalationPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_escalations: 3,
            escalate_on_error: true,
            escalate_on_refusal: true,
            escalate_on_test_failure: false,
            escalate_on_syntax_error: true,
            escalate_on_low_confidence: false,
            confidence_threshold: 0.8,
            daily_budget_limit: 20.0,
        }
    }
}

impl EscalationPolicy {
    pub fn save(&self, config_dir: &PathBuf) -> Result<()> {
        let policy_path = config_dir.join("escalation_policy.json");
        let json = serde_json::to_string_pretty(self)?;
        fs::write(policy_path, json)?;
        Ok(())
    }

    pub fn load(config_dir: &PathBuf) -> Result<Self> {
        let policy_path = config_dir.join("escalation_policy.json");
        if !policy_path.exists() {
            return Ok(Self::default());
        }
        let json = fs::read_to_string(policy_path)?;
        let policy = serde_json::from_str(&json)?;
        Ok(policy)
    }
}

/// Classify a task based on input and context
pub fn classify_task(input: &str, is_scheduled: bool) -> TaskCategory {
    // 1. Explicit override in input
    if input.starts_with("[heartbeat]") {
        return TaskCategory::Heartbeat;
    }
    if input.starts_with("[plan]") || input.starts_with("/plan") {
        return TaskCategory::Planning;
    }
    if input.starts_with("[code]") || input.starts_with("/code") {
        return TaskCategory::Coding;
    }
    if input.starts_with("[review]") {
        return TaskCategory::Review;
    }

    // 2. Command-based classification
    if is_scheduled {
        return TaskCategory::Heartbeat;
    }

    // 3. Keyword-based heuristics
    let lower = input.to_lowercase();

    if lower.contains("plan") || lower.contains("design") || lower.contains("architect") {
        return TaskCategory::Planning;
    }

    if lower.contains("write") || lower.contains("implement") || lower.contains("refactor")
        || lower.contains("create") || lower.contains("add function") {
        return TaskCategory::Coding;
    }

    if lower.contains("review") || lower.contains("test") || lower.contains("validate")
        || lower.contains("check code") {
        return TaskCategory::Review;
    }

    // 4. Default to Daily for simple queries
    TaskCategory::Daily
}
