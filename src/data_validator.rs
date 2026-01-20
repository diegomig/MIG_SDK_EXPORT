use crate::settings::{DataQuality, Settings};

/// Data quality classification for pool states.
///
/// Used to track the freshness and reliability of pool state data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateQuality {
    Fresh,
    Stale,
    Corrupt,
    ProbableCorrupt,
}

pub struct DataValidator<'a> {
    cfg: &'a DataQuality,
}

impl<'a> DataValidator<'a> {
    pub fn new(settings: &'a Settings) -> Self {
        Self {
            cfg: &settings.data_quality,
        }
    }

    pub fn classify(
        &self,
        state_age_secs: u64,
        price_deviation_pct: f64,
        liquidity_usd: f64,
    ) -> StateQuality {
        if liquidity_usd < self.cfg.min_liquidity_usd {
            return StateQuality::Corrupt;
        }
        if state_age_secs > self.cfg.state_max_age_secs {
            return StateQuality::Stale;
        }
        if price_deviation_pct > self.cfg.price_deviation_pct_threshold {
            return StateQuality::Stale;
        }
        StateQuality::Fresh
    }
}
