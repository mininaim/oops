mod rules;

pub use rules::diagnose;

use serde::Serialize;
use std::fmt;

/// How much a finding should alarm the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Git is in a broken or interrupted state, or clearly needs attention.
    Problem,
    /// Evidence suggests a possible mistake, but intent can't be proven.
    Suspicious,
    /// Useful repository hygiene or context; nothing is broken.
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Severity::Problem => "problem",
            Severity::Suspicious => "suspicious",
            Severity::Info => "info",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    ReadOnly,
    Mutating,
}

/// A command oops displays. It is never executed by oops itself.
#[derive(Debug, Clone)]
pub struct Suggestion {
    pub description: String,
    pub command: String,
    pub kind: ActionKind,
}

impl Suggestion {
    pub fn read_only(description: &str, command: &str) -> Self {
        Suggestion {
            description: description.to_string(),
            command: command.to_string(),
            kind: ActionKind::ReadOnly,
        }
    }

    pub fn mutating(description: &str, command: &str) -> Self {
        Suggestion {
            description: description.to_string(),
            command: command.to_string(),
            kind: ActionKind::Mutating,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnosis {
    /// Stable machine-readable identifier, also used in JSON output.
    pub id: &'static str,
    pub title: String,
    pub severity: Severity,
    pub confidence: Confidence,
    /// One-line version of the finding, used for compact rendering.
    pub brief: String,
    pub explanation: String,
    pub evidence: Vec<String>,
    pub suggestions: Vec<Suggestion>,
}

/// How plain `oops` (no flags) presents a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultDisplay {
    /// A full diagnosis block with evidence and suggestions.
    Full,
    /// A one-line "Worth knowing" note.
    Note,
    /// Not shown at all; available through --verbose / explain.
    Hidden,
}

impl Diagnosis {
    /// Display policy for plain `oops`: problems and defensible suspicions
    /// get full blocks, info findings become one-line notes, and anything
    /// low-confidence stays out of default output entirely.
    pub fn default_display(&self) -> DefaultDisplay {
        match (self.severity, self.confidence) {
            (Severity::Problem, _) => DefaultDisplay::Full,
            (_, Confidence::Low) => DefaultDisplay::Hidden,
            (Severity::Suspicious, _) => DefaultDisplay::Full,
            (Severity::Info, _) => DefaultDisplay::Note,
        }
    }
}
