use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId};
use crate::utils::credentials;
use chrono::{DateTime, Datelike, Duration, Local, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── HTTP-path types (used only when native rate_limits absent) ───────────────

#[derive(Debug, Deserialize)]
struct ApiUsageResponse {
    five_hour: UsagePeriod,
    seven_day: UsagePeriod,
}

#[derive(Debug, Deserialize)]
struct UsagePeriod {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApiUsageCache {
    five_hour_utilization: f64,
    seven_day_utilization: f64,
    resets_at: Option<String>,
    cached_at: String,
}

// ── Segment impl ─────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct UsageSegment;

impl UsageSegment {
    pub fn new() -> Self {
        Self
    }

    /// Pie-slice Nerd-Font icon representing 7-day utilisation (0.0–1.0 fraction).
    fn get_circle_icon(utilization: f64) -> String {
        let percent = (utilization * 100.0) as u8;
        match percent {
            0..=12 => "\u{f0a9e}".to_string(),  // circle_slice_1
            13..=25 => "\u{f0a9f}".to_string(), // circle_slice_2
            26..=37 => "\u{f0aa0}".to_string(), // circle_slice_3
            38..=50 => "\u{f0aa1}".to_string(), // circle_slice_4
            51..=62 => "\u{f0aa2}".to_string(), // circle_slice_5
            63..=75 => "\u{f0aa3}".to_string(), // circle_slice_6
            76..=87 => "\u{f0aa4}".to_string(), // circle_slice_7
            _ => "\u{f0aa5}".to_string(),       // circle_slice_8
        }
    }

    /// Format an RFC 3339 reset-time string as a compact `M-D-H` label.
    fn format_reset_time_rfc3339(reset_time_str: Option<&str>) -> String {
        if let Some(time_str) = reset_time_str {
            if let Ok(dt) = DateTime::parse_from_rfc3339(time_str) {
                let mut local_dt = dt.with_timezone(&Local);
                if local_dt.minute() > 45 {
                    local_dt += Duration::hours(1);
                }
                return format!(
                    "{}-{}-{}",
                    local_dt.month(),
                    local_dt.day(),
                    local_dt.hour()
                );
            }
        }
        "?".to_string()
    }

    /// Format a Unix-epoch-seconds reset time as a compact `M-D-H` label.
    /// Used for the native `rate_limits.*.resets_at` field.
    fn format_reset_time_epoch(epoch_secs: Option<u64>) -> String {
        if let Some(secs) = epoch_secs {
            use chrono::TimeZone;
            if let Some(dt) = Utc.timestamp_opt(secs as i64, 0).single() {
                let mut local_dt = dt.with_timezone(&Local);
                if local_dt.minute() > 45 {
                    local_dt += Duration::hours(1);
                }
                return format!(
                    "{}-{}-{}",
                    local_dt.month(),
                    local_dt.day(),
                    local_dt.hour()
                );
            }
        }
        "?".to_string()
    }

    // ── HTTP / cache helpers (legacy path) ───────────────────────────────────

    fn get_cache_path() -> Option<std::path::PathBuf> {
        let home = dirs::home_dir()?;
        Some(
            home.join(".claude")
                .join("ccline")
                .join(".api_usage_cache.json"),
        )
    }

    fn load_cache(&self) -> Option<ApiUsageCache> {
        let cache_path = Self::get_cache_path()?;
        if !cache_path.exists() {
            return None;
        }
        let content = std::fs::read_to_string(&cache_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn save_cache(&self, cache: &ApiUsageCache) {
        if let Some(cache_path) = Self::get_cache_path() {
            if let Some(parent) = cache_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(cache) {
                let _ = std::fs::write(&cache_path, json);
            }
        }
    }

    fn is_cache_valid(&self, cache: &ApiUsageCache, cache_duration: u64) -> bool {
        if let Ok(cached_at) = DateTime::parse_from_rfc3339(&cache.cached_at) {
            let now = Utc::now();
            let elapsed = now.signed_duration_since(cached_at.with_timezone(&Utc));
            elapsed.num_seconds() < cache_duration as i64
        } else {
            false
        }
    }

    fn get_claude_code_version() -> String {
        use std::process::Command;
        let output = Command::new("npm")
            .args(["view", "@anthropic-ai/claude-code", "version"])
            .output();
        match output {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !version.is_empty() {
                    return format!("claude-code/{}", version);
                }
            }
            _ => {}
        }
        "claude-code".to_string()
    }

    fn get_proxy_from_settings() -> Option<String> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()?;
        let settings_path = format!("{}/.claude/settings.json", home);
        let content = std::fs::read_to_string(&settings_path).ok()?;
        let settings: serde_json::Value = serde_json::from_str(&content).ok()?;
        settings
            .get("env")?
            .get("HTTPS_PROXY")
            .or_else(|| settings.get("env")?.get("HTTP_PROXY"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn fetch_api_usage(
        &self,
        api_base_url: &str,
        token: &str,
        timeout_secs: u64,
    ) -> Option<ApiUsageResponse> {
        let url = format!("{}/api/oauth/usage", api_base_url);
        let user_agent = Self::get_claude_code_version();

        let agent = if let Some(proxy_url) = Self::get_proxy_from_settings() {
            if let Ok(proxy) = ureq::Proxy::new(&proxy_url) {
                ureq::Agent::config_builder()
                    .proxy(Some(proxy))
                    .build()
                    .new_agent()
            } else {
                ureq::Agent::new_with_defaults()
            }
        } else {
            ureq::Agent::new_with_defaults()
        };

        let response = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", token))
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("User-Agent", &user_agent)
            .config()
            .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
            .build()
            .call()
            .ok()?;

        response.into_body().read_json().ok()
    }
}

impl Segment for UsageSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        // ── Source 1: Native rate_limits (Claude.ai Pro/Max subscribers) ─────────
        //
        // `rate_limits` is present in the stdin JSON only for subscriber accounts
        // after the first API response. When present it is more reliable than the
        // Keychain→HTTP path: no network call, no Keychain hex-decode bug, works
        // on Windows and Linux without platform credential stores.
        //
        // Config option `usage_mode`:
        //   "auto"         (default) — native when available, HTTP otherwise
        //   "subscription" — always use native; return None if absent
        //   "api"          — always use the HTTP path (API / Extra-Usage users)
        let segment_config = crate::config::Config::load().ok().and_then(|c| {
            c.segments
                .into_iter()
                .find(|s| s.id == SegmentId::Usage)
        });

        let usage_mode = segment_config
            .as_ref()
            .and_then(|sc| sc.options.get("usage_mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("auto")
            .to_string();

        // Try native path unless user forces "api" mode
        if usage_mode != "api" {
            if let Some(rl) = &input.rate_limits {
                if let Some(five_h) = &rl.five_hour {
                    let five_hour_pct = five_h.used_percentage;
                    let seven_day_pct = rl
                        .seven_day
                        .as_ref()
                        .map(|w| w.used_percentage)
                        .unwrap_or(0.0);
                    let resets_at_epoch = rl.seven_day.as_ref().and_then(|w| w.resets_at);

                    let dynamic_icon = Self::get_circle_icon(seven_day_pct / 100.0);
                    let five_hour_percent = five_hour_pct.round() as u8;
                    let primary = format!("{}%", five_hour_percent);
                    let secondary = format!(
                        "· {}",
                        Self::format_reset_time_epoch(resets_at_epoch)
                    );

                    let mut metadata = HashMap::new();
                    metadata.insert("dynamic_icon".to_string(), dynamic_icon);
                    metadata.insert(
                        "five_hour_utilization".to_string(),
                        five_hour_pct.to_string(),
                    );
                    metadata.insert(
                        "seven_day_utilization".to_string(),
                        seven_day_pct.to_string(),
                    );
                    metadata.insert("source".to_string(), "native".to_string());

                    return Some(SegmentData {
                        primary,
                        secondary,
                        metadata,
                    });
                }
            }

            // If the user explicitly chose "subscription" mode and native data is
            // absent (API user or before the first response), surface nothing rather
            // than falling through to the HTTP path.
            if usage_mode == "subscription" {
                return None;
            }
        }

        // ── Source 2: HTTP / Keychain path (API users, Extra-Usage, legacy) ──────
        let token = credentials::get_oauth_token()?;

        let api_base_url = segment_config
            .as_ref()
            .and_then(|sc| sc.options.get("api_base_url"))
            .and_then(|v| v.as_str())
            .unwrap_or("https://api.anthropic.com");

        let cache_duration = segment_config
            .as_ref()
            .and_then(|sc| sc.options.get("cache_duration"))
            .and_then(|v| v.as_u64())
            .unwrap_or(300);

        let timeout = segment_config
            .as_ref()
            .and_then(|sc| sc.options.get("timeout"))
            .and_then(|v| v.as_u64())
            .unwrap_or(2);

        let cached_data = self.load_cache();
        let use_cached = cached_data
            .as_ref()
            .map(|cache| self.is_cache_valid(cache, cache_duration))
            .unwrap_or(false);

        let (five_hour_util, seven_day_util, resets_at) = if use_cached {
            let cache = cached_data.unwrap();
            (
                cache.five_hour_utilization,
                cache.seven_day_utilization,
                cache.resets_at,
            )
        } else {
            match self.fetch_api_usage(api_base_url, &token, timeout) {
                Some(response) => {
                    let cache = ApiUsageCache {
                        five_hour_utilization: response.five_hour.utilization,
                        seven_day_utilization: response.seven_day.utilization,
                        resets_at: response.seven_day.resets_at.clone(),
                        cached_at: Utc::now().to_rfc3339(),
                    };
                    self.save_cache(&cache);
                    (
                        response.five_hour.utilization,
                        response.seven_day.utilization,
                        response.seven_day.resets_at,
                    )
                }
                None => {
                    if let Some(cache) = cached_data {
                        (
                            cache.five_hour_utilization,
                            cache.seven_day_utilization,
                            cache.resets_at,
                        )
                    } else {
                        return None;
                    }
                }
            }
        };

        let dynamic_icon = Self::get_circle_icon(seven_day_util / 100.0);
        let five_hour_percent = five_hour_util.round() as u8;
        let primary = format!("{}%", five_hour_percent);
        let secondary = format!(
            "· {}",
            Self::format_reset_time_rfc3339(resets_at.as_deref())
        );

        let mut metadata = HashMap::new();
        metadata.insert("dynamic_icon".to_string(), dynamic_icon);
        metadata.insert(
            "five_hour_utilization".to_string(),
            five_hour_util.to_string(),
        );
        metadata.insert(
            "seven_day_utilization".to_string(),
            seven_day_util.to_string(),
        );
        metadata.insert("source".to_string(), "http".to_string());

        Some(SegmentData {
            primary,
            secondary,
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Usage
    }
}
