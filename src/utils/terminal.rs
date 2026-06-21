/// Terminal width detection for the status-line render path.
///
/// In status-line mode stdout is piped (not a TTY), so the normal
/// `crossterm::terminal::size()` / `ioctl(STDOUT_FILENO, TIOCGWINSZ)` path
/// returns an error. We resolve width via a short priority chain:
///
/// 1. `$COLUMNS` environment variable — most reliable; set by most POSIX
///    shells and respected by Claude Code.
/// 2. `crossterm::terminal::size()` — works when the *controlling terminal*
///    can be queried even with piped stdout (crossterm probes stderr on some
///    platforms).
/// 3. Wide default (200) so the full label is always shown when we cannot
///    detect the terminal.
///
/// The result is intentionally cheap to call (two env lookups + one syscall
/// at worst) and returns the same value for the lifetime of one invocation,
/// so callers can call it freely without caching.
pub fn get_terminal_width() -> u16 {
    // 1. $COLUMNS (set by shell / Claude Code)
    if let Ok(cols) = std::env::var("COLUMNS") {
        if let Ok(w) = cols.trim().parse::<u16>() {
            if w > 0 {
                return w;
            }
        }
    }

    // 2. crossterm query (may work when stderr/controlling-tty is still open)
    if let Ok((w, _h)) = crossterm::terminal::size() {
        if w > 0 {
            return w;
        }
    }

    // 3. Conservative wide default — shows the full "tokens" label
    200
}

/// Format a token count as a compact string.
///
/// - Below 1 000: raw integer, e.g. `"847"`
/// - 1 000 and above: k-suffix, e.g. `"48.7k"` or `"100k"`
pub fn format_token_count(tokens: u32) -> String {
    if tokens >= 1_000 {
        let k = tokens as f64 / 1_000.0;
        if k.fract() == 0.0 {
            format!("{}k", k as u32)
        } else {
            format!("{:.1}k", k)
        }
    } else {
        tokens.to_string()
    }
}

/// Choose the token-count label suffix based on available terminal width.
///
/// | Width         | Suffix              | Example           |
/// |---------------|---------------------|-------------------|
/// | ≥ threshold   | ` tokens`           | `48.7k tokens`    |
/// | < threshold   | ` tk`               | `48.7k tk`        |
///
/// The `narrow_width` parameter (default 80) is the column boundary below
/// which the label collapses. It can be read from a segment option so the
/// user can tune it in the TUI.
pub fn token_label(narrow_width: u16) -> &'static str {
    if get_terminal_width() >= narrow_width {
        " tokens"
    } else {
        " tk"
    }
}
