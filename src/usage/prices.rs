//! Input list prices for the Ledger overlay's Headroom savings estimate.
//! Anthropic first-party rates (claude-api skill cache, 2026-09-25), updated
//! by hand. Models without a known price get no estimate.

/// (model id substring, US cents per million input tokens). First match wins,
/// so a more specific id comes before any id it contains.
const INPUT_CENTS_PER_MTOK: &[(&str, u64)] = &[
    ("fable", 1000),
    ("mythos", 1000),
    ("opus-5-5", 400),
    ("opus-5", 500),
    ("opus-4", 500),
    ("sonnet-5", 200),
    ("sonnet-4", 300),
    ("haiku-4-5", 100),
];

/// What `saved_tokens` input tokens would have cost, in whole cents, rounded down.
pub fn estimate_cents(saved_tokens: u64, model: &str) -> Option<u64> {
    let model = model.to_ascii_lowercase();
    INPUT_CENTS_PER_MTOK
        .iter()
        .find(|(id, _)| model.contains(id))
        .map(|(_, cents)| saved_tokens.saturating_mul(*cents) / 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::estimate_cents;

    #[test]
    fn matches_the_most_specific_model_and_rounds_down() {
        let cases = [
            ("global.anthropic.claude-opus-5", 1_132_000, Some(566)),
            ("claude-opus-5-5", 26_500, Some(10)),
            ("us.anthropic.claude-opus-5-5", 1_000_000, Some(400)),
            ("claude-opus-4-8", 1_000_000, Some(500)),
            ("claude-sonnet-5-5", 1_000_000, Some(200)),
            ("claude-sonnet-4-6", 300_000, Some(90)),
            (
                "global.anthropic.claude-haiku-4-5-20251001-v1:0",
                1_000_000,
                Some(100),
            ),
            ("claude-fable-5-1", 1_000_000, Some(1000)),
            ("gpt-6-astra", 1_000_000, None),
            ("metadata", 1_000_000, None),
            ("", 1_000_000, None),
        ];
        for (model, saved, want) in cases {
            assert_eq!(estimate_cents(saved, model), want, "{model}");
        }
    }
}
