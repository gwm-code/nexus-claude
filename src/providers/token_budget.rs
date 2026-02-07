/// Tracks token usage across a multi-turn agent session and enforces budget limits.
///
/// The estimator uses a simple `len / 4` heuristic (roughly 4 bytes per token for
/// English text), which is intentionally cheap so it can be called on every turn
/// without adding latency.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// Hard cap on cumulative input (prompt) tokens.
    pub max_input_tokens: u32,
    /// Hard cap on cumulative total (input + output) tokens.
    pub max_total_tokens: u32,
    /// Running total of input tokens consumed so far.
    pub used_input_tokens: u32,
    /// Running total of output tokens consumed so far.
    pub used_output_tokens: u32,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self::new(100_000, 200_000)
    }
}

impl TokenBudget {
    /// Create a new budget with explicit caps.
    pub fn new(max_input: u32, max_total: u32) -> Self {
        Self {
            max_input_tokens: max_input,
            max_total_tokens: max_total,
            used_input_tokens: 0,
            used_output_tokens: 0,
        }
    }

    /// Cheap heuristic: ~4 bytes per token for English text.
    pub fn estimate_tokens(text: &str) -> u32 {
        (text.len() as u32) / 4
    }

    /// How many total tokens remain before the budget is exhausted.
    pub fn remaining(&self) -> u32 {
        self.max_total_tokens
            .saturating_sub(self.used_input_tokens + self.used_output_tokens)
    }

    /// Returns `true` if there is room for at least one more completion turn.
    /// A small headroom (256 tokens) avoids issuing a request that will
    /// immediately be clipped.
    pub fn can_continue(&self) -> bool {
        let total_used = self.used_input_tokens + self.used_output_tokens;
        total_used + 256 < self.max_total_tokens
            && self.used_input_tokens < self.max_input_tokens
    }

    /// Record actual usage returned from a completion response.
    pub fn record_usage(&mut self, input: u32, output: u32) {
        self.used_input_tokens += input;
        self.used_output_tokens += output;
    }

    /// Suggest a `max_tokens` value for the next request that respects
    /// the remaining budget while defaulting to 4096.
    pub fn dynamic_max_tokens(&self) -> u32 {
        std::cmp::min(4096, self.remaining())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_budget() {
        let b = TokenBudget::default();
        assert_eq!(b.max_input_tokens, 100_000);
        assert_eq!(b.max_total_tokens, 200_000);
        assert_eq!(b.remaining(), 200_000);
        assert!(b.can_continue());
    }

    #[test]
    fn test_estimate_tokens() {
        // 12 chars -> 3 tokens
        assert_eq!(TokenBudget::estimate_tokens("hello world!"), 3);
        assert_eq!(TokenBudget::estimate_tokens(""), 0);
    }

    #[test]
    fn test_record_usage_and_remaining() {
        let mut b = TokenBudget::new(1000, 2000);
        b.record_usage(400, 300);
        assert_eq!(b.used_input_tokens, 400);
        assert_eq!(b.used_output_tokens, 300);
        assert_eq!(b.remaining(), 1300);
    }

    #[test]
    fn test_can_continue_false_when_exhausted() {
        let mut b = TokenBudget::new(1000, 2000);
        b.record_usage(1000, 900);
        // remaining = 100, which is < 256 headroom
        assert!(!b.can_continue());
    }

    #[test]
    fn test_can_continue_false_when_input_exhausted() {
        let mut b = TokenBudget::new(500, 2000);
        b.record_usage(500, 0);
        assert!(!b.can_continue());
    }

    #[test]
    fn test_dynamic_max_tokens() {
        let b = TokenBudget::new(100_000, 200_000);
        assert_eq!(b.dynamic_max_tokens(), 4096);

        let mut b2 = TokenBudget::new(1000, 2000);
        b2.record_usage(900, 900);
        // remaining = 200
        assert_eq!(b2.dynamic_max_tokens(), 200);
    }
}
