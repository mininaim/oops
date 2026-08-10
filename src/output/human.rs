use super::{Style, rel_time_short, wrap_text};
use crate::diagnosis::{ActionKind, DefaultDisplay, Diagnosis, Severity, Suggestion};
use crate::git::snapshot::{Operation, RepositorySnapshot};
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Default,
    Verbose,
    Explain,
}

pub fn render(
    snap: &RepositorySnapshot,
    diagnoses: &[Diagnosis],
    mode: Mode,
    style: &Style,
) -> String {
    let mut out = String::from("\n");

    let full: Vec<&Diagnosis> = diagnoses
        .iter()
        .filter(|d| d.default_display() == DefaultDisplay::Full)
        .collect();
    let notes: Vec<&Diagnosis> = diagnoses
        .iter()
        .filter(|d| d.default_display() == DefaultDisplay::Note && d.id != "all_clear")
        .collect();
    let hidden: Vec<&Diagnosis> = diagnoses
        .iter()
        .filter(|d| d.default_display() == DefaultDisplay::Hidden)
        .collect();

    // The first meaningful line is the finding itself. A headline only
    // exists when there is no finding to lead with.
    if full.is_empty() {
        let _ = writeln!(
            out,
            "  {} {}",
            style.ok(style.glyphs().check),
            style.bold("Nothing looks broken")
        );
        out.push('\n');
    }

    let shown: Vec<&Diagnosis> = if mode == Mode::Default {
        for diagnosis in &full {
            compact_block(&mut out, diagnosis, snap, style);
        }
        notes_block(&mut out, &notes, !full.is_empty(), style);
        if !hidden.is_empty() {
            let n = hidden.len();
            let _ = writeln!(
                out,
                "    {} {}  {}",
                style.accent(style.glyphs().arrow),
                style.accent("oops --verbose"),
                style.dim(&format!(
                    "· {n} low-confidence {} hidden",
                    if n == 1 {
                        "observation"
                    } else {
                        "observations"
                    }
                ))
            );
            out.push('\n');
        }
        full
    } else {
        // Findings lead in every mode; the repository dossier follows.
        // `diagnose` already orders findings problem → suspicious → info.
        // Major sections breathe: one extra blank line between findings,
        // the dossier, the activity timeline, and the closing lines.
        for diagnosis in diagnoses {
            expanded_block(&mut out, diagnosis, snap, style);
        }
        out.push('\n');
        repository_block(&mut out, snap, style);
        if mode == Mode::Explain {
            let _ = writeln!(
                out,
                "    {}",
                style.dim("oops checked: branch and upstream state, working tree, staged")
            );
            let _ = writeln!(
                out,
                "    {}",
                style.dim("changes, operation state, reflog, stashes.")
            );
            out.push('\n');
        }
        if !snap.reflog.is_empty() {
            out.push('\n');
            recent_activity_block(&mut out, snap, style);
        }
        diagnoses.iter().collect()
    };

    let any_mutating = shown
        .iter()
        .flat_map(|d| &d.suggestions)
        .any(|s| s.kind == ActionKind::Mutating);
    if any_mutating {
        if mode != Mode::Default {
            out.push('\n');
        }
        let legend = "review before running — \"changes state\" commands modify the repository";
        let mut first = true;
        for line in wrap_text(legend, style.width.saturating_sub(6)) {
            let marker = if first {
                style.warn(style.glyphs().note)
            } else {
                " ".to_string()
            };
            let _ = writeln!(out, "    {marker} {}", style.dim(&line));
            first = false;
        }
        out.push('\n');
    }

    // The oops signature: the branch line ends, and nothing moved.
    let _ = writeln!(
        out,
        "  {} {}",
        style.ok(style.glyphs().tail),
        style.dim("nothing was changed")
    );
    out
}

fn glyph_for(diagnosis: &Diagnosis, style: &Style) -> String {
    let glyphs = style.glyphs();
    match diagnosis.severity {
        Severity::Problem => style.danger(glyphs.problem),
        Severity::Suspicious => style.warn(glyphs.suspicious),
        Severity::Info if diagnosis.id == "all_clear" => style.ok(glyphs.check),
        Severity::Info => style.dim(glyphs.note),
    }
}

/// The few concrete paths worth seeing even in compact output.
fn highlights(diagnosis: &Diagnosis, snap: &RepositorySnapshot) -> Vec<String> {
    let paths: Vec<&str> = match diagnosis.id {
        "merge_in_progress"
        | "rebase_in_progress"
        | "cherry_pick_in_progress"
        | "revert_in_progress" => snap.conflicted.iter().map(String::as_str).collect(),
        "tracked_files_deleted" => snap
            .unstaged_deleted()
            .iter()
            .map(|e| e.path.as_str())
            .collect(),
        _ => Vec::new(),
    };
    let mut lines: Vec<String> = paths.iter().take(4).map(|p| p.to_string()).collect();
    if paths.len() > 4 {
        lines.push(format!("… and {} more", paths.len() - 4));
    }
    lines
}

/// Default view: what happened, the one-line consequence, the concrete
/// paths if any, and what to do next. Everything else lives in --verbose.
fn compact_block(
    out: &mut String,
    diagnosis: &Diagnosis,
    snap: &RepositorySnapshot,
    style: &Style,
) {
    let _ = writeln!(
        out,
        "  {} {}",
        glyph_for(diagnosis, style),
        style.bold(&diagnosis.title)
    );
    let _ = writeln!(out, "    {}", diagnosis.brief);
    out.push('\n');

    let highlighted = highlights(diagnosis, snap);
    if !highlighted.is_empty() {
        for line in highlighted {
            let _ = writeln!(out, "    {line}");
        }
        out.push('\n');
    }

    suggestions_block(out, &diagnosis.suggestions, style);
}

/// Verbose view: the same block, expanded with confidence, the full
/// explanation and the evidence trail.
fn expanded_block(
    out: &mut String,
    diagnosis: &Diagnosis,
    snap: &RepositorySnapshot,
    style: &Style,
) {
    let meta = format!(
        "{} · {} confidence",
        diagnosis.severity, diagnosis.confidence
    );
    let _ = writeln!(
        out,
        "  {} {}  {}",
        glyph_for(diagnosis, style),
        style.bold(&diagnosis.title),
        style.dim(&meta)
    );
    let wrap_width = style.width.min(84).saturating_sub(4);
    for line in wrap_text(&diagnosis.explanation, wrap_width) {
        let _ = writeln!(out, "    {line}");
    }
    out.push('\n');

    if diagnosis.id == "stash_entries" && !snap.stashes.is_empty() {
        stash_evidence_block(out, snap, style);
    } else if !diagnosis.evidence.is_empty() {
        let _ = writeln!(out, "    {}", style.dim("evidence"));
        for item in &diagnosis.evidence {
            let _ = writeln!(
                out,
                "    {} {}",
                style.dim(style.glyphs().note),
                prettify_moves(item, style)
            );
        }
        out.push('\n');
    }

    suggestions_block(out, &diagnosis.suggestions, style);
}

/// Stash evidence as an aligned, scannable table instead of prose:
/// name column, age column, then the stash's own subject.
fn stash_evidence_block(out: &mut String, snap: &RepositorySnapshot, style: &Style) {
    let _ = writeln!(out, "    {}", style.dim("evidence"));
    let name_width = snap
        .stashes
        .iter()
        .take(5)
        .map(|s| s.name.len())
        .max()
        .unwrap_or(0);
    for stash in snap.stashes.iter().take(5) {
        let _ = writeln!(
            out,
            "    {} {:<name_width$}  {}  {}",
            style.dim(style.glyphs().note),
            stash.name,
            style.dim(&format!("{:>4}", rel_time_short(snap.now, stash.time))),
            stash.subject
        );
    }
    if snap.stashes.len() > 5 {
        let _ = writeln!(
            out,
            "    {}",
            style.dim(&format!("… and {} more", snap.stashes.len() - 5))
        );
    }
    out.push('\n');
}

/// Presentation-only rewrite of reflog checkout prose:
/// `checkout: moving from A to B` becomes `A → B`. The original text
/// stays untouched in the diagnosis model and in --json.
fn prettify_moves(text: &str, style: &Style) -> String {
    const PREFIX: &str = "checkout: moving from ";
    if let Some(idx) = text.find(PREFIX)
        && let Some((from, to)) = text[idx + PREFIX.len()..].split_once(" to ")
    {
        return format!("{}{from} {} {to}", &text[..idx], style.glyphs().flow);
    }
    text.to_string()
}

/// Commands are the strongest visual objects on screen: an accent arrow,
/// the command itself in accent, the terse purpose in slate, and a
/// compact amber tag on anything that would change the repository.
/// The command always leads; a purpose that doesn't fit the terminal
/// width drops to a dim sub-line instead of pushing the command around.
fn suggestions_block(out: &mut String, suggestions: &[Suggestion], style: &Style) {
    if suggestions.is_empty() {
        return;
    }
    let _ = writeln!(out, "    {}", style.dim("next"));
    let tag_width = |s: &Suggestion| {
        if s.kind == ActionKind::Mutating {
            16
        } else {
            0
        }
    };

    // Choose the command column that lets the most suggestions sit on one
    // aligned line; anything that doesn't fit (oversized command or long
    // purpose) stacks on its own without dragging the others down.
    let fits_at = |s: &Suggestion, pad: usize| {
        s.command.len() <= pad && 6 + pad + 4 + s.description.len() + tag_width(s) <= style.width
    };
    let mut pads: Vec<usize> = suggestions.iter().map(|s| s.command.len()).collect();
    pads.sort_unstable();
    pads.dedup();
    let minimum_members = if suggestions.len() == 1 { 1 } else { 2 };
    let column = pads
        .iter()
        .rev()
        .map(|&pad| (pad, suggestions.iter().filter(|s| fits_at(s, pad)).count()))
        .filter(|&(_, count)| count >= minimum_members)
        .max_by_key(|&(_, count)| count)
        .map(|(pad, _)| pad);

    for suggestion in suggestions {
        let tag = match suggestion.kind {
            ActionKind::ReadOnly => String::new(),
            ActionKind::Mutating => format!(" {} {}", style.dim("·"), style.warn("changes state")),
        };
        match column {
            Some(pad) if fits_at(suggestion, pad) => {
                let padded = format!("{:<pad$}", suggestion.command);
                let _ = writeln!(
                    out,
                    "    {} {}    {}{tag}",
                    style.accent(style.glyphs().arrow),
                    style.accent(&padded),
                    style.dim(&suggestion.description)
                );
            }
            _ => {
                // The description introduces the command as prose (with a
                // trailing colon) so it can never be mistaken for something
                // executable; only the command line carries the marker.
                let _ = writeln!(
                    out,
                    "    {}",
                    style.dim(&format!("{}:", suggestion.description))
                );
                let _ = writeln!(
                    out,
                    "    {} {}{tag}",
                    style.accent(style.glyphs().arrow),
                    style.accent(&suggestion.command)
                );
            }
        }
    }
    out.push('\n');
}

/// Info notes. Nested quietly under the healthy check when nothing else is
/// on screen; labeled when they follow real findings.
fn notes_block(out: &mut String, notes: &[&Diagnosis], labeled: bool, style: &Style) {
    if notes.is_empty() {
        return;
    }
    if labeled {
        let _ = writeln!(out, "    {}", style.dim("worth knowing"));
    }
    for diagnosis in notes {
        let _ = writeln!(
            out,
            "    {} {}",
            style.dim(style.glyphs().note),
            diagnosis.brief
        );
    }
    out.push('\n');
}

fn repository_block(out: &mut String, snap: &RepositorySnapshot, style: &Style) {
    let _ = writeln!(out, "    {}", style.dim("repository"));
    let key = |name: &str| style.dim(&format!("{name:<9}"));

    let branch = match (&snap.branch, snap.detached) {
        (Some(branch), _) => branch.clone(),
        (None, true) => "(detached)".to_string(),
        (None, false) => "(unknown)".to_string(),
    };
    let _ = writeln!(out, "    {} {branch}", key("branch"));

    if let Some(head) = &snap.head {
        let subject = snap.head_subject().unwrap_or("");
        let _ = writeln!(out, "    {} {head}  {subject}", key("head"));
    } else {
        let _ = writeln!(out, "    {} no commits yet", key("head"));
    }

    if let Some(upstream) = &snap.upstream {
        let sync = match snap.ahead_behind {
            Some((0, 0)) => "in sync".to_string(),
            Some((a, b)) => format!("ahead {a} · behind {b}"),
            None => "not comparable".to_string(),
        };
        let _ = writeln!(out, "    {} {upstream} · {sync}", key("upstream"));
    } else {
        let _ = writeln!(out, "    {} none", key("upstream"));
    }

    let mut tree = Vec::new();
    let modified = snap.unstaged_modified().len();
    let deleted = snap.unstaged_deleted().len();
    let staged = snap.staged().len();
    if modified > 0 {
        tree.push(format!("{modified} modified"));
    }
    if deleted > 0 {
        tree.push(format!("{deleted} deleted"));
    }
    if staged > 0 {
        tree.push(format!("{staged} staged"));
    }
    if !snap.conflicted.is_empty() {
        tree.push(format!("{} conflicted", snap.conflicted.len()));
    }
    if snap.untracked > 0 {
        tree.push(format!("{} untracked", snap.untracked));
    }
    let tree = if tree.is_empty() {
        "clean".to_string()
    } else {
        tree.join(" · ")
    };
    let _ = writeln!(out, "    {} {tree}", key("worktree"));

    if !snap.stashes.is_empty() {
        let _ = writeln!(
            out,
            "    {} {} entr{}",
            key("stash"),
            snap.stashes.len(),
            if snap.stashes.len() == 1 { "y" } else { "ies" }
        );
    }
    if snap.operation != Operation::None {
        let _ = writeln!(
            out,
            "    {} {} in progress",
            key("operation"),
            snap.operation.as_str()
        );
    }
    out.push('\n');
}

/// The reflog as one truthful timeline, newest first: a single column of
/// stops — solid for commits, hollow for every other movement of HEAD.
/// Chronology only; no topology is implied.
fn recent_activity_block(out: &mut String, snap: &RepositorySnapshot, style: &Style) {
    if snap.reflog.is_empty() {
        return;
    }
    let glyphs = style.glyphs();
    let _ = writeln!(
        out,
        "    {} {}",
        style.dim("recent activity"),
        style.dim("· newest first")
    );
    for entry in snap.reflog.iter().take(5) {
        let stop = if entry.subject.starts_with("commit") {
            glyphs.commit.to_string()
        } else {
            style.dim(glyphs.movement)
        };
        let mut subject = prettify_moves(&entry.subject, style);
        if subject.chars().count() > 58 {
            subject = subject.chars().take(57).collect();
            subject.push('…');
        }
        let _ = writeln!(
            out,
            "    {stop} {}  {subject}",
            style.dim(&format!("{:>4}", rel_time_short(snap.now, entry.time)))
        );
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::Confidence;

    fn diagnosis(id: &'static str, severity: Severity, confidence: Confidence) -> Diagnosis {
        Diagnosis {
            id,
            title: format!("Title for {id}"),
            severity,
            confidence,
            brief: format!("brief for {id}"),
            explanation: format!("Explanation for {id}."),
            evidence: vec!["some evidence".to_string()],
            suggestions: vec![Suggestion::read_only("look around", "git status")],
        }
    }

    fn render_default(diagnoses: &[Diagnosis]) -> String {
        render(
            &RepositorySnapshot::default(),
            diagnoses,
            Mode::Default,
            &Style::plain(),
        )
    }

    #[test]
    fn checkout_reflog_prose_becomes_an_arrow() {
        let style = Style::plain();
        assert_eq!(
            prettify_moves("checkout: moving from main to feat/x", &style),
            "main → feat/x"
        );
        assert_eq!(
            prettify_moves("reflog: checkout: moving from a to b", &style),
            "reflog: a → b"
        );
        // Anything else stays untouched — no invented topology.
        assert_eq!(
            prettify_moves("rebase (start): checkout main", &style),
            "rebase (start): checkout main"
        );
        assert_eq!(
            prettify_moves("commit: add to cart", &style),
            "commit: add to cart"
        );
    }

    #[test]
    fn stash_evidence_renders_as_aligned_columns_in_verbose() {
        use crate::git::snapshot::StashEntry;
        let mut snap = RepositorySnapshot::default();
        snap.now = 1_700_000_000;
        snap.stashes = vec![
            StashEntry {
                name: "stash@{0}".to_string(),
                time: snap.now - 7 * 86400,
                subject: "On main: wip from 7 days ago".to_string(),
            },
            StashEntry {
                name: "stash@{1}".to_string(),
                time: snap.now - 9 * 86400,
                subject: "On main: wip from 9 days ago".to_string(),
            },
        ];
        let d = diagnosis("stash_entries", Severity::Info, Confidence::High);
        let text = render(&snap, &[d], Mode::Verbose, &Style::plain());
        assert!(
            text.contains("stash@{0}    7d  On main: wip from 7 days ago"),
            "{text}"
        );
        assert!(
            text.contains("stash@{1}    9d  On main: wip from 9 days ago"),
            "{text}"
        );
        assert!(
            !text.contains("some evidence"),
            "prose evidence is replaced by the aligned table: {text}"
        );
    }

    #[test]
    fn stacked_descriptions_never_look_like_commands() {
        let mut narrow = Style::plain();
        narrow.width = 40;
        let mut d = diagnosis("stashes", Severity::Problem, Confidence::High);
        d.suggestions = vec![Suggestion::read_only(
            "list the stashes and their ages",
            "git stash list",
        )];
        let text = render(&RepositorySnapshot::default(), &[d], Mode::Default, &narrow);
        let desc_line = text
            .lines()
            .find(|l| l.contains("list the stashes"))
            .unwrap();
        assert!(
            !desc_line.contains('›') && !desc_line.contains("git"),
            "descriptions carry no command marker: {desc_line}"
        );
        assert!(
            desc_line.trim_end().ends_with(':'),
            "stacked descriptions read as prose introductions: {desc_line}"
        );
        let cmd_line = text.lines().find(|l| l.contains("git stash list")).unwrap();
        assert!(cmd_line.contains('›'), "{cmd_line}");
    }

    #[test]
    fn activity_timeline_is_a_single_column_with_arrows() {
        use crate::git::snapshot::LogEntry;
        let mut snap = RepositorySnapshot::default();
        snap.now = 1_700_000_000;
        snap.reflog = vec![
            LogEntry {
                sha: "abc".to_string(),
                time: snap.now - 60,
                subject: "commit: fix totals".to_string(),
            },
            LogEntry {
                sha: "def".to_string(),
                time: snap.now - 120,
                subject: "checkout: moving from main to feat/x".to_string(),
            },
        ];
        let d = diagnosis("note", Severity::Info, Confidence::High);
        let text = render(&snap, &[d], Mode::Verbose, &Style::plain());
        assert!(text.contains("● "), "{text}");
        assert!(text.contains("○ "), "{text}");
        assert!(text.contains("main → feat/x"), "{text}");
        assert!(
            !text.contains("checkout: moving from"),
            "checkout prose is rendered as an arrow: {text}"
        );
        let activity_zone = text.split("recent activity").nth(1).unwrap();
        let before_signature = activity_zone.split('╰').next().unwrap();
        assert!(
            !before_signature.contains("│\n"),
            "no interleaved rail lines inside the timeline: {before_signature}"
        );
    }

    #[test]
    fn problems_lead_without_a_generic_headline() {
        let diagnoses = vec![diagnosis("broken", Severity::Problem, Confidence::High)];
        let text = render_default(&diagnoses);
        assert!(!text.contains("Oops found"), "{text}");
        assert!(!text.contains("Nothing looks broken"), "{text}");
        let first = text.lines().find(|l| !l.trim().is_empty()).unwrap();
        assert!(first.contains("● Title for broken"), "{first}");
    }

    #[test]
    fn healthy_state_keeps_its_check_headline() {
        let info_only = vec![diagnosis("c", Severity::Info, Confidence::High)];
        let text = render_default(&info_only);
        assert!(text.contains("✓ Nothing looks broken"), "{text}");
        assert!(text.contains("· brief for c"), "{text}");
    }

    #[test]
    fn confidence_stays_out_of_default_output() {
        let diagnoses = vec![diagnosis("broken", Severity::Problem, Confidence::High)];
        let text = render_default(&diagnoses);
        assert!(!text.contains("confidence"), "{text}");

        let verbose = render(
            &RepositorySnapshot::default(),
            &diagnoses,
            Mode::Verbose,
            &Style::plain(),
        );
        assert!(verbose.contains("problem · high confidence"), "{verbose}");
    }

    #[test]
    fn default_hides_explanation_and_evidence() {
        let diagnoses = vec![diagnosis("broken", Severity::Problem, Confidence::High)];
        let text = render_default(&diagnoses);
        assert!(text.contains("brief for broken"), "{text}");
        assert!(!text.contains("Explanation for broken"), "{text}");
        assert!(!text.contains("some evidence"), "{text}");
    }

    #[test]
    fn notes_are_labeled_only_after_real_findings() {
        let with_problem = vec![
            diagnosis("broken", Severity::Problem, Confidence::High),
            diagnosis("note", Severity::Info, Confidence::High),
        ];
        assert!(render_default(&with_problem).contains("worth knowing"));

        let notes_only = vec![diagnosis("note", Severity::Info, Confidence::High)];
        let text = render_default(&notes_only);
        assert!(!text.contains("worth knowing"), "{text}");
        assert!(text.contains("· brief for note"), "{text}");
    }

    #[test]
    fn low_confidence_findings_hide_behind_a_verbose_hint() {
        let diagnoses = vec![diagnosis("weak", Severity::Suspicious, Confidence::Low)];
        let text = render_default(&diagnoses);
        assert!(!text.contains("Title for weak"), "{text}");
        assert!(text.contains("low-confidence observation hidden"), "{text}");
        assert!(text.contains("oops --verbose"), "{text}");

        let verbose = render(
            &RepositorySnapshot::default(),
            &diagnoses,
            Mode::Verbose,
            &Style::plain(),
        );
        assert!(verbose.contains("Title for weak"), "{verbose}");
        assert!(verbose.contains("suspicious · low confidence"), "{verbose}");
    }

    #[test]
    fn low_confidence_info_is_shown_in_verbose_as_info() {
        let diagnoses = vec![diagnosis("weak_note", Severity::Info, Confidence::Low)];
        let text = render_default(&diagnoses);
        assert!(!text.contains("Title for weak_note"), "{text}");

        let verbose = render(
            &RepositorySnapshot::default(),
            &diagnoses,
            Mode::Verbose,
            &Style::plain(),
        );
        assert!(verbose.contains("info · low confidence"), "{verbose}");
    }

    #[test]
    fn severity_is_readable_without_color() {
        let diagnoses = vec![
            diagnosis("broken", Severity::Problem, Confidence::High),
            diagnosis("odd", Severity::Suspicious, Confidence::Medium),
        ];
        let text = render_default(&diagnoses);
        assert!(!text.contains('\u{1b}'), "plain style must emit no ANSI");
        assert!(text.contains("● Title for broken"), "{text}");
        assert!(text.contains("○ Title for odd"), "{text}");
        assert!(text.contains("╰╴ nothing was changed"), "{text}");
    }

    #[test]
    fn read_only_and_mutating_commands_are_distinguished_without_color() {
        let mut d = diagnosis("merge", Severity::Problem, Confidence::High);
        d.suggestions
            .push(Suggestion::mutating("abort it", "git merge --abort"));
        let text = render_default(&[d]);
        let abort_line = text.lines().find(|l| l.contains("--abort")).unwrap();
        assert!(abort_line.contains("changes state"), "{abort_line}");
        let status_line = text.lines().find(|l| l.contains("git status")).unwrap();
        assert!(!status_line.contains("changes state"), "{status_line}");
        assert!(text.contains("review before running"), "{text}");
    }

    #[test]
    fn mutating_legend_only_appears_when_a_shown_block_has_one() {
        let mut info = diagnosis("stash_entries", Severity::Info, Confidence::High);
        info.suggestions
            .push(Suggestion::mutating("apply it", "git stash apply"));
        let text = render_default(std::slice::from_ref(&info));
        assert!(
            !text.contains("review before running"),
            "info notes show no commands, so no legend: {text}"
        );

        let mut problem = diagnosis("merge", Severity::Problem, Confidence::High);
        problem
            .suggestions
            .push(Suggestion::mutating("abort it", "git merge --abort"));
        let text = render_default(&[problem]);
        assert!(text.contains("review before running"), "{text}");
    }

    #[test]
    fn one_oversized_command_does_not_stack_the_whole_block() {
        let mut style = Style::plain();
        style.width = 80;
        let mut d = diagnosis("detached", Severity::Problem, Confidence::High);
        d.suggestions = vec![
            Suggestion::read_only("see where you are", "git log --oneline -5"),
            Suggestion::mutating("go back to the previous branch", "git switch -"),
            Suggestion::mutating(
                "or keep this work on a real branch",
                "git switch -c <new-branch-name>",
            ),
        ];
        let text = render(&RepositorySnapshot::default(), &[d], Mode::Default, &style);
        let switch_line = text
            .lines()
            .find(|l| l.contains("git switch -") && !l.contains("-c"))
            .unwrap();
        assert!(
            switch_line.contains("go back to the previous branch"),
            "short commands keep their aligned one-line form: {switch_line}"
        );
        let long_line = text.lines().find(|l| l.contains("-c")).unwrap();
        assert!(
            !long_line.contains("or keep"),
            "the oversized command stacks alone: {long_line}"
        );
    }

    #[test]
    fn narrow_terminals_stack_command_descriptions() {
        let mut narrow = Style::plain();
        narrow.width = 40;
        let mut d = diagnosis("merge", Severity::Problem, Confidence::High);
        d.suggestions.push(Suggestion::mutating(
            "abandon and restore the pre-merge state",
            "git merge --abort",
        ));
        let text = render(&RepositorySnapshot::default(), &[d], Mode::Default, &narrow);
        let command_line = text.lines().find(|l| l.contains("--abort")).unwrap();
        assert!(
            !command_line.contains("abandon"),
            "description moves to its own line when width is tight: {command_line}"
        );
    }
}
