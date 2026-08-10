use crate::diagnosis::{ActionKind, Confidence, DefaultDisplay, Diagnosis, Severity};
use crate::git::snapshot::RepositorySnapshot;
use serde::Serialize;

pub const MUTATING_WARNING: &str = "Review before running — this command changes repository state.";

/// Stable schema for `oops --json`. Bump `schema_version` on breaking changes.
#[derive(Serialize)]
struct Root<'a> {
    schema_version: u32,
    repository: Repository<'a>,
    diagnoses: Vec<JsonDiagnosis<'a>>,
    changed_by_oops: bool,
}

#[derive(Serialize)]
struct Repository<'a> {
    branch: Option<&'a str>,
    detached: bool,
    head: Option<&'a str>,
    upstream: Option<&'a str>,
    ahead: Option<u32>,
    behind: Option<u32>,
    operation: &'static str,
    working_tree: WorkingTree,
    stash_count: usize,
}

#[derive(Serialize)]
struct WorkingTree {
    staged: usize,
    unstaged_modified: usize,
    unstaged_deleted: usize,
    untracked: usize,
    conflicted: usize,
}

#[derive(Serialize)]
struct JsonDiagnosis<'a> {
    id: &'a str,
    title: &'a str,
    severity: Severity,
    confidence: Confidence,
    /// One-line version of the finding, matching the compact human output.
    summary: &'a str,
    /// Whether plain `oops` shows this as a full diagnosis block.
    shown_by_default: bool,
    explanation: &'a str,
    evidence: &'a [String],
    suggestions: Vec<JsonSuggestion<'a>>,
}

#[derive(Serialize)]
struct JsonSuggestion<'a> {
    description: &'a str,
    command: &'a str,
    mutates_repository: bool,
    warning: Option<&'static str>,
}

pub fn render(snap: &RepositorySnapshot, diagnoses: &[Diagnosis]) -> serde_json::Result<String> {
    let root = Root {
        schema_version: 2,
        repository: Repository {
            branch: snap.branch.as_deref(),
            detached: snap.detached,
            head: snap.head.as_deref(),
            upstream: snap.upstream.as_deref(),
            ahead: snap.ahead_behind.map(|(a, _)| a),
            behind: snap.ahead_behind.map(|(_, b)| b),
            operation: snap.operation.as_str(),
            working_tree: WorkingTree {
                staged: snap.staged().len(),
                unstaged_modified: snap.unstaged_modified().len(),
                unstaged_deleted: snap.unstaged_deleted().len(),
                untracked: snap.untracked,
                conflicted: snap.conflicted.len(),
            },
            stash_count: snap.stashes.len(),
        },
        diagnoses: diagnoses
            .iter()
            .map(|d| JsonDiagnosis {
                id: d.id,
                title: &d.title,
                severity: d.severity,
                confidence: d.confidence,
                summary: &d.brief,
                shown_by_default: d.default_display() != DefaultDisplay::Hidden,
                explanation: &d.explanation,
                evidence: &d.evidence,
                suggestions: d
                    .suggestions
                    .iter()
                    .map(|s| {
                        let mutates = s.kind == ActionKind::Mutating;
                        JsonSuggestion {
                            description: &s.description,
                            command: &s.command,
                            mutates_repository: mutates,
                            warning: mutates.then_some(MUTATING_WARNING),
                        }
                    })
                    .collect(),
            })
            .collect(),
        changed_by_oops: false,
    };
    serde_json::to_string_pretty(&root)
}
