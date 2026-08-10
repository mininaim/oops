use super::{Confidence, Diagnosis, Severity, Suggestion};
use crate::git::snapshot::{Operation, RepositorySnapshot};

/// A switch-then-commit gap at or below this is treated as "suspiciously quick".
const WRONG_BRANCH_WINDOW_SECS: u64 = 120;
/// The wrong-branch heuristic only looks at commits this recent.
const WRONG_BRANCH_RECENCY_SECS: u64 = 3600;
/// A stash older than this is described as possibly forgotten.
const STASH_FORGOTTEN_SECS: u64 = 24 * 3600;

/// Runs every rule against the snapshot and returns the findings ordered
/// problem → suspicious → info. Pure function: same snapshot in, same
/// diagnoses out.
pub fn diagnose(snap: &RepositorySnapshot) -> Vec<Diagnosis> {
    let mut found = Vec::new();

    let operation_active = snap.operation != Operation::None;
    match snap.operation {
        Operation::Merge => found.push(merge_in_progress(snap)),
        Operation::Rebase => found.push(rebase_in_progress(snap)),
        Operation::CherryPick => found.push(cherry_pick_in_progress(snap)),
        Operation::Revert => found.push(revert_in_progress(snap)),
        Operation::None => {}
    }

    // While an operation is in progress, a dirty tree, conflicts, detached
    // HEAD and reflog churn are expected side effects, not separate findings.
    if !operation_active {
        found.extend(detached_head(snap));
        found.extend(upstream_state(snap));
        found.extend(tracked_files_deleted(snap));
        found.extend(wrong_branch_commit(snap));
        found.extend(staged_changes(snap));
        found.extend(dirty_working_tree(snap));
    }

    found.extend(stash_entries(snap));

    if found.is_empty() {
        found.push(all_clear(snap));
    }
    found.sort_by_key(|d| match d.severity {
        Severity::Problem => 0,
        Severity::Suspicious => 1,
        Severity::Info => 2,
    });
    found
}

fn merge_in_progress(snap: &RepositorySnapshot) -> Diagnosis {
    let conflicts = snap.conflicted.len();
    let branch = snap.branch.as_deref().unwrap_or("this branch");
    let explanation = if conflicts > 0 {
        format!(
            "A merge into {branch} is in progress and {} unresolved {}.",
            conflicts,
            plural(conflicts, "conflict remains", "conflicts remain")
        )
    } else {
        format!(
            "A merge into {branch} is in progress. No conflicts are unresolved, \
             but the merge has not been committed yet."
        )
    };

    let mut evidence = vec!["MERGE_HEAD exists in the git directory".to_string()];
    push_conflict_evidence(&mut evidence, snap);

    let mut suggestions = vec![Suggestion::read_only(
        "see where things stand",
        "git status",
    )];
    if conflicts > 0 {
        suggestions.push(Suggestion::read_only(
            "list the conflicted files",
            "git diff --name-only --diff-filter=U",
        ));
        suggestions.push(Suggestion::mutating(
            "finish once conflicts are staged",
            "git merge --continue",
        ));
    } else {
        suggestions.push(Suggestion::mutating(
            "finish the merge",
            "git merge --continue",
        ));
    }
    suggestions.push(Suggestion::mutating(
        "abandon and restore the pre-merge state",
        "git merge --abort",
    ));

    Diagnosis {
        id: "merge_in_progress",
        title: "Merge in progress".to_string(),
        severity: Severity::Problem,
        confidence: Confidence::High,
        brief: if conflicts > 0 {
            format!(
                "{conflicts} {} attention",
                plural(conflicts, "conflict needs", "conflicts need")
            )
        } else {
            "no conflicts remain — the merge just needs a commit".to_string()
        },
        explanation,
        evidence,
        suggestions,
    }
}

fn rebase_in_progress(snap: &RepositorySnapshot) -> Diagnosis {
    let conflicts = snap.conflicted.len();
    let branch = snap
        .rebase
        .branch
        .clone()
        .or_else(|| snap.branch.clone())
        .unwrap_or_else(|| "your branch".to_string());
    let onto = snap
        .rebase
        .onto
        .as_deref()
        .map(|o| format!(" onto {o}"))
        .unwrap_or_default();
    let explanation = if conflicts > 0 {
        format!(
            "You're rebasing {branch}{onto}. {} unresolved {}.",
            conflicts,
            plural(conflicts, "conflict remains", "conflicts remain")
        )
    } else {
        format!(
            "You're rebasing {branch}{onto}. No conflicts are currently unresolved — \
             the rebase is paused mid-step."
        )
    };

    let mut evidence = vec!["a rebase state directory exists in the git directory".to_string()];
    if snap.detached {
        evidence.push("HEAD is detached (normal during a rebase)".to_string());
    }
    push_conflict_evidence(&mut evidence, snap);

    Diagnosis {
        id: "rebase_in_progress",
        title: "Rebase in progress".to_string(),
        severity: Severity::Problem,
        confidence: Confidence::High,
        brief: if conflicts > 0 {
            format!(
                "{conflicts} {} attention",
                plural(conflicts, "conflict needs", "conflicts need")
            )
        } else {
            "paused mid-step — no conflicts remain".to_string()
        },
        explanation,
        evidence,
        suggestions: vec![
            Suggestion::read_only("see where the rebase stopped", "git status"),
            Suggestion::mutating("continue after resolving", "git rebase --continue"),
            Suggestion::mutating(
                &format!("abandon and restore {branch}"),
                "git rebase --abort",
            ),
        ],
    }
}

fn cherry_pick_in_progress(snap: &RepositorySnapshot) -> Diagnosis {
    let conflicts = snap.conflicted.len();
    let explanation = if conflicts > 0 {
        format!(
            "A cherry-pick is in progress and {} unresolved {}.",
            conflicts,
            plural(conflicts, "conflict remains", "conflicts remain")
        )
    } else {
        "A cherry-pick is in progress but has not been committed yet.".to_string()
    };
    let mut evidence = vec!["CHERRY_PICK_HEAD exists in the git directory".to_string()];
    push_conflict_evidence(&mut evidence, snap);

    Diagnosis {
        id: "cherry_pick_in_progress",
        title: "Cherry-pick in progress".to_string(),
        severity: Severity::Problem,
        confidence: Confidence::High,
        brief: if conflicts > 0 {
            format!(
                "{conflicts} {} attention",
                plural(conflicts, "conflict needs", "conflicts need")
            )
        } else {
            "paused — it still needs a commit".to_string()
        },
        explanation,
        evidence,
        suggestions: vec![
            Suggestion::read_only("see which paths need attention", "git status"),
            Suggestion::mutating("continue after resolving", "git cherry-pick --continue"),
            Suggestion::mutating(
                "abandon and restore the previous state",
                "git cherry-pick --abort",
            ),
        ],
    }
}

fn revert_in_progress(snap: &RepositorySnapshot) -> Diagnosis {
    let conflicts = snap.conflicted.len();
    let explanation = if conflicts > 0 {
        format!(
            "A revert is in progress and {} unresolved {}.",
            conflicts,
            plural(conflicts, "conflict remains", "conflicts remain")
        )
    } else {
        "A revert is in progress but has not been committed yet.".to_string()
    };
    let mut evidence = vec!["REVERT_HEAD exists in the git directory".to_string()];
    push_conflict_evidence(&mut evidence, snap);

    Diagnosis {
        id: "revert_in_progress",
        title: "Revert in progress".to_string(),
        severity: Severity::Problem,
        confidence: Confidence::High,
        brief: if conflicts > 0 {
            format!(
                "{conflicts} {} attention",
                plural(conflicts, "conflict needs", "conflicts need")
            )
        } else {
            "paused — it still needs a commit".to_string()
        },
        explanation,
        evidence,
        suggestions: vec![
            Suggestion::read_only("see which paths need attention", "git status"),
            Suggestion::mutating("continue after resolving", "git revert --continue"),
            Suggestion::mutating(
                "abandon and restore the previous state",
                "git revert --abort",
            ),
        ],
    }
}

fn detached_head(snap: &RepositorySnapshot) -> Option<Diagnosis> {
    if !snap.detached {
        return None;
    }
    let head = snap.head.as_deref().unwrap_or("an unknown commit");
    let mut evidence = vec![format!("git reports HEAD detached at {head}")];
    if let Some(subject) = snap.head_subject() {
        evidence.push(format!("that commit is: {subject}"));
    }
    if let Some(entry) = snap
        .reflog
        .iter()
        .find(|e| e.subject.starts_with("checkout: moving from"))
    {
        evidence.push(format!("reflog: {}", entry.subject));
    }

    Some(Diagnosis {
        id: "detached_head",
        title: "Detached HEAD".to_string(),
        severity: Severity::Problem,
        confidence: Confidence::High,
        brief: format!("HEAD sits on {head} directly — commits made here are easy to lose"),
        explanation: format!(
            "HEAD points directly at commit {head} instead of a branch. \
             New commits made here are easy to lose track of, because no branch moves with them."
        ),
        evidence,
        suggestions: vec![
            Suggestion::read_only("see where you are", "git log --oneline -5"),
            Suggestion::mutating("go back to the previous branch", "git switch -"),
            Suggestion::mutating(
                "or keep this work on a real branch",
                "git switch -c <branch-name>",
            ),
        ],
    })
}

fn upstream_state(snap: &RepositorySnapshot) -> Option<Diagnosis> {
    let upstream = snap.upstream.as_deref()?;
    let (ahead, behind) = snap.ahead_behind?;
    let branch = snap.branch.as_deref().unwrap_or("this branch");

    if ahead > 0 && behind > 0 {
        return Some(Diagnosis {
            id: "diverged_from_upstream",
            title: "Branch diverged from upstream".to_string(),
            severity: Severity::Problem,
            confidence: Confidence::High,
            brief: format!(
                "{branch} and {upstream} have diverged ({ahead} local / {behind} upstream)"
            ),
            explanation: format!(
                "{branch} and {upstream} each have commits the other doesn't \
                 ({ahead} local, {behind} upstream). A plain pull will combine them. \
                 Avoid force-pushing to make this go away."
            ),
            evidence: vec![format!(
                "git reports {branch} is ahead {ahead} and behind {behind} of {upstream}"
            )],
            suggestions: vec![
                Suggestion::read_only(
                    "see both sides of the divergence",
                    "git log --left-right --oneline @{upstream}...HEAD",
                ),
                Suggestion::mutating(
                    "replay your commits on top of upstream",
                    "git pull --rebase",
                ),
                Suggestion::mutating("or combine both sides with a merge", "git pull"),
            ],
        });
    }
    if behind > 0 {
        return Some(Diagnosis {
            id: "behind_upstream",
            title: "Branch behind upstream".to_string(),
            severity: Severity::Info,
            confidence: Confidence::High,
            brief: format!(
                "{branch} is {behind} {} behind {upstream} — a fast-forward pull is safe",
                plural(behind as usize, "commit", "commits")
            ),
            explanation: format!(
                "{branch} is {behind} {} behind {upstream}, with no local commits on top. \
                 Fast-forwarding is safe — nothing local would be rewritten.",
                plural(behind as usize, "commit", "commits")
            ),
            evidence: vec![format!(
                "git reports {branch} is behind {upstream} by {behind}"
            )],
            suggestions: vec![
                Suggestion::read_only(
                    "see what upstream has",
                    "git log HEAD..@{upstream} --oneline",
                ),
                Suggestion::mutating("fast-forward to upstream", "git pull --ff-only"),
            ],
        });
    }
    if ahead > 0 {
        return Some(Diagnosis {
            id: "unpushed_commits",
            title: "Local commits not pushed".to_string(),
            severity: Severity::Info,
            confidence: Confidence::High,
            brief: format!(
                "{branch} is {ahead} {} ahead of {upstream} (not pushed yet)",
                plural(ahead as usize, "commit", "commits")
            ),
            explanation: format!(
                "{branch} is {ahead} {} ahead of {upstream}. \
                 {} only on this machine until pushed.",
                plural(ahead as usize, "commit", "commits"),
                plural(ahead as usize, "That commit exists", "Those commits exist")
            ),
            evidence: vec![format!(
                "git reports {branch} is ahead of {upstream} by {ahead}"
            )],
            suggestions: vec![
                Suggestion::read_only(
                    "see what hasn't been pushed",
                    "git log @{upstream}..HEAD --oneline",
                ),
                Suggestion::mutating("push when ready", "git push"),
            ],
        });
    }
    None
}

fn tracked_files_deleted(snap: &RepositorySnapshot) -> Option<Diagnosis> {
    let deleted = snap.unstaged_deleted();
    if deleted.is_empty() {
        return None;
    }
    let n = deleted.len();
    let mut evidence: Vec<String> = deleted
        .iter()
        .take(5)
        .map(|e| format!("deleted (not staged): {}", e.path))
        .collect();
    if n > 5 {
        evidence.push(format!("… and {} more", n - 5));
    }
    let example = &deleted[0].path;

    Some(Diagnosis {
        id: "tracked_files_deleted",
        title: "Tracked files deleted".to_string(),
        severity: Severity::Suspicious,
        confidence: Confidence::Medium,
        brief: format!(
            "{n} tracked {} deleted but the deletion is not staged",
            plural(n, "file is", "files are")
        ),
        explanation: format!(
            "{n} tracked {} deleted from the working tree, but the deletion is not staged. \
             If that wasn't deliberate, the {} can be restored from the last commit.",
            plural(n, "file is", "files are"),
            plural(n, "file", "files")
        ),
        evidence,
        suggestions: vec![
            Suggestion::read_only("confirm what's missing", "git status"),
            Suggestion::mutating(
                "restore the file from the last commit",
                &format!("git restore -- '{example}'"),
            ),
        ],
    })
}

fn is_long_lived(branch: &str) -> bool {
    matches!(branch, "main" | "master" | "develop" | "dev" | "trunk")
}

/// Wrong-branch heuristic. A quick `checkout → commit` alone is normal Git
/// usage, so timing by itself only ever produces a low-confidence note that
/// stays out of default output. The defensible accident signal is direction:
/// leaving a feature branch and immediately committing on a long-lived branch
/// (main, master, …) is the classic "committed on main by mistake" shape.
fn wrong_branch_commit(snap: &RepositorySnapshot) -> Option<Diagnosis> {
    let commit = snap.reflog.first()?;
    if !commit.subject.starts_with("commit:") {
        return None;
    }
    let switch = snap.reflog.get(1)?;
    let moved = switch.subject.strip_prefix("checkout: moving from ")?;
    let (from, to) = moved.split_once(" to ")?;
    if from == to || snap.branch.as_deref() != Some(to) {
        return None;
    }
    let gap = commit.time.saturating_sub(switch.time);
    if gap > WRONG_BRANCH_WINDOW_SECS
        || snap.now.saturating_sub(commit.time) > WRONG_BRANCH_RECENCY_SECS
    {
        return None;
    }

    let gap_text = if gap < 5 {
        "moments".to_string()
    } else {
        format!("{gap} seconds")
    };
    let mut evidence = vec![
        format!("reflog: {}", switch.subject),
        format!("reflog: {} ({gap_text} after the switch)", commit.subject),
    ];

    let onto_long_lived = is_long_lived(to) && !is_long_lived(from);
    if !onto_long_lived {
        // Weak signal: timing alone. Usually normal, so it is plain context
        // (info), not a suspicion — kept for --verbose only.
        return Some(Diagnosis {
            id: "possible_wrong_branch_commit",
            title: "Quick branch switch before the last commit".to_string(),
            severity: Severity::Info,
            confidence: Confidence::Low,
            brief: format!("the last commit came {gap_text} after switching to {to}"),
            explanation: format!(
                "You switched from {from} to {to} and committed {gap_text} later. \
                 That is usually completely normal — it's noted here only as context, \
                 in case the commit surprised you by being on {to}."
            ),
            evidence,
            suggestions: vec![Suggestion::read_only(
                "confirm the commit is where you expect",
                "git log -1 --oneline",
            )],
        });
    }

    let sha = &commit.sha;
    if let (Some(upstream), Some((ahead, _))) = (&snap.upstream, snap.ahead_behind)
        && ahead > 0
    {
        evidence.push(format!(
            "{to} is now {ahead} {} ahead of {upstream}",
            plural(ahead as usize, "commit", "commits")
        ));
    }

    Some(Diagnosis {
        id: "possible_wrong_branch_commit",
        title: "Commit may be on the wrong branch".to_string(),
        severity: Severity::Suspicious,
        confidence: Confidence::Medium,
        brief: format!("a commit landed on {to} right after leaving {from}"),
        explanation: format!(
            "You left {from} and committed on {to} {gap_text} later. {to} looks like a \
             long-lived branch, and quick commits straight onto one are often accidental. \
             oops can't know your intent, so check it."
        ),
        evidence,
        suggestions: vec![
            Suggestion::read_only("check the newest commit", "git log -1 --oneline"),
            Suggestion::read_only(
                &format!("compare with {from}"),
                &format!("git log --oneline -3 {from}"),
            ),
            Suggestion::mutating(
                &format!("if it belongs on {from}, switch there"),
                &format!("git switch {from}"),
            ),
            Suggestion::mutating("carry the commit over", &format!("git cherry-pick {sha}")),
            Suggestion::mutating(
                "then undo it here, history-safe",
                &format!("git switch {to} && git revert {sha}"),
            ),
        ],
    })
}

fn staged_changes(snap: &RepositorySnapshot) -> Option<Diagnosis> {
    let staged = snap.staged();
    if staged.is_empty() {
        return None;
    }
    let n = staged.len();
    let deletions = staged.iter().filter(|e| e.staged == 'D').count();
    let mut evidence = vec![format!("{n} {} staged", plural(n, "path", "paths"))];
    if deletions > 0 {
        evidence.push(format!(
            "{deletions} of them {} staged deletion",
            plural(deletions, "is a", "are")
        ));
    }

    Some(Diagnosis {
        id: "staged_changes",
        title: "Staged but uncommitted changes".to_string(),
        severity: Severity::Info,
        confidence: Confidence::High,
        brief: format!(
            "{n} {} staged, waiting for a commit",
            plural(n, "path is", "paths are")
        ),
        explanation: format!(
            "{n} {} staged and waiting for a commit. Nothing wrong with that — \
             just don't lose track of it.",
            plural(n, "path is", "paths are")
        ),
        evidence,
        suggestions: vec![
            Suggestion::read_only("review the staged diff", "git diff --cached --stat"),
            Suggestion::mutating("commit it", "git commit"),
            Suggestion::mutating(
                "unstage but keep the changes",
                "git restore --staged <path>",
            ),
        ],
    })
}

fn dirty_working_tree(snap: &RepositorySnapshot) -> Option<Diagnosis> {
    let modified = snap.unstaged_modified();
    if modified.is_empty() {
        return None;
    }
    let n = modified.len();
    Some(Diagnosis {
        id: "dirty_working_tree",
        title: "Uncommitted changes in the working tree".to_string(),
        severity: Severity::Info,
        confidence: Confidence::High,
        brief: format!(
            "{n} {} uncommitted changes",
            plural(n, "file has", "files have")
        ),
        explanation: format!(
            "{n} tracked {} unstaged modifications. Often that's just work in progress — \
             oops can't tell intent from evidence here.",
            plural(n, "file has", "files have")
        ),
        evidence: vec![format!(
            "{n} modified {} reported by git status",
            plural(n, "path", "paths")
        )],
        suggestions: vec![
            Suggestion::read_only("see what changed", "git diff --stat"),
            Suggestion::mutating("stash the changes for later", "git stash push"),
        ],
    })
}

fn stash_entries(snap: &RepositorySnapshot) -> Option<Diagnosis> {
    if snap.stashes.is_empty() {
        return None;
    }
    let n = snap.stashes.len();
    let oldest = snap
        .stashes
        .iter()
        .map(|s| s.time)
        .min()
        .unwrap_or(snap.now);
    let age = snap.now.saturating_sub(oldest);
    let old = age > STASH_FORGOTTEN_SECS;

    let mut evidence: Vec<String> = snap
        .stashes
        .iter()
        .take(3)
        .map(|s| format!("{}: {}", s.name, s.subject))
        .collect();
    if n > 3 {
        evidence.push(format!("… and {} more", n - 3));
    }

    let brief = if n == 1 {
        format!("1 stash is still around, from {}", rough_age(age))
    } else {
        format!(
            "{n} stashes are still around — the oldest is from {}",
            rough_age(age)
        )
    };

    Some(Diagnosis {
        id: "stash_entries",
        title: "Stash entries exist".to_string(),
        severity: Severity::Info,
        confidence: Confidence::High,
        brief,
        explanation: format!(
            "You have {n} stash {}. The oldest is from {}{}",
            plural(n, "entry", "entries"),
            rough_age(age),
            if old {
                " — old stashes are easy to forget about."
            } else {
                "."
            }
        ),
        evidence,
        suggestions: vec![
            Suggestion::read_only("list the stashes", "git stash list"),
            Suggestion::read_only("peek at the newest", "git stash show -p 'stash@{0}'"),
            Suggestion::mutating(
                "re-apply the newest — keeps a backup",
                "git stash apply 'stash@{0}'",
            ),
        ],
    })
}

fn all_clear(snap: &RepositorySnapshot) -> Diagnosis {
    let mut evidence = vec![
        "no changes to tracked files".to_string(),
        "nothing staged".to_string(),
        "no merge, rebase, cherry-pick or revert in progress".to_string(),
        "no stash entries".to_string(),
    ];
    if snap.untracked > 0 {
        evidence.push(format!(
            "{} untracked {} present (not treated as a problem)",
            snap.untracked,
            plural(snap.untracked, "path", "paths")
        ));
    }

    let (confidence, explanation) = match (&snap.upstream, snap.ahead_behind, &snap.head) {
        (_, _, None) => (
            Confidence::Medium,
            "This repository has no commits yet, and nothing looks out of place.".to_string(),
        ),
        (Some(upstream), Some((0, 0)), _) => (
            Confidence::High,
            format!(
                "The working tree is clean and {} is in sync with {upstream}.",
                snap.branch.as_deref().unwrap_or("HEAD")
            ),
        ),
        (Some(upstream), _, _) => (
            Confidence::Medium,
            format!(
                "The working tree is clean, but {upstream} wasn't comparable \
                 (it may have been deleted on the remote), so sync wasn't verified."
            ),
        ),
        (None, _, _) => (
            Confidence::Medium,
            format!(
                "The working tree is clean. {} has no upstream configured, \
                 so oops couldn't check whether it's in sync with a remote.",
                snap.branch.as_deref().unwrap_or("HEAD")
            ),
        ),
    };

    Diagnosis {
        id: "all_clear",
        title: "No obvious problem detected".to_string(),
        severity: Severity::Info,
        confidence,
        brief: "nothing found".to_string(),
        explanation,
        evidence,
        suggestions: vec![Suggestion::read_only(
            "if something still feels wrong, the reflog sees everything",
            "git reflog -20",
        )],
    }
}

fn push_conflict_evidence(evidence: &mut Vec<String>, snap: &RepositorySnapshot) {
    let n = snap.conflicted.len();
    if n == 0 {
        return;
    }
    evidence.push(format!("{n} unmerged {}", plural(n, "path", "paths")));
    for path in snap.conflicted.iter().take(5) {
        evidence.push(format!("conflict: {path}"));
    }
    if n > 5 {
        evidence.push(format!("… and {} more", n - 5));
    }
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

fn rough_age(secs: u64) -> String {
    match secs {
        0..=59 => "moments ago".to_string(),
        60..=3599 => {
            let m = secs / 60;
            format!("{m} {} ago", plural(m as usize, "minute", "minutes"))
        }
        3600..=86399 => {
            let h = secs / 3600;
            format!("about {h} {} ago", plural(h as usize, "hour", "hours"))
        }
        _ => {
            let d = secs / 86400;
            format!("{d} {} ago", plural(d as usize, "day", "days"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::DefaultDisplay;
    use crate::git::snapshot::{LogEntry, StashEntry, StatusEntry};

    fn base_snapshot() -> RepositorySnapshot {
        RepositorySnapshot {
            branch: Some("main".to_string()),
            head: Some("abc1234".to_string()),
            now: 1_700_000_000,
            ..Default::default()
        }
    }

    fn ids(diagnoses: &[Diagnosis]) -> Vec<&'static str> {
        diagnoses.iter().map(|d| d.id).collect()
    }

    fn quick_switch_reflog(snap: &RepositorySnapshot, from: &str, to: &str) -> Vec<LogEntry> {
        vec![
            LogEntry {
                sha: "abc1234".to_string(),
                time: snap.now - 30,
                subject: "commit: quick fix".to_string(),
            },
            LogEntry {
                sha: "def5678".to_string(),
                time: snap.now - 60,
                subject: format!("checkout: moving from {from} to {to}"),
            },
        ]
    }

    #[test]
    fn clean_repo_without_upstream_is_all_clear_medium() {
        let diagnoses = diagnose(&base_snapshot());
        assert_eq!(ids(&diagnoses), vec!["all_clear"]);
        assert_eq!(diagnoses[0].confidence, Confidence::Medium);
        assert_eq!(diagnoses[0].severity, Severity::Info);
    }

    #[test]
    fn clean_synced_repo_is_all_clear_high() {
        let mut snap = base_snapshot();
        snap.upstream = Some("origin/main".to_string());
        snap.ahead_behind = Some((0, 0));
        let diagnoses = diagnose(&snap);
        assert_eq!(ids(&diagnoses), vec!["all_clear"]);
        assert_eq!(diagnoses[0].confidence, Confidence::High);
    }

    #[test]
    fn operations_in_progress_are_problems() {
        let mut snap = base_snapshot();
        snap.operation = Operation::Merge;
        let diagnoses = diagnose(&snap);
        assert_eq!(diagnoses[0].severity, Severity::Problem);
        assert_eq!(diagnoses[0].default_display(), DefaultDisplay::Full);
    }

    #[test]
    fn rebase_suppresses_detached_and_tree_noise() {
        let mut snap = base_snapshot();
        snap.operation = Operation::Rebase;
        snap.detached = true;
        snap.branch = None;
        snap.conflicted = vec!["a.rs".to_string()];
        snap.entries = vec![StatusEntry {
            staged: '.',
            worktree: 'M',
            path: "b.rs".to_string(),
        }];
        let diagnoses = diagnose(&snap);
        assert_eq!(ids(&diagnoses), vec!["rebase_in_progress"]);
    }

    #[test]
    fn diverged_is_a_problem_but_plain_ahead_and_behind_are_info() {
        let mut snap = base_snapshot();
        snap.upstream = Some("origin/main".to_string());

        snap.ahead_behind = Some((2, 3));
        let diagnoses = diagnose(&snap);
        assert_eq!(ids(&diagnoses), vec!["diverged_from_upstream"]);
        assert_eq!(diagnoses[0].severity, Severity::Problem);
        let commands: Vec<&str> = diagnoses[0]
            .suggestions
            .iter()
            .map(|s| s.command.as_str())
            .collect();
        assert!(!commands.iter().any(|c| c.contains("--force")));
        assert!(!commands.iter().any(|c| c.contains("reset --hard")));

        snap.ahead_behind = Some((2, 0));
        let diagnoses = diagnose(&snap);
        assert_eq!(diagnoses[0].id, "unpushed_commits");
        assert_eq!(diagnoses[0].severity, Severity::Info);
        assert_eq!(diagnoses[0].default_display(), DefaultDisplay::Note);

        snap.ahead_behind = Some((0, 3));
        let diagnoses = diagnose(&snap);
        assert_eq!(diagnoses[0].id, "behind_upstream");
        assert_eq!(diagnoses[0].severity, Severity::Info);
    }

    #[test]
    fn commit_onto_long_lived_branch_after_leaving_feature_is_suspicious() {
        let mut snap = base_snapshot();
        snap.reflog = quick_switch_reflog(&snap, "feat/x", "main");
        let diagnoses = diagnose(&snap);
        let d = diagnoses
            .iter()
            .find(|d| d.id == "possible_wrong_branch_commit")
            .unwrap();
        assert_eq!(d.severity, Severity::Suspicious);
        // Direction + timing gives medium, never high — intent stays unprovable.
        assert_eq!(d.confidence, Confidence::Medium);
        assert_eq!(d.default_display(), DefaultDisplay::Full);
    }

    #[test]
    fn commit_after_switching_to_feature_branch_is_only_info_context() {
        let mut snap = base_snapshot();
        snap.branch = Some("ci/fix-pipeline".to_string());
        snap.reflog = quick_switch_reflog(&snap, "main", "ci/fix-pipeline");
        let diagnoses = diagnose(&snap);
        let d = diagnoses
            .iter()
            .find(|d| d.id == "possible_wrong_branch_commit")
            .unwrap();
        // Usually-normal behavior must not be labeled suspicious.
        assert_eq!(d.severity, Severity::Info);
        assert_eq!(d.confidence, Confidence::Low);
        assert_eq!(
            d.default_display(),
            DefaultDisplay::Hidden,
            "must stay out of default output"
        );
        assert!(
            d.suggestions
                .iter()
                .all(|s| s.kind == crate::diagnosis::ActionKind::ReadOnly),
            "weak heuristic must be non-actionable"
        );
    }

    #[test]
    fn switching_between_long_lived_branches_is_only_info_context() {
        let mut snap = base_snapshot();
        snap.branch = Some("develop".to_string());
        snap.reflog = quick_switch_reflog(&snap, "main", "develop");
        let d = diagnose(&snap);
        let d = d
            .iter()
            .find(|d| d.id == "possible_wrong_branch_commit")
            .unwrap();
        assert_eq!(d.severity, Severity::Info);
        assert_eq!(d.confidence, Confidence::Low);
    }

    #[test]
    fn wrong_branch_stays_quiet_when_commit_is_slow_or_old() {
        let mut snap = base_snapshot();
        snap.reflog = vec![
            LogEntry {
                sha: "abc1234".to_string(),
                time: snap.now - 30,
                subject: "commit: quick fix".to_string(),
            },
            LogEntry {
                sha: "def5678".to_string(),
                time: snap.now - 30 - WRONG_BRANCH_WINDOW_SECS - 1,
                subject: "checkout: moving from feat/x to main".to_string(),
            },
        ];
        assert_eq!(ids(&diagnose(&snap)), vec!["all_clear"]);

        let mut old = base_snapshot();
        old.reflog = vec![
            LogEntry {
                sha: "abc1234".to_string(),
                time: old.now - WRONG_BRANCH_RECENCY_SECS - 100,
                subject: "commit: quick fix".to_string(),
            },
            LogEntry {
                sha: "def5678".to_string(),
                time: old.now - WRONG_BRANCH_RECENCY_SECS - 130,
                subject: "checkout: moving from feat/x to main".to_string(),
            },
        ];
        assert_eq!(ids(&diagnose(&old)), vec!["all_clear"]);
    }

    #[test]
    fn wrong_branch_ignores_amends_and_foreign_branches() {
        let mut snap = base_snapshot();
        snap.reflog = vec![
            LogEntry {
                sha: "abc1234".to_string(),
                time: snap.now - 30,
                subject: "commit (amend): oops".to_string(),
            },
            LogEntry {
                sha: "def5678".to_string(),
                time: snap.now - 40,
                subject: "checkout: moving from feat/x to main".to_string(),
            },
        ];
        assert_eq!(ids(&diagnose(&snap)), vec!["all_clear"]);
    }

    #[test]
    fn stashes_are_always_info_regardless_of_age() {
        let mut snap = base_snapshot();
        snap.stashes = vec![StashEntry {
            name: "stash@{0}".to_string(),
            time: snap.now - 7 * 86400,
            subject: "WIP on main: abc1234 x".to_string(),
        }];
        let diagnoses = diagnose(&snap);
        let d = diagnoses.iter().find(|d| d.id == "stash_entries").unwrap();
        assert_eq!(d.severity, Severity::Info);
        assert_eq!(d.default_display(), DefaultDisplay::Note);
        assert!(d.brief.contains("7 days ago"), "{}", d.brief);

        snap.stashes[0].time = snap.now - 60;
        let diagnoses = diagnose(&snap);
        let d = diagnoses.iter().find(|d| d.id == "stash_entries").unwrap();
        assert_eq!(d.severity, Severity::Info);
    }

    #[test]
    fn findings_are_ordered_problem_then_suspicious_then_info() {
        let mut snap = base_snapshot();
        snap.detached = true;
        snap.branch = None;
        snap.entries = vec![
            StatusEntry {
                staged: '.',
                worktree: 'D',
                path: "gone.rs".to_string(),
            },
            StatusEntry {
                staged: '.',
                worktree: 'M',
                path: "b.rs".to_string(),
            },
        ];
        snap.stashes = vec![StashEntry {
            name: "stash@{0}".to_string(),
            time: snap.now - 60,
            subject: "WIP".to_string(),
        }];
        let severities: Vec<Severity> = diagnose(&snap).iter().map(|d| d.severity).collect();
        let mut sorted = severities.clone();
        sorted.sort_by_key(|s| match s {
            Severity::Problem => 0,
            Severity::Suspicious => 1,
            Severity::Info => 2,
        });
        assert_eq!(severities, sorted);
        assert_eq!(severities[0], Severity::Problem);
    }

    #[test]
    fn every_mutating_suggestion_avoids_destructive_commands() {
        // Exercise a snapshot that fires many rules at once, then check
        // no suggestion ever contains a forbidden destructive command.
        let mut snap = base_snapshot();
        snap.upstream = Some("origin/main".to_string());
        snap.ahead_behind = Some((2, 3));
        snap.entries = vec![
            StatusEntry {
                staged: 'M',
                worktree: '.',
                path: "a.rs".to_string(),
            },
            StatusEntry {
                staged: '.',
                worktree: 'M',
                path: "b.rs".to_string(),
            },
            StatusEntry {
                staged: '.',
                worktree: 'D',
                path: "c.rs".to_string(),
            },
        ];
        snap.stashes = vec![StashEntry {
            name: "stash@{0}".to_string(),
            time: snap.now - 60,
            subject: "WIP".to_string(),
        }];
        for d in diagnose(&snap) {
            for s in d.suggestions {
                assert!(!s.command.contains("reset --hard"), "{}", s.command);
                assert!(!s.command.contains("clean -"), "{}", s.command);
                assert!(!s.command.contains("--force"), "{}", s.command);
                assert!(!s.command.contains("push -f"), "{}", s.command);
            }
        }
    }
}
