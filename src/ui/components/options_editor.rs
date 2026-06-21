use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::collections::HashMap;

/// Pop-up editor for a segment's `options` `HashMap<String, serde_json::Value>`.
///
/// Key interaction model (mirrors `SeparatorEditorComponent`):
/// - `↑ / ↓`  — move between existing key entries
/// - `n`      — create a new key (opens key-name input)
/// - `Enter`  — begin editing the selected entry's value
/// - `Delete` — remove the selected entry
/// - `Esc`    — discard changes and close
/// - `S`      — confirm / save all changes
#[derive(Debug, Clone)]
pub struct OptionsEditorComponent {
    pub is_open: bool,
    /// Working copy of the options; committed back on save.
    pub entries: Vec<OptionEntry>,
    /// Currently highlighted row (key/value pair).
    pub selected_idx: usize,
    /// Whether we are editing the value of the selected entry.
    pub editing_value: bool,
    /// Whether we are entering a new key name.
    pub entering_key: bool,
    /// Buffer for the value (or new key) being typed.
    pub input_buffer: String,
}

#[derive(Debug, Clone)]
pub struct OptionEntry {
    pub key: String,
    pub value: String,
}

impl Default for OptionsEditorComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl OptionsEditorComponent {
    pub fn new() -> Self {
        Self {
            is_open: false,
            entries: Vec::new(),
            selected_idx: 0,
            editing_value: false,
            entering_key: false,
            input_buffer: String::new(),
        }
    }

    /// Open the editor, pre-populated with a segment's current options.
    pub fn open(&mut self, options: &HashMap<String, serde_json::Value>) {
        self.entries = options
            .iter()
            .map(|(k, v)| OptionEntry {
                key: k.clone(),
                value: match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
            })
            .collect();
        // Sort by key for stable display
        self.entries.sort_by(|a, b| a.key.cmp(&b.key));
        self.selected_idx = 0;
        self.editing_value = false;
        self.entering_key = false;
        self.input_buffer.clear();
        self.is_open = true;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.editing_value = false;
        self.entering_key = false;
        self.input_buffer.clear();
    }

    /// Return the current entries as a `HashMap` ready to write back into
    /// `SegmentConfig.options`.
    pub fn get_options(&self) -> HashMap<String, serde_json::Value> {
        self.entries
            .iter()
            .filter(|e| !e.key.is_empty())
            .map(|e| {
                // Try to preserve original types by parsing the string value
                let json_val = if let Ok(b) = e.value.parse::<bool>() {
                    serde_json::Value::Bool(b)
                } else if let Ok(n) = e.value.parse::<i64>() {
                    serde_json::Value::Number(n.into())
                } else if let Ok(n) = e.value.parse::<f64>() {
                    serde_json::json!(n)
                } else {
                    serde_json::Value::String(e.value.clone())
                };
                (e.key.clone(), json_val)
            })
            .collect()
    }

    // ── Navigation ───────────────────────────────────────────────────────────

    pub fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() {
            self.selected_idx = 0;
            return;
        }
        let len = self.entries.len() as i32;
        self.selected_idx =
            ((self.selected_idx as i32 + delta).rem_euclid(len)) as usize;
    }

    // ── Editing actions ──────────────────────────────────────────────────────

    /// Begin editing the value of the currently selected entry.
    pub fn start_edit_value(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.editing_value = true;
        self.entering_key = false;
        self.input_buffer = self.entries[self.selected_idx].value.clone();
    }

    /// Commit an in-progress value edit.
    pub fn finish_edit_value(&mut self) {
        if self.editing_value && !self.entries.is_empty() {
            self.entries[self.selected_idx].value = self.input_buffer.clone();
        }
        self.editing_value = false;
        self.input_buffer.clear();
    }

    /// Begin entering a new option key.
    pub fn start_new_key(&mut self) {
        self.entering_key = true;
        self.editing_value = false;
        self.input_buffer.clear();
    }

    /// Commit the new key and add an empty entry for it.
    pub fn finish_new_key(&mut self) {
        if self.entering_key {
            let key = self.input_buffer.trim().to_string();
            if !key.is_empty() && !self.entries.iter().any(|e| e.key == key) {
                self.entries.push(OptionEntry {
                    key,
                    value: String::new(),
                });
                self.entries.sort_by(|a, b| a.key.cmp(&b.key));
                // Position cursor on the newly added entry
                self.selected_idx = self
                    .entries
                    .iter()
                    .position(|e| e.key == self.input_buffer.trim())
                    .unwrap_or(0);
            }
        }
        self.entering_key = false;
        self.input_buffer.clear();
    }

    /// Remove the selected entry.
    pub fn delete_selected(&mut self) {
        if !self.entries.is_empty() {
            self.entries.remove(self.selected_idx);
            if self.selected_idx > 0 && self.selected_idx >= self.entries.len() {
                self.selected_idx = self.entries.len().saturating_sub(1);
            }
        }
    }

    // ── Character input (shared by value edit and key-name entry) ────────────

    pub fn input_char(&mut self, c: char) {
        if (self.editing_value || self.entering_key) && !c.is_control() {
            self.input_buffer.push(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.editing_value || self.entering_key {
            self.input_buffer.pop();
        }
    }

    // ── Rendering ────────────────────────────────────────────────────────────

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if !self.is_open {
            return;
        }

        let popup_height = (self.entries.len() as u16 + 7).max(12).min(area.height - 2);
        let popup_width = 70u16.min(area.width - 2);
        let popup_area = Rect {
            x: (area.width.saturating_sub(popup_width)) / 2,
            y: (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        f.render_widget(Clear, popup_area);

        let popup_block = Block::default()
            .borders(Borders::ALL)
            .title("Options Editor");
        let inner = popup_block.inner(popup_area);
        f.render_widget(popup_block, popup_area);

        let constraints = if self.editing_value || self.entering_key {
            vec![
                Constraint::Min(3),    // entries list
                Constraint::Length(3), // input field
                Constraint::Length(2), // help
            ]
        } else {
            vec![
                Constraint::Min(3),    // entries list
                Constraint::Length(2), // help
            ]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        // ── Entries list ─────────────────────────────────────────────────────
        let entries_text = if self.entries.is_empty() {
            "(no options — press [N] to add one)".to_string()
        } else {
            self.entries
                .iter()
                .enumerate()
                .map(|(i, entry)| {
                    let marker = if i == self.selected_idx { "▶" } else { " " };
                    format!("{} {}: {}", marker, entry.key, entry.value)
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        f.render_widget(
            Paragraph::new(entries_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Options (↑↓ to select)"),
            ),
            chunks[0],
        );

        // ── Input field (when active) ────────────────────────────────────────
        if self.editing_value || self.entering_key {
            let title = if self.entering_key {
                "New Option Key"
            } else {
                "Edit Value"
            };
            f.render_widget(
                Paragraph::new(format!("> {} <", self.input_buffer))
                    .style(Style::default().fg(Color::Yellow))
                    .block(Block::default().borders(Borders::ALL).title(title)),
                chunks[1],
            );
        }

        // ── Help bar ────────────────────────────────────────────────────────
        let help_idx = if self.editing_value || self.entering_key {
            2
        } else {
            1
        };
        let help_text = if self.editing_value || self.entering_key {
            "[Enter] Confirm  [Esc] Cancel edit"
        } else {
            "[Enter] Edit  [N] New  [Del] Remove  [S] Save  [Esc] Close"
        };
        f.render_widget(
            Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray)),
            chunks[help_idx],
        );
    }
}
