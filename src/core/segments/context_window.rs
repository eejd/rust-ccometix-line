use super::{Segment, SegmentData};
use crate::config::{InputData, ModelConfig, SegmentId, TranscriptEntry};
use crate::utils::terminal::{format_token_count, token_label};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Default column threshold below which the unit label collapses from " tokens" to " tk".
/// Can be overridden per-segment via `options.narrow_width`.
const DEFAULT_NARROW_WIDTH: u16 = 80;

#[derive(Default)]
pub struct ContextWindowSegment;

impl ContextWindowSegment {
    pub fn new() -> Self {
        Self
    }

    /// Get context limit for the specified model (fallback when native data absent).
    fn get_context_limit_for_model(model_id: &str) -> u32 {
        let model_config = ModelConfig::load();
        model_config.get_context_limit(model_id)
    }
}

impl Segment for ContextWindowSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        // Read the column-width threshold from segment options (configurable in TUI).
        let narrow_width = crate::config::Config::load()
            .ok()
            .and_then(|c| {
                c.segments
                    .into_iter()
                    .find(|s| s.id == SegmentId::ContextWindow)
            })
            .and_then(|sc| sc.options.get("narrow_width").and_then(|v| v.as_u64()))
            .map(|v| v.clamp(1, u16::MAX as u64) as u16)
            .unwrap_or(DEFAULT_NARROW_WIDTH);

        // ── Source 1: Native context_window data (Claude Code ≥ v2.1.x) ──────────
        // The `context_window` field is populated by modern Claude Code on every
        // turn and is far more reliable than parsing the transcript.  We prefer it
        // when present and fall back to transcript-parse for older versions.
        if let Some(cw) = &input.context_window {
            if cw.context_window_size > 0 {
                let pct = cw.used_percentage;
                let percentage = if pct.fract() == 0.0 {
                    format!("{:.0}%", pct)
                } else {
                    format!("{:.1}%", pct)
                };

                // Use total_input_tokens (matches the official input-only formula).
                let tokens_str = format_token_count(cw.total_input_tokens);
                let label = token_label(narrow_width);
                let primary = format!("{} · {}{}", percentage, tokens_str, label);

                let mut metadata = HashMap::new();
                metadata.insert("tokens".to_string(), cw.total_input_tokens.to_string());
                metadata.insert("percentage".to_string(), pct.to_string());
                metadata.insert("limit".to_string(), cw.context_window_size.to_string());
                metadata.insert("model".to_string(), input.model.id.clone());
                metadata.insert("source".to_string(), "native".to_string());

                return Some(SegmentData {
                    primary,
                    secondary: String::new(),
                    metadata,
                });
            }
        }

        // ── Source 2: Transcript-parse fallback (legacy / pre-v2.1.x) ────────────
        let context_limit = Self::get_context_limit_for_model(&input.model.id);
        let context_used_token_opt = parse_transcript_usage(&input.transcript_path);

        let (percentage_display, tokens_display) = match context_used_token_opt {
            Some(context_used_token) => {
                let context_used_rate =
                    (context_used_token as f64 / context_limit as f64) * 100.0;

                let percentage = if context_used_rate.fract() == 0.0 {
                    format!("{:.0}%", context_used_rate)
                } else {
                    format!("{:.1}%", context_used_rate)
                };

                let tokens = format_token_count(context_used_token);
                (percentage, tokens)
            }
            None => ("-".to_string(), "-".to_string()),
        };

        let mut metadata = HashMap::new();
        match context_used_token_opt {
            Some(context_used_token) => {
                let context_used_rate =
                    (context_used_token as f64 / context_limit as f64) * 100.0;
                metadata.insert("tokens".to_string(), context_used_token.to_string());
                metadata.insert("percentage".to_string(), context_used_rate.to_string());
            }
            None => {
                metadata.insert("tokens".to_string(), "-".to_string());
                metadata.insert("percentage".to_string(), "-".to_string());
            }
        }
        metadata.insert("limit".to_string(), context_limit.to_string());
        metadata.insert("model".to_string(), input.model.id.clone());
        metadata.insert("source".to_string(), "transcript".to_string());

        let label = token_label(narrow_width);
        Some(SegmentData {
            primary: format!("{} · {}{}", percentage_display, tokens_display, label),
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::ContextWindow
    }
}

fn parse_transcript_usage<P: AsRef<Path>>(transcript_path: P) -> Option<u32> {
    let path = transcript_path.as_ref();

    // Try to parse from current transcript file
    if let Some(usage) = try_parse_transcript_file(path) {
        return Some(usage);
    }

    // If file doesn't exist, try to find usage from project history
    if !path.exists() {
        if let Some(usage) = try_find_usage_from_project_history(path) {
            return Some(usage);
        }
    }

    None
}

fn try_parse_transcript_file(path: &Path) -> Option<u32> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    if lines.is_empty() {
        return None;
    }

    // Check if the last line is a summary
    let last_line = lines.last()?.trim();
    if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(last_line) {
        if entry.r#type.as_deref() == Some("summary") {
            // Handle summary case: find usage by leafUuid
            if let Some(leaf_uuid) = &entry.leaf_uuid {
                let project_dir = path.parent()?;
                return find_usage_by_leaf_uuid(leaf_uuid, project_dir);
            }
        }
    }

    // Normal case: find the last assistant message in current file
    for line in lines.iter().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
            if entry.r#type.as_deref() == Some("assistant") {
                if let Some(message) = &entry.message {
                    if let Some(raw_usage) = &message.usage {
                        let normalized = raw_usage.clone().normalize();
                        return Some(normalized.display_tokens());
                    }
                }
            }
        }
    }

    None
}

fn find_usage_by_leaf_uuid(leaf_uuid: &str, project_dir: &Path) -> Option<u32> {
    // Search for the leafUuid across all session files in the project directory
    let entries = fs::read_dir(project_dir).ok()?;

    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        if let Some(usage) = search_uuid_in_file(&path, leaf_uuid) {
            return Some(usage);
        }
    }

    None
}

fn search_uuid_in_file(path: &Path, target_uuid: &str) -> Option<u32> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default();

    // Find the message with target_uuid
    for line in &lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
            if let Some(uuid) = &entry.uuid {
                if uuid == target_uuid {
                    // Found the target message, check its type
                    if entry.r#type.as_deref() == Some("assistant") {
                        // Direct assistant message with usage
                        if let Some(message) = &entry.message {
                            if let Some(raw_usage) = &message.usage {
                                let normalized = raw_usage.clone().normalize();
                                return Some(normalized.display_tokens());
                            }
                        }
                    } else if entry.r#type.as_deref() == Some("user") {
                        // User message, need to find the parent assistant message
                        if let Some(parent_uuid) = &entry.parent_uuid {
                            return find_assistant_message_by_uuid(&lines, parent_uuid);
                        }
                    }
                    break;
                }
            }
        }
    }

    None
}

fn find_assistant_message_by_uuid(lines: &[String], target_uuid: &str) -> Option<u32> {
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
            if let Some(uuid) = &entry.uuid {
                if uuid == target_uuid && entry.r#type.as_deref() == Some("assistant") {
                    if let Some(message) = &entry.message {
                        if let Some(raw_usage) = &message.usage {
                            let normalized = raw_usage.clone().normalize();
                            return Some(normalized.display_tokens());
                        }
                    }
                }
            }
        }
    }

    None
}

fn try_find_usage_from_project_history(transcript_path: &Path) -> Option<u32> {
    let project_dir = transcript_path.parent()?;

    // Find the most recent session file in the project directory
    let mut session_files: Vec<PathBuf> = Vec::new();
    let entries = fs::read_dir(project_dir).ok()?;

    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            session_files.push(path);
        }
    }

    if session_files.is_empty() {
        return None;
    }

    // Sort by modification time (most recent first)
    session_files.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    session_files.reverse();

    // Try to find usage from the most recent session
    for session_path in &session_files {
        if let Some(usage) = try_parse_transcript_file(session_path) {
            return Some(usage);
        }
    }

    None
}
