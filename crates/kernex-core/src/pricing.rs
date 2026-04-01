//! Per-model token pricing for cost estimation.

/// Per-model token pricing in USD per 1,000,000 tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPricing {
    /// USD per 1,000,000 input tokens.
    pub input_per_mtok: f64,
    /// USD per 1,000,000 output tokens.
    pub output_per_mtok: f64,
}

impl ModelPricing {
    /// Blended cost per 1,000,000 tokens (average of input and output rates).
    ///
    /// Used when input and output token counts are not tracked separately.
    pub fn blended_per_mtok(&self) -> f64 {
        (self.input_per_mtok + self.output_per_mtok) / 2.0
    }

    /// Estimate cost in USD for a given number of tokens.
    ///
    /// Uses the blended rate when the input/output split is unknown.
    pub fn estimate_cost(&self, tokens: u64) -> f64 {
        self.blended_per_mtok() * (tokens as f64) / 1_000_000.0
    }
}

/// Returns pricing for a known model, or `None` if unrecognized.
///
/// Matches by substring so both `claude-sonnet-4-20250514` and `claude-sonnet-4-6`
/// resolve to the same pricing tier.
///
/// Prices are approximate and based on publicly available rates at release time.
/// Local (Ollama) models are not matched and callers should treat `None` as zero cost.
pub fn pricing_for(model: &str) -> Option<ModelPricing> {
    let m = model.to_lowercase();

    // Anthropic Claude — check opus before sonnet to avoid substring false-match.
    if m.contains("claude-opus-4") || m.contains("claude-opus-3") {
        return Some(ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
        });
    }
    if m.contains("claude-sonnet-4")
        || m.contains("claude-sonnet-3-7")
        || m.contains("claude-3-5-sonnet")
    {
        return Some(ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        });
    }
    if m.contains("claude-haiku-4")
        || m.contains("claude-3-5-haiku")
        || m.contains("claude-3-haiku")
    {
        return Some(ModelPricing {
            input_per_mtok: 0.25,
            output_per_mtok: 1.25,
        });
    }

    // OpenAI — order matters: check more specific names first.
    if m.contains("o1-mini") {
        return Some(ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 12.0,
        });
    }
    if m == "o1" || m.starts_with("o1-") || m.starts_with("o1 ") {
        return Some(ModelPricing {
            input_per_mtok: 15.0,
            output_per_mtok: 60.0,
        });
    }
    if m.contains("o3-mini") {
        return Some(ModelPricing {
            input_per_mtok: 1.1,
            output_per_mtok: 4.4,
        });
    }
    if m == "o3" || m.starts_with("o3-") || m.starts_with("o3 ") {
        return Some(ModelPricing {
            input_per_mtok: 10.0,
            output_per_mtok: 40.0,
        });
    }
    if m.contains("gpt-4o-mini") {
        return Some(ModelPricing {
            input_per_mtok: 0.15,
            output_per_mtok: 0.60,
        });
    }
    if m.contains("gpt-4o") {
        return Some(ModelPricing {
            input_per_mtok: 2.5,
            output_per_mtok: 10.0,
        });
    }
    if m.contains("gpt-4-turbo") || m.contains("gpt-4-1106") {
        return Some(ModelPricing {
            input_per_mtok: 10.0,
            output_per_mtok: 30.0,
        });
    }
    if m.contains("gpt-4") {
        return Some(ModelPricing {
            input_per_mtok: 30.0,
            output_per_mtok: 60.0,
        });
    }
    if m.contains("gpt-3.5-turbo") {
        return Some(ModelPricing {
            input_per_mtok: 0.5,
            output_per_mtok: 1.5,
        });
    }

    // Google Gemini
    if m.contains("gemini-2.0-flash") || m.contains("gemini-2-flash") {
        return Some(ModelPricing {
            input_per_mtok: 0.1,
            output_per_mtok: 0.4,
        });
    }
    if m.contains("gemini-1.5-pro") {
        return Some(ModelPricing {
            input_per_mtok: 1.25,
            output_per_mtok: 5.0,
        });
    }
    if m.contains("gemini-1.5-flash") {
        return Some(ModelPricing {
            input_per_mtok: 0.075,
            output_per_mtok: 0.3,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pricing_for_claude_sonnet() {
        let p = pricing_for("claude-sonnet-4-6").unwrap();
        assert_eq!(p.input_per_mtok, 3.0);
        assert_eq!(p.output_per_mtok, 15.0);
    }

    #[test]
    fn test_pricing_for_claude_opus() {
        let p = pricing_for("claude-opus-4-6").unwrap();
        assert_eq!(p.input_per_mtok, 15.0);
        assert_eq!(p.output_per_mtok, 75.0);
    }

    #[test]
    fn test_pricing_for_gpt4o_mini() {
        let p = pricing_for("gpt-4o-mini").unwrap();
        assert_eq!(p.input_per_mtok, 0.15);
    }

    #[test]
    fn test_pricing_for_gpt4o_not_mini() {
        let p = pricing_for("gpt-4o").unwrap();
        assert_eq!(p.input_per_mtok, 2.5);
    }

    #[test]
    fn test_pricing_for_unknown_returns_none() {
        assert!(pricing_for("llama3.2").is_none());
        assert!(pricing_for("mistral-7b").is_none());
    }

    #[test]
    fn test_pricing_for_case_insensitive() {
        assert!(pricing_for("Claude-Sonnet-4-6").is_some());
        assert!(pricing_for("GPT-4O").is_some());
    }

    #[test]
    fn test_estimate_cost() {
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        };
        // blended = (3 + 15) / 2 = 9.0 per M tokens
        // 1000 tokens = 9.0 * 1000 / 1_000_000 = 0.009 USD
        let cost = p.estimate_cost(1_000);
        assert!((cost - 0.009).abs() < 1e-9);
    }

    #[test]
    fn test_blended_per_mtok() {
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        };
        assert_eq!(p.blended_per_mtok(), 9.0);
    }
}
