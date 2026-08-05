//! Cost estimation for memory extraction.
//!
//! Before any analyze run spends money, we estimate it: how many substantial
//! un-analyzed turns, roughly how many tokens, and the dollar cost at the
//! configured provider/model's list price. The estimate is deliberately
//! approximate (a chars/4 token heuristic and a fixed per-turn output budget) —
//! its job is to keep "point this at gigabytes of history" from being a
//! surprise, not to bill to the cent.

/// List price in USD per 1M tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPrice {
    pub input_per_m: f64,
    pub output_per_m: f64,
}

/// Approximate list prices (USD / 1M tokens), as of 2026-08. Matched by model
/// substring first, then a provider default. These drift — update as needed;
/// estimates are approximate by design.
pub fn price_for(provider: &str, model: &str) -> ModelPrice {
    let m = model.to_ascii_lowercase();
    let p = |i: f64, o: f64| ModelPrice {
        input_per_m: i,
        output_per_m: o,
    };
    // Anthropic
    if m.contains("haiku") {
        return p(1.0, 5.0);
    }
    if m.contains("sonnet") {
        return p(3.0, 15.0);
    }
    if m.contains("opus") {
        return p(15.0, 75.0);
    }
    // Gemini — checked before the OpenAI mini/nano rules because "geMINI"
    // contains the substring "mini".
    if m.contains("flash") {
        return p(0.15, 0.60);
    }
    if m.contains("gemini") {
        return p(1.25, 5.0);
    }
    // OpenAI
    if m.contains("nano") {
        return p(0.05, 0.40);
    }
    if m.contains("mini") {
        return p(0.25, 2.0);
    }
    if m.starts_with("gpt-5") || m.starts_with("gpt5") {
        return p(1.25, 10.0);
    }
    // Provider fallback
    match provider {
        "anthropic" => p(1.0, 5.0),
        "openai" | "openai-compatible" => p(0.25, 2.0),
        "gemini" => p(0.15, 0.60),
        _ => p(1.0, 5.0),
    }
}

/// Rough tokens ≈ chars / 4.
const CHARS_PER_TOKEN: usize = 4;
/// Extraction prompt (EXTRACTION_SYSTEM) sent once per call — approx tokens.
const SYSTEM_PROMPT_TOKENS: u64 = 300;
/// Extraction returns a compact JSON array; budget a small fixed output.
const OUTPUT_TOKENS_PER_TURN: u64 = 350;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CostEstimate {
    pub turns: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub usd: f64,
}

/// Estimate the cost of extracting `turns` substantial turns whose exchange
/// texts total `total_exchange_chars` characters, at `price`.
pub fn estimate_cost(
    turns: usize,
    total_exchange_chars: usize,
    price: &ModelPrice,
) -> CostEstimate {
    let input_tokens =
        SYSTEM_PROMPT_TOKENS * turns as u64 + (total_exchange_chars / CHARS_PER_TOKEN) as u64;
    let output_tokens = OUTPUT_TOKENS_PER_TURN * turns as u64;
    let usd = (input_tokens as f64 / 1_000_000.0) * price.input_per_m
        + (output_tokens as f64 / 1_000_000.0) * price.output_per_m;
    CostEstimate {
        turns,
        input_tokens,
        output_tokens,
        usd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_table_matches_model_then_provider() {
        assert_eq!(
            price_for("anthropic", "claude-haiku-4-5"),
            price_for("x", "haiku")
        );
        assert_eq!(price_for("openai", "gpt-5-mini").input_per_m, 0.25);
        assert_eq!(price_for("gemini", "gemini-3-flash").input_per_m, 0.15);
        // Unknown model falls back to provider default.
        assert_eq!(price_for("anthropic", "mystery").input_per_m, 1.0);
    }

    #[test]
    fn estimate_scales_with_turns_and_chars() {
        let price = ModelPrice {
            input_per_m: 1.0,
            output_per_m: 5.0,
        };
        let e = estimate_cost(10, 40_000, &price);
        assert_eq!(e.turns, 10);
        // 300*10 system + 40000/4 exchange = 3000 + 10000 = 13000 input tokens
        assert_eq!(e.input_tokens, 13_000);
        assert_eq!(e.output_tokens, 3_500);
        // 13000/1e6*1 + 3500/1e6*5 = 0.013 + 0.0175
        assert!((e.usd - 0.0305).abs() < 1e-9);

        // Zero turns → zero cost.
        assert_eq!(estimate_cost(0, 0, &price).usd, 0.0);
    }
}
