//! Spend accounting. What an agent reported is stored as is; any estimate is computed
//! when the spend is read, from the price table current at that moment, so a newer price
//! table corrects history without a migration.
//!
//! The price table is `<data dir>/price-table.json` when present, else the one built into
//! the app (`crates/switchyard-core/price-table.json`). Format:
//! `{"version": "...", "models": {"<model id or family>": {"input", "output", "cacheRead", "cacheWrite"?}}}`,
//! prices in USD per million tokens (cache writes cost what input costs when not named).

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use switchyard_schema::{ModelSpend, Spend};

const BUILT_IN: &str = include_str!("../price-table.json");
const FILE_NAME: &str = "price-table.json";

/// One token_usage report: running totals for what the session has spent on `model`, as of
/// `at_ms`. No `model` means the Session's Model (an agent that does not report per Model).
#[derive(Debug, Clone)]
pub(crate) struct Usage {
    pub at_ms: u64,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reported_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PriceTable {
    version: String,
    models: BTreeMap<String, Prices>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prices {
    input: f64,
    output: f64,
    cache_read: f64,
    /// Cache writes; priced as input when the table names no price for them.
    #[serde(default)]
    cache_write: Option<f64>,
}

impl PriceTable {
    /// The price table as it is on disk right now. An unreadable override falls back to
    /// the built-in table.
    pub fn read(data_dir: &Path) -> PriceTable {
        std::fs::read_to_string(data_dir.join(FILE_NAME))
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| serde_json::from_str(BUILT_IN).expect("the built-in price table parses"))
    }

    /// Prices for `model`: an exact key, else the longest key that is a dash-separated
    /// prefix of it (a dated release priced by its family).
    fn prices(&self, model: &str) -> Option<&Prices> {
        self.models
            .iter()
            .filter(|(key, _)| model == key.as_str() || model.strip_prefix(key.as_str()).is_some_and(|rest| rest.starts_with('-')))
            .max_by_key(|(key, _)| key.len())
            .map(|(_, prices)| prices)
    }
}

impl Usage {
    /// The spend this report stands for, on `model`. A reported cost is never estimated over.
    fn spend(&self, model: Option<&str>, table: &PriceTable) -> ModelSpend {
        let prices = match self.reported_usd {
            Some(_) => None,
            None => model.and_then(|m| table.prices(m)),
        };
        let per_million = |tokens: u64, usd: f64| tokens as f64 * usd / 1_000_000.0;
        ModelSpend {
            model: model.map(str::to_string),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            reported_usd: self.reported_usd,
            estimated_usd: prices.map(|p| {
                per_million(self.input_tokens, p.input)
                    + per_million(self.output_tokens, p.output)
                    + per_million(self.cache_read_tokens, p.cache_read)
                    + per_million(self.cache_write_tokens, p.cache_write.unwrap_or(p.input))
            }),
            price_table_version: prices.map(|_| table.version.clone()),
        }
    }
}

/// What `reports` (oldest first) add up to: the latest report on each Model, each priced on
/// its own Model, summed. A report naming no Model is on `session_model`, so it and a later
/// report naming that Model are one running total (as when an unresolved Default resolves).
pub(crate) fn spend(reports: &[Usage], session_model: Option<&str>, table: &PriceTable) -> Spend {
    let on = |r: &Usage| r.model.as_deref().or(session_model).map(str::to_string);
    let mut latest: Vec<(Option<String>, &Usage)> = Vec::new();
    for report in reports {
        let model = on(report);
        match latest.iter_mut().find(|(m, _)| *m == model) {
            Some(slot) => slot.1 = report,
            None => latest.push((model, report)),
        }
    }
    let by_model: Vec<ModelSpend> = latest.iter().map(|(model, r)| r.spend(model.as_deref(), table)).collect();
    let sum = |cost: fn(&ModelSpend) -> Option<f64>| by_model.iter().filter_map(cost).reduce(|a, b| a + b);
    Spend {
        input_tokens: by_model.iter().map(|m| m.input_tokens).sum(),
        output_tokens: by_model.iter().map(|m| m.output_tokens).sum(),
        cache_read_tokens: by_model.iter().map(|m| m.cache_read_tokens).sum(),
        cache_write_tokens: by_model.iter().map(|m| m.cache_write_tokens).sum(),
        reported_usd: sum(|m| m.reported_usd),
        estimated_usd: sum(|m| m.estimated_usd),
        price_table_version: by_model.iter().find_map(|m| m.price_table_version.clone()),
        by_model,
    }
}
