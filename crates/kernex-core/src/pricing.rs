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

/// Parse the minor version from a `claude-opus-4-N` model id (lowercased).
///
/// Returns `None` for bare `claude-opus-4` and for date-suffixed 4.0 ids like
/// `claude-opus-4-20250514`: minor versions are 1-2 digits, longer digit runs
/// are date stamps.
fn opus_4_minor(m: &str) -> Option<u32> {
    let rest = &m[m.find("claude-opus-4-")? + "claude-opus-4-".len()..];
    let digits: &str = &rest[..rest
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(rest.len())];
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    digits.parse().ok()
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

    // Anthropic Claude. Order matters: match Fable/Mythos and the modern
    // (4.5+) Opus tiers before the legacy Opus rule, and Opus before Sonnet, to
    // avoid substring false-matches.
    //
    // Fable 5 / Mythos 5: $10 / $50 per MTok.
    if m.contains("claude-fable-5") || m.contains("claude-mythos") {
        return Some(ModelPricing {
            input_per_mtok: 10.0,
            output_per_mtok: 50.0,
        });
    }
    // Opus 4.x by parsed minor version: the 4.5 release cut the price to
    // $5 / $25 and later minors (4.6, 4.7, 4.8, ...) hold it, so future minors
    // do not silently fall into the legacy bucket; 4.0/4.1 bill the original
    // $15 / $75. Date-suffixed 4.0 ids (`claude-opus-4-20250514`) parse as no
    // minor and fall through to legacy below.
    if let Some(minor) = opus_4_minor(&m) {
        return Some(if minor >= 5 {
            ModelPricing {
                input_per_mtok: 5.0,
                output_per_mtok: 25.0,
            }
        } else {
            ModelPricing {
                input_per_mtok: 15.0,
                output_per_mtok: 75.0,
            }
        });
    }
    // Legacy Opus (bare/date-suffixed 4.0, Opus 3): the original $15 / $75.
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
    // Haiku 4.x (4.5): $1 / $5 per MTok.
    if m.contains("claude-haiku-4") {
        return Some(ModelPricing {
            input_per_mtok: 1.0,
            output_per_mtok: 5.0,
        });
    }
    // Legacy Haiku 3 / 3.5.
    if m.contains("claude-3-5-haiku") || m.contains("claude-3-haiku") {
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
    fn test_pricing_for_modern_opus_is_5_25() {
        // Opus 4.5+ (4.5, 4.6, 4.7, 4.8) all bill at $5 / $25.
        for model in [
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
        ] {
            let p = pricing_for(model).unwrap();
            assert_eq!(p.input_per_mtok, 5.0, "{model} input");
            assert_eq!(p.output_per_mtok, 25.0, "{model} output");
        }
    }

    #[test]
    fn test_pricing_for_legacy_opus_is_15_75() {
        // Opus 4.0 / 4.1 and Opus 3 keep the original $15 / $75.
        for model in ["claude-opus-4-0", "claude-opus-4-1", "claude-opus-3"] {
            let p = pricing_for(model).unwrap();
            assert_eq!(p.input_per_mtok, 15.0, "{model} input");
            assert_eq!(p.output_per_mtok, 75.0, "{model} output");
        }
    }

    #[test]
    fn test_pricing_for_future_opus_minors_stay_modern() {
        // 4.9 / 4.10 must NOT fall into the legacy $15/$75 bucket (silent 3x
        // misbilling); the modern price holds until a new entry says otherwise.
        for model in ["claude-opus-4-9", "claude-opus-4-10"] {
            let p = pricing_for(model).unwrap();
            assert_eq!(p.input_per_mtok, 5.0, "{model} input");
            assert_eq!(p.output_per_mtok, 25.0, "{model} output");
        }
    }

    #[test]
    fn test_pricing_for_date_suffixed_opus_4_is_legacy() {
        // claude-opus-4-20250514 is Opus 4.0 with a date stamp, not minor
        // version 20250514; it bills legacy.
        let p = pricing_for("claude-opus-4-20250514").unwrap();
        assert_eq!(p.input_per_mtok, 15.0);
        assert_eq!(p.output_per_mtok, 75.0);
    }

    #[test]
    fn test_pricing_for_future_haiku_minor_assumed_current() {
        // Documented assumption: any Haiku 4.x bills the 4.5 rate until a new
        // entry says otherwise.
        let p = pricing_for("claude-haiku-4-6").unwrap();
        assert_eq!(p.input_per_mtok, 1.0);
        assert_eq!(p.output_per_mtok, 5.0);
    }

    #[test]
    fn test_opus_4_minor_parsing() {
        assert_eq!(opus_4_minor("claude-opus-4-5"), Some(5));
        assert_eq!(opus_4_minor("claude-opus-4-10"), Some(10));
        assert_eq!(opus_4_minor("claude-opus-4-20250514"), None); // date stamp
        assert_eq!(opus_4_minor("claude-opus-4"), None); // bare
        assert_eq!(opus_4_minor("claude-opus-3"), None);
    }

    #[test]
    fn test_pricing_for_haiku_4_5() {
        let p = pricing_for("claude-haiku-4-5").unwrap();
        assert_eq!(p.input_per_mtok, 1.0);
        assert_eq!(p.output_per_mtok, 5.0);
    }

    #[test]
    fn test_pricing_for_legacy_haiku_unchanged() {
        let p = pricing_for("claude-3-5-haiku").unwrap();
        assert_eq!(p.input_per_mtok, 0.25);
        assert_eq!(p.output_per_mtok, 1.25);
    }

    #[test]
    fn test_pricing_for_fable_5() {
        let p = pricing_for("claude-fable-5").unwrap();
        assert_eq!(p.input_per_mtok, 10.0);
        assert_eq!(p.output_per_mtok, 50.0);
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
