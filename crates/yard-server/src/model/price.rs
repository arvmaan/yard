//! The built-in per-model price table. First-party
//! list prices in USD per million tokens, as cached on 2026-10-06; an
//! estimate for cost caps only (Bedrock bills its own rates). An unknown model is priced at the highest
//! input and output rate in the table, so an unrecognised id over- rather
//! than under-spends the budget.

/// Input and output price, USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rate {
    pub input: f64,
    pub output: f64,
}

/// One table row: a model-id stem, its rate, and an optional long-context
/// rate that applies when one call's input exceeds the threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Price {
    pub stem: &'static str,
    pub rate: Rate,
    pub long_context: Option<(u64, Rate)>,
}

const fn rate(input: f64, output: f64) -> Rate {
    Rate { input, output }
}

const fn row(stem: &'static str, input: f64, output: f64) -> Price {
    Price {
        stem,
        rate: rate(input, output),
        long_context: None,
    }
}

const fn long(stem: &'static str, base: Rate, above: u64, then: Rate) -> Price {
    Price {
        stem,
        rate: base,
        long_context: Some((above, then)),
    }
}

/// The long-context rate of Sonnet 4.x.
const SONNET_4_LONG_CONTEXT: Rate = rate(6.0, 22.5);

/// The fallback for an unknown model: the highest input and the highest
/// output rate of any row (Opus 4 / 4.1 list price). A test keeps it at or
/// above every row.
pub(crate) const UNKNOWN_MODEL: Rate = rate(15.0, 75.0);

/// Every known stem. Order does not matter: a stem matches only when the
/// next character ends it (see [`stem_matches`]).
pub(crate) const PRICES: &[Price] = &[
    row("claude-sonnet-5-5", 2.0, 10.0),
    row("claude-sonnet-5", 2.0, 10.0),
    long(
        "claude-sonnet-4-6",
        rate(3.0, 15.0),
        200_000,
        SONNET_4_LONG_CONTEXT,
    ),
    long(
        "claude-sonnet-4-5",
        rate(3.0, 15.0),
        200_000,
        SONNET_4_LONG_CONTEXT,
    ),
    long(
        "claude-sonnet-4",
        rate(3.0, 15.0),
        200_000,
        SONNET_4_LONG_CONTEXT,
    ),
    row("claude-3-7-sonnet", 3.0, 15.0),
    row("claude-3-5-sonnet", 3.0, 15.0),
    long(
        "claude-haiku-5-5",
        rate(0.10, 0.50),
        100_000,
        rate(0.50, 2.50),
    ),
    row("claude-haiku-4-5", 1.0, 5.0),
    row("claude-3-5-haiku", 0.80, 4.0),
    row("claude-3-haiku", 0.25, 1.25),
    row("claude-opus-5-5", 4.0, 20.0),
    row("claude-opus-5", 5.0, 25.0),
    row("claude-opus-4-8", 5.0, 25.0),
    row("claude-opus-4-7", 5.0, 25.0),
    row("claude-opus-4-6", 5.0, 25.0),
    row("claude-opus-4-5", 5.0, 25.0),
    row("claude-opus-4-1", 15.0, 75.0),
    row("claude-opus-4", 15.0, 75.0),
    row("claude-fable-5-1", 10.0, 50.0),
    row("claude-fable-5", 10.0, 50.0),
    row("claude-mythos-5-1", 10.0, 50.0),
    row("claude-mythos-5", 10.0, 50.0),
];

/// True when `model` holds `stem` followed by its end, `-v…`, a
/// `-<8-digit date>`, `:` or `@` (so `claude-sonnet-5` does not match
/// `claude-sonnet-5-5`, and `claude-opus-4` does not match `claude-opus-4-1`).
#[must_use]
pub(crate) fn stem_matches(model: &str, stem: &str) -> bool {
    model.match_indices(stem).any(|(start, _)| {
        let rest = &model[start + stem.len()..];
        if rest.is_empty() || rest.starts_with(':') || rest.starts_with('@') {
            return true;
        }
        let Some(tail) = rest.strip_prefix('-') else {
            return false;
        };
        if tail.starts_with('v') {
            return true;
        }
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        digits == 8
            && tail[8..]
                .chars()
                .next()
                .is_none_or(|c| c == '-' || c == ':')
    })
}

/// The table row for `model`, if any.
#[must_use]
pub(crate) fn lookup(model: &str) -> Option<&'static Price> {
    PRICES.iter().find(|price| stem_matches(model, price.stem))
}

/// The rate of one call of `model` with `input_tokens` of input.
#[must_use]
pub(crate) fn rate_for(model: &str, input_tokens: u64) -> Rate {
    match lookup(model) {
        None => UNKNOWN_MODEL,
        Some(price) => match price.long_context {
            Some((above, then)) if input_tokens > above => then,
            _ => price.rate,
        },
    }
}

/// Estimated USD of one call.
#[must_use]
pub(crate) fn cost_usd(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let rate = rate_for(model, input_tokens);
    #[allow(clippy::cast_precision_loss)]
    let (input, output) = (input_tokens as f64, output_tokens as f64);
    (input * rate.input + output * rate.output) / 1_000_000.0
}
