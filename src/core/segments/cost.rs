use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId};
use std::collections::HashMap;

#[derive(Default)]
pub struct CostSegment;

impl CostSegment {
    pub fn new() -> Self {
        Self
    }
}

impl Segment for CostSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let cost_data = input.cost.as_ref()?;

        // Primary display: total cost
        let primary = if let Some(cost) = cost_data.total_cost_usd {
            if cost == 0.0 || cost < 0.01 {
                "$0".to_string()
            } else {
                format!("${:.2}", cost)
            }
        } else {
            return None;
        };

        // Optional burn-rate suffix, e.g. "($1.23/h)".
        //
        // Enabled when segment option `show_burn_rate` is truthy and the
        // session has been running for at least 60 seconds (avoids an
        // absurdly large $/h at the very start of a session).
        //
        // The TUI options editor (PR 4) will let users toggle this without
        // hand-editing config.toml.
        let show_burn_rate = crate::config::Config::load()
            .ok()
            .and_then(|c| {
                c.segments
                    .into_iter()
                    .find(|s| s.id == SegmentId::Cost)
            })
            .and_then(|sc| sc.options.get("show_burn_rate").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        let burn_rate_suffix = if show_burn_rate {
            if let (Some(cost), Some(duration_ms)) =
                (cost_data.total_cost_usd, cost_data.total_duration_ms)
            {
                let duration_hours = duration_ms as f64 / 3_600_000.0;
                if duration_hours >= (60.0 / 3600.0) {
                    // only show after ~60 s
                    let rate = cost / duration_hours;
                    format!(" (${:.2}/h)", rate)
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Extra-usage indicator: subscription window exhausted but cost
        // continues to accrue (Extra Usage / overage billing).
        //
        // We detect this heuristically: `rate_limits` is present (subscriber)
        // AND the 5-hour window is at or above 100%.  This situation would
        // normally prevent new turns, but Claude.ai Max users with Extra Usage
        // enabled can continue spending at pay-as-you-go rates.
        let extra_usage = input
            .rate_limits
            .as_ref()
            .and_then(|rl| rl.five_hour.as_ref())
            .map(|w| w.used_percentage >= 100.0)
            .unwrap_or(false);

        let extra_suffix = if extra_usage { " ⚡" } else { "" };

        let full_primary = format!("{}{}{}", primary, burn_rate_suffix, extra_suffix);

        let mut metadata = HashMap::new();
        if let Some(cost) = cost_data.total_cost_usd {
            metadata.insert("cost".to_string(), cost.to_string());
        }
        if extra_usage {
            metadata.insert("extra_usage".to_string(), "true".to_string());
        }

        Some(SegmentData {
            primary: full_primary,
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Cost
    }
}
