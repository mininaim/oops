pub mod human;
pub mod json;
pub mod spinner;

use std::io::IsTerminal;

/// How much color the terminal can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorLevel {
    None,
    Basic,
    Xterm256,
    TrueColor,
}

/// The oops palette. One brand, three renderings:
///
/// | role     | truecolor            | 256  | 16-color |
/// |----------|----------------------|------|----------|
/// | problem  | coral    `#F4634D`   | 203  | red      |
/// | caution  | amber    `#F2A65A`   | 215  | yellow   |
/// | safe     | mint     `#59C89B`   | 78   | green    |
/// | command  | sky      `#7FB4F2`   | 111  | cyan     |
/// | meta     | slate    `#7A8690`   | 245  | dim      |
///
/// Primary prose always stays the terminal's default color.
#[derive(Debug, Clone, Copy)]
enum Role {
    Problem,
    Caution,
    Safe,
    Command,
    Meta,
}

impl Role {
    fn code(self, level: ColorLevel) -> &'static str {
        match (self, level) {
            (Role::Problem, ColorLevel::TrueColor) => "38;2;244;99;77",
            (Role::Problem, ColorLevel::Xterm256) => "38;5;203",
            (Role::Problem, _) => "31",
            (Role::Caution, ColorLevel::TrueColor) => "38;2;242;166;90",
            (Role::Caution, ColorLevel::Xterm256) => "38;5;215",
            (Role::Caution, _) => "33",
            (Role::Safe, ColorLevel::TrueColor) => "38;2;89;200;155",
            (Role::Safe, ColorLevel::Xterm256) => "38;5;78",
            (Role::Safe, _) => "32",
            (Role::Command, ColorLevel::TrueColor) => "38;2;127;180;242",
            (Role::Command, ColorLevel::Xterm256) => "38;5;111",
            (Role::Command, _) => "36",
            (Role::Meta, ColorLevel::TrueColor) => "38;2;122;134;144",
            (Role::Meta, ColorLevel::Xterm256) => "38;5;245",
            (Role::Meta, _) => "2",
        }
    }
}

/// The tiny glyph vocabulary, borrowed from Git-graph geometry.
/// Falls back to ASCII when the locale is explicitly non-UTF-8.
#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    /// A solid finding — something is definitely here. Problems.
    pub problem: &'static str,
    /// A hollow finding — present but unproven. Suspicions.
    pub suspicious: &'static str,
    /// A passing remark. Info notes.
    pub note: &'static str,
    /// Healthy.
    pub check: &'static str,
    /// Action / command prefix.
    pub arrow: &'static str,
    /// The vertical rail of the activity graph.
    pub rail: &'static str,
    /// A commit on the activity graph.
    pub commit: &'static str,
    /// Any other movement on the activity graph.
    pub movement: &'static str,
    /// Branch-to-branch movement, e.g. `main → feat/checkout`.
    pub flow: &'static str,
    /// The closing branch tail — the oops signature mark.
    pub tail: &'static str,
}

const UNICODE_GLYPHS: Glyphs = Glyphs {
    problem: "●",
    suspicious: "○",
    note: "·",
    check: "✓",
    arrow: "›",
    rail: "│",
    commit: "●",
    movement: "○",
    flow: "→",
    tail: "╰╴",
};

const ASCII_GLYPHS: Glyphs = Glyphs {
    problem: "*",
    suspicious: "o",
    note: "-",
    check: "+",
    arrow: ">",
    rail: "|",
    commit: "*",
    movement: "o",
    flow: "->",
    tail: "`-",
};

/// Whether colored output is appropriate. Pure so tests can cover the matrix
/// without a real terminal.
pub fn color_allowed(stdout_tty: bool, no_color_set: bool, term_dumb: bool) -> bool {
    stdout_tty && !no_color_set && !term_dumb
}

/// Whether the spinner may animate. Stricter than color: the spinner draws
/// on stderr while results go to stdout, so both must be interactive.
pub fn animation_allowed(
    stdout_tty: bool,
    stderr_tty: bool,
    no_color_set: bool,
    term_dumb: bool,
) -> bool {
    color_allowed(stdout_tty, no_color_set, term_dumb) && stderr_tty
}

fn no_color_set() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

fn term_dumb() -> bool {
    std::env::var_os("TERM").is_some_and(|t| t == "dumb")
}

/// Unicode unless the locale explicitly opts out (e.g. LC_ALL=C).
pub fn locale_supports_unicode() -> bool {
    let vars = ["LC_ALL", "LC_CTYPE", "LANG"];
    let set: Vec<String> = vars
        .iter()
        .filter_map(std::env::var_os)
        .map(|v| v.to_string_lossy().to_uppercase())
        .filter(|v| !v.is_empty())
        .collect();
    set.is_empty()
        || set
            .iter()
            .any(|v| v.contains("UTF-8") || v.contains("UTF8"))
}

pub fn detect_animation() -> bool {
    animation_allowed(
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
        no_color_set(),
        term_dumb(),
    )
}

pub struct Style {
    level: ColorLevel,
    glyphs: Glyphs,
    /// Usable columns for wrapped prose.
    pub width: usize,
}

impl Style {
    pub fn detect() -> Self {
        let level = if !color_allowed(std::io::stdout().is_terminal(), no_color_set(), term_dumb())
        {
            ColorLevel::None
        } else {
            match supports_color::on_cached(supports_color::Stream::Stdout) {
                Some(support) if support.has_16m => ColorLevel::TrueColor,
                Some(support) if support.has_256 => ColorLevel::Xterm256,
                Some(_) => ColorLevel::Basic,
                None => ColorLevel::None,
            }
        };
        let width = terminal_size::terminal_size()
            .map(|(w, _)| w.0 as usize)
            .unwrap_or(80)
            .clamp(40, 100);
        Style {
            level,
            glyphs: if locale_supports_unicode() {
                UNICODE_GLYPHS
            } else {
                ASCII_GLYPHS
            },
            width,
        }
    }

    pub fn plain() -> Self {
        Style {
            level: ColorLevel::None,
            glyphs: UNICODE_GLYPHS,
            width: 80,
        }
    }

    #[cfg(test)]
    pub fn with_level(level: ColorLevel) -> Self {
        Style {
            level,
            glyphs: UNICODE_GLYPHS,
            width: 80,
        }
    }

    pub fn glyphs(&self) -> Glyphs {
        self.glyphs
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.level == ColorLevel::None {
            text.to_string()
        } else {
            format!("\x1b[{code}m{text}\x1b[0m")
        }
    }

    fn role(&self, role: Role, text: &str) -> String {
        self.wrap(role.code(self.level), text)
    }

    pub fn bold(&self, text: &str) -> String {
        self.wrap("1", text)
    }

    /// Metadata, evidence, secondary prose — slate.
    pub fn dim(&self, text: &str) -> String {
        self.role(Role::Meta, text)
    }

    /// Healthy / safe — mint.
    pub fn ok(&self, text: &str) -> String {
        self.role(Role::Safe, text)
    }

    /// Suspicion and mutating-command annotations — amber.
    pub fn warn(&self, text: &str) -> String {
        self.role(Role::Caution, text)
    }

    /// The problem focal point — coral. Used sparingly.
    pub fn danger(&self, text: &str) -> String {
        self.role(Role::Problem, text)
    }

    /// Commands and action hints — sky.
    pub fn accent(&self, text: &str) -> String {
        self.role(Role::Command, text)
    }
}

/// Greedy word-wrap for tidy terminal paragraphs.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Compact relative time: "2m", "3h", "5d", "now".
pub fn rel_time_short(now: u64, then: u64) -> String {
    let delta = now.saturating_sub(then);
    match delta {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", delta / 60),
        3600..=86399 => format!("{}h", delta / 3600),
        _ => format!("{}d", delta / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_respects_width() {
        let lines = wrap_text("one two three four five six seven", 10);
        assert!(lines.iter().all(|l| l.len() <= 10));
        assert_eq!(lines.join(" "), "one two three four five six seven");
    }

    #[test]
    fn rel_time_short_buckets() {
        assert_eq!(rel_time_short(1000, 990), "now");
        assert_eq!(rel_time_short(1000, 1000 - 120), "2m");
        assert_eq!(rel_time_short(100_000, 100_000 - 7200), "2h");
        assert_eq!(rel_time_short(1_000_000, 1_000_000 - 200_000), "2d");
    }

    #[test]
    fn color_requires_tty_and_no_color_unset() {
        assert!(color_allowed(true, false, false));
        assert!(!color_allowed(false, false, false), "piped output");
        assert!(!color_allowed(true, true, false), "NO_COLOR set");
        assert!(!color_allowed(true, false, true), "TERM=dumb");
    }

    #[test]
    fn animation_requires_both_streams_interactive() {
        assert!(animation_allowed(true, true, false, false));
        assert!(
            !animation_allowed(false, true, false, false),
            "stdout piped"
        );
        assert!(
            !animation_allowed(true, false, false, false),
            "stderr redirected"
        );
        assert!(!animation_allowed(true, true, true, false), "NO_COLOR set");
        assert!(!animation_allowed(true, true, false, true), "TERM=dumb");
    }

    #[test]
    fn plain_style_emits_no_ansi() {
        let style = Style::plain();
        for text in [
            style.bold("x"),
            style.dim("x"),
            style.ok("x"),
            style.warn("x"),
            style.danger("x"),
            style.accent("x"),
        ] {
            assert_eq!(text, "x");
        }
    }

    #[test]
    fn palette_degrades_by_capability() {
        let truecolor = Style::with_level(ColorLevel::TrueColor);
        assert_eq!(truecolor.danger("x"), "\x1b[38;2;244;99;77mx\x1b[0m");
        assert_eq!(truecolor.accent("x"), "\x1b[38;2;127;180;242mx\x1b[0m");

        let xterm = Style::with_level(ColorLevel::Xterm256);
        assert_eq!(xterm.danger("x"), "\x1b[38;5;203mx\x1b[0m");
        assert_eq!(xterm.dim("x"), "\x1b[38;5;245mx\x1b[0m");

        let basic = Style::with_level(ColorLevel::Basic);
        assert_eq!(basic.danger("x"), "\x1b[31mx\x1b[0m");
        assert_eq!(basic.dim("x"), "\x1b[2mx\x1b[0m");
    }
}
