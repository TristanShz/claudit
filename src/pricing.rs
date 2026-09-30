//! API list prices per model and token class, and the cost arithmetic.
//!
//! The built-in table is `pricing/prices.toml`, compiled in. Nothing priced
//! is ever stored: `stats::cost` multiplies archived token counts by a
//! [`PriceTable`] at query time, so correcting a price reprices the whole
//! archive, and tests can pass their own table.
//!
//! Amounts are exact integers: a price of `$p / MTok` is `p` µ$ per token,
//! kept as picodollars per token ([`Usd`] counts picodollars), so any price
//! with up to 6 decimals multiplies without rounding.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::stats::consumption::TokenTotals;

const BUILTIN: &str = include_str!("../pricing/prices.toml");

/// An amount of US dollars, exact to the picodollar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Usd(u64);

impl Usd {
    pub const ZERO: Usd = Usd(0);

    pub fn from_picos(picos: u64) -> Self {
        Self(picos)
    }

    pub fn from_micros(micros: u64) -> Self {
        Self(micros.saturating_mul(1_000_000))
    }

    pub fn picos(self) -> u64 {
        self.0
    }

    /// The amount in dollars, for display.
    pub fn dollars(self) -> f64 {
        self.0 as f64 / 1e12
    }
}

impl std::ops::Add for Usd {
    type Output = Usd;
    fn add(self, other: Usd) -> Usd {
        Usd(self.0.saturating_add(other.0))
    }
}

impl std::ops::AddAssign for Usd {
    fn add_assign(&mut self, other: Usd) {
        *self = *self + other;
    }
}

impl fmt::Display for Usd {
    /// `$0.0012`, `$0.12`, `$12.34`, `$1,234`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.dollars();
        match d {
            _ if self.0 == 0 => write!(f, "$0"),
            d if d < 0.01 => write!(f, "${d:.4}"),
            d if d < 1_000.0 => write!(f, "${d:.2}"),
            d => {
                let whole = d.round() as u64;
                let digits = whole.to_string();
                let mut out = String::new();
                for (i, c) in digits.chars().enumerate() {
                    if i > 0 && (digits.len() - i) % 3 == 0 {
                        out.push(',');
                    }
                    out.push(c);
                }
                write!(f, "${out}")
            }
        }
    }
}

/// One model's prices, in picodollars per token (= $/MTok × 10⁶).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPrice {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

impl ModelPrice {
    /// The cost of `tokens`, of whose `cache_write` tokens `cache_write_1h`
    /// were 1-hour writes; the rest are priced as 5-minute writes.
    pub fn cost(&self, tokens: &TokenTotals, cache_write_1h: u64) -> Usd {
        let term = |n: u64, price: u64| n.saturating_mul(price);
        let cache_write_1h = cache_write_1h.min(tokens.cache_write);
        let cache_write_5m = tokens.cache_write - cache_write_1h;
        Usd(term(tokens.input, self.input)
            .saturating_add(term(tokens.output, self.output))
            .saturating_add(term(cache_write_5m, self.cache_write_5m))
            .saturating_add(term(cache_write_1h, self.cache_write_1h))
            .saturating_add(term(tokens.cache_read, self.cache_read)))
    }
}

/// Prices per model id, plus the provenance of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceTable {
    /// The date the prices were read, e.g. `2026-09-29`.
    pub version: String,
    /// Where the prices come from.
    pub source: String,
    /// Keyed by model id and by alias.
    models: BTreeMap<String, ModelPrice>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TableFile {
    version: String,
    source: String,
    models: BTreeMap<String, ModelEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelEntry {
    #[serde(default)]
    aliases: Vec<String>,
    input: f64,
    output: f64,
    cache_write_5m: f64,
    cache_write_1h: f64,
    cache_read: f64,
}

impl PriceTable {
    /// The table compiled in from `pricing/prices.toml`.
    pub fn builtin() -> &'static PriceTable {
        static TABLE: OnceLock<PriceTable> = OnceLock::new();
        TABLE.get_or_init(|| {
            PriceTable::from_toml(BUILTIN).expect("pricing/prices.toml is a valid price table")
        })
    }

    /// Parses a table in the format of `pricing/prices.toml` (USD per MTok).
    pub fn from_toml(text: &str) -> Result<Self> {
        let file: TableFile = toml::from_str(text).context("invalid price table")?;
        let mut models = BTreeMap::new();
        for (id, entry) in file.models {
            let price = ModelPrice {
                input: per_token(&id, "input", entry.input)?,
                output: per_token(&id, "output", entry.output)?,
                cache_write_5m: per_token(&id, "cache_write_5m", entry.cache_write_5m)?,
                cache_write_1h: per_token(&id, "cache_write_1h", entry.cache_write_1h)?,
                cache_read: per_token(&id, "cache_read", entry.cache_read)?,
            };
            for key in std::iter::once(id.clone()).chain(entry.aliases) {
                if models.insert(key.clone(), price).is_some() {
                    bail!("price table lists {key} twice");
                }
            }
        }
        Ok(Self {
            version: file.version,
            source: file.source,
            models,
        })
    }

    /// The prices of `model` as the API reports it, `None` when unknown.
    ///
    /// Matching: after stripping a context-window suffix (`[1m]`), a
    /// provider prefix (anything up to `anthropic.`) and a snapshot date
    /// (`-YYYYMMDD` or `@YYYYMMDD`), the id must equal a table id or alias.
    /// There is no family prefix matching, so an unlisted point release
    /// (`claude-opus-5-7`) is unknown rather than mispriced.
    pub fn price(&self, model: &str) -> Option<&ModelPrice> {
        self.models.get(normalize(model))
    }
}

/// `$/MTok` → picodollars per token, rejecting what can't be exact.
fn per_token(model: &str, class: &str, per_mtok: f64) -> Result<u64> {
    let picos = per_mtok * 1e6;
    if !picos.is_finite() || picos < 0.0 || (picos - picos.round()).abs() > 1e-6 {
        bail!(
            "price table: {model}.{class} = {per_mtok} is not a non-negative amount with at most 6 decimals"
        );
    }
    Ok(picos.round() as u64)
}

fn normalize(model: &str) -> &str {
    let mut id = model.trim();
    if let Some(open) = id.find('[')
        && id.ends_with(']')
    {
        id = &id[..open];
    }
    if let Some(at) = id.rfind("anthropic.") {
        id = &id[at + "anthropic.".len()..];
    }
    for sep in ['@', '-'] {
        if let Some((head, date)) = id.rsplit_once(sep)
            && date.len() == 8
            && date.bytes().all(|b| b.is_ascii_digit())
        {
            id = head;
            break;
        }
    }
    id
}

/// The cost of an aggregate that may span several models.
///
/// `known` sums the models the table prices; tokens of unpriced models are
/// left out of it and flagged, so a partial sum is never mistaken for the
/// whole (and an unknown model never shows as `$0`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Cost {
    /// Cost of the tokens of priced models.
    pub known: Usd,
    /// Tokens of models the price table doesn't know.
    pub unknown_tokens: TokenTotals,
    /// Those models, sorted, without duplicates.
    pub unknown_models: Vec<String>,
}

impl Cost {
    /// Prices `tokens` of `model` with `prices`, `cache_write_1h` of the
    /// cache writes at the 1-hour rate (see [`ModelPrice::cost`]).
    pub fn of(prices: &PriceTable, model: &str, tokens: &TokenTotals, cache_write_1h: u64) -> Self {
        match prices.price(model) {
            Some(price) => Cost {
                known: price.cost(tokens, cache_write_1h),
                ..Cost::default()
            },
            None => Cost {
                known: Usd::ZERO,
                unknown_tokens: *tokens,
                unknown_models: if tokens.total() > 0 {
                    vec![model.to_owned()]
                } else {
                    Vec::new()
                },
            },
        }
    }

    /// Whether every token was priced.
    pub fn is_complete(&self) -> bool {
        self.unknown_models.is_empty()
    }

    /// The full cost, `None` when some tokens are of an unknown model.
    pub fn total(&self) -> Option<Usd> {
        self.is_complete().then_some(self.known)
    }

    /// Adds another aggregate's cost into this one.
    pub fn add(&mut self, other: &Cost) {
        self.known += other.known;
        self.unknown_tokens += other.unknown_tokens;
        for model in &other.unknown_models {
            if let Err(at) = self.unknown_models.binary_search(model) {
                self.unknown_models.insert(at, model.clone());
            }
        }
    }
}
