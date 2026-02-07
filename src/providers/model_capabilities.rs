use serde::{Deserialize, Serialize};
use once_cell::sync::Lazy;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub speed_score: u8,           // 1-10 (10 = fastest)
    pub reasoning_score: u8,       // 1-10 (10 = best reasoning)
    pub coding_score: u8,          // 1-10 (10 = best at code)
    pub cost_per_1m_tokens: f64,   // Estimated cost in USD
    pub context_window: u32,       // Max tokens
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub release_date: String,      // "2024-12-01"
}

impl ModelCapabilities {
    pub fn get_all() -> &'static [ModelCapabilities] {
        &MODEL_RANKINGS
    }

    pub fn get_by_id(id: &str) -> Option<&'static ModelCapabilities> {
        MODEL_RANKINGS.iter().find(|m| m.id == id)
    }

    pub fn filter_by_provider(provider: &str) -> Vec<&'static ModelCapabilities> {
        MODEL_RANKINGS.iter().filter(|m| m.provider == provider).collect()
    }

    /// Ranking algorithm for heartbeat tasks (prioritize speed + low cost)
    pub fn rank_for_heartbeat(models: &[String]) -> Vec<String> {
        rank_by_score(models, |cap| {
            cap.speed_score as f64 - (cap.cost_per_1m_tokens * 10.0)
        })
    }

    /// Ranking algorithm for planning tasks (prioritize reasoning - medium cost tolerance)
    pub fn rank_for_planning(models: &[String]) -> Vec<String> {
        rank_by_score(models, |cap| {
            cap.reasoning_score as f64 - (cap.cost_per_1m_tokens * 2.0)
        })
    }

    /// Ranking algorithm for coding tasks (prioritize coding + reasoning)
    pub fn rank_for_coding(models: &[String]) -> Vec<String> {
        rank_by_score(models, |cap| {
            (cap.coding_score + cap.reasoning_score) as f64
        })
    }

    /// Ranking algorithm for review tasks (balanced approach)
    pub fn rank_for_review(models: &[String]) -> Vec<String> {
        rank_by_score(models, |cap| {
            ((cap.coding_score + cap.reasoning_score) as f64 / 2.0) - (cap.cost_per_1m_tokens * 1.0)
        })
    }
}

fn rank_by_score<F>(models: &[String], score_fn: F) -> Vec<String>
where
    F: Fn(&ModelCapabilities) -> f64,
{
    let mut ranked: Vec<_> = models
        .iter()
        .filter_map(|id| {
            ModelCapabilities::get_by_id(id).map(|cap| (id.clone(), score_fn(cap)))
        })
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().map(|(id, _)| id).collect()
}

// Lazy static model rankings database (based on research from Feb 2026)
static MODEL_RANKINGS: Lazy<Vec<ModelCapabilities>> = Lazy::new(|| {
    vec![
        // Claude Models
        ModelCapabilities {
            id: "claude-opus-4-6".to_string(),
            provider: "claude".to_string(),
            display_name: "Claude Opus 4.6".to_string(),
            speed_score: 4,
            reasoning_score: 10,
            coding_score: 10,
            cost_per_1m_tokens: 15.0,
            context_window: 200_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-12-01".to_string(),
        },
        ModelCapabilities {
            id: "claude-sonnet-4-5".to_string(),
            provider: "claude".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            speed_score: 7,
            reasoning_score: 9,
            coding_score: 9,
            cost_per_1m_tokens: 3.0,
            context_window: 200_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-10-22".to_string(),
        },
        ModelCapabilities {
            id: "claude-haiku-3-5".to_string(),
            provider: "claude".to_string(),
            display_name: "Claude Haiku 3.5".to_string(),
            speed_score: 10,
            reasoning_score: 7,
            coding_score: 7,
            cost_per_1m_tokens: 0.25,
            context_window: 200_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-08-01".to_string(),
        },
        // Google Gemini Models
        ModelCapabilities {
            id: "gemini-2.0-flash-exp".to_string(),
            provider: "google".to_string(),
            display_name: "Gemini 2.0 Flash (Experimental)".to_string(),
            speed_score: 10,
            reasoning_score: 8,
            coding_score: 8,
            cost_per_1m_tokens: 0.0,
            context_window: 1_000_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-12-11".to_string(),
        },
        ModelCapabilities {
            id: "gemini-1.5-pro".to_string(),
            provider: "google".to_string(),
            display_name: "Gemini 1.5 Pro".to_string(),
            speed_score: 8,
            reasoning_score: 8,
            coding_score: 7,
            cost_per_1m_tokens: 1.25,
            context_window: 2_000_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-05-14".to_string(),
        },
        ModelCapabilities {
            id: "gemini-1.5-flash".to_string(),
            provider: "google".to_string(),
            display_name: "Gemini 1.5 Flash".to_string(),
            speed_score: 10,
            reasoning_score: 6,
            coding_score: 6,
            cost_per_1m_tokens: 0.075,
            context_window: 1_000_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-05-14".to_string(),
        },
        // OpenAI Models
        ModelCapabilities {
            id: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            display_name: "GPT-4o".to_string(),
            speed_score: 8,
            reasoning_score: 9,
            coding_score: 8,
            cost_per_1m_tokens: 2.5,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-05-13".to_string(),
        },
        ModelCapabilities {
            id: "gpt-4o-mini".to_string(),
            provider: "openai".to_string(),
            display_name: "GPT-4o Mini".to_string(),
            speed_score: 10,
            reasoning_score: 7,
            coding_score: 7,
            cost_per_1m_tokens: 0.15,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-07-18".to_string(),
        },
        ModelCapabilities {
            id: "o1".to_string(),
            provider: "openai".to_string(),
            display_name: "OpenAI o1".to_string(),
            speed_score: 2,
            reasoning_score: 10,
            coding_score: 9,
            cost_per_1m_tokens: 15.0,
            context_window: 200_000,
            supports_streaming: false,
            supports_tools: false,
            release_date: "2024-12-17".to_string(),
        },
        ModelCapabilities {
            id: "o1-mini".to_string(),
            provider: "openai".to_string(),
            display_name: "OpenAI o1-mini".to_string(),
            speed_score: 5,
            reasoning_score: 9,
            coding_score: 8,
            cost_per_1m_tokens: 3.0,
            context_window: 128_000,
            supports_streaming: false,
            supports_tools: false,
            release_date: "2024-09-12".to_string(),
        },
        // Other Providers
        ModelCapabilities {
            id: "grok-beta".to_string(),
            provider: "xai".to_string(),
            display_name: "Grok Beta".to_string(),
            speed_score: 7,
            reasoning_score: 8,
            coding_score: 7,
            cost_per_1m_tokens: 5.0,
            context_window: 131_072,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-11-04".to_string(),
        },
        ModelCapabilities {
            id: "openrouter/auto:free".to_string(),
            provider: "openrouter".to_string(),
            display_name: "OpenRouter Auto (Free)".to_string(),
            speed_score: 8,
            reasoning_score: 6,
            coding_score: 6,
            cost_per_1m_tokens: 0.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: false,
            release_date: "2024-01-01".to_string(),
        },
        ModelCapabilities {
            id: "openrouter/auto".to_string(),
            provider: "openrouter".to_string(),
            display_name: "OpenRouter Auto".to_string(),
            speed_score: 7,
            reasoning_score: 8,
            coding_score: 8,
            cost_per_1m_tokens: 2.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-01-01".to_string(),
        },
        ModelCapabilities {
            id: "deepseek-chat".to_string(),
            provider: "deepseek".to_string(),
            display_name: "DeepSeek Chat".to_string(),
            speed_score: 9,
            reasoning_score: 8,
            coding_score: 9,
            cost_per_1m_tokens: 0.14,
            context_window: 64_000,
            supports_streaming: true,
            supports_tools: true,
            release_date: "2024-01-01".to_string(),
        },
    ]
});
