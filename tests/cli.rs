use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

/// A throwaway git repository for exercising the compiled `oops` binary.
struct Repo {
    _tmp: TempDir,
    root: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let repo = Repo { _tmp: tmp, root };
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    fn with_commit() -> Self {
        let repo = Repo::new();
        repo.write("README.md", "hello\nworld\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "initial commit"]);
        repo
    }

    /// Adds a bare `origin` inside the temp dir and pushes main to it.
    fn with_origin(&self) {
        let origin = self._tmp.path().join("origin.git");
        run_git(
            self._tmp.path(),
            &["init", "--bare", origin.to_str().unwrap()],
        )
        .unwrap();
        self.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        self.git(&["push", "-u", "origin", "main"]);
    }

    fn write(&self, name: &str, content: &str) {
        fs::write(self.root.join(name), content).unwrap();
    }

    fn git(&self, args: &[&str]) {
        run_git(&self.root, args).unwrap_or_else(|e| panic!("git {args:?} failed: {e}"));
    }

    /// Runs git with a committer date pinned to `epoch`, for aging stashes.
    fn git_at(&self, epoch: u64, args: &[&str]) {
        let date = format!("@{epoch} +0000");
        let out = git_command(&self.root, args)
            .env("GIT_COMMITTER_DATE", &date)
            .env("GIT_AUTHOR_DATE", &date)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Runs git expecting a non-zero exit (conflicting merges etc.).
    fn git_expect_failure(&self, args: &[&str]) {
        let out = git_command(&self.root, args).output().unwrap();
        assert!(!out.status.success(), "expected `git {args:?}` to fail");
    }

    fn oops(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_oops"))
            .args(args)
            .current_dir(&self.root)
            .env("NO_COLOR", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap()
    }

    fn stdout(&self, args: &[&str]) -> String {
        let out = self.oops(args);
        assert!(
            out.status.success(),
            "oops {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout(&["--json"])).expect("oops --json must emit valid JSON")
    }

    fn diagnosis_ids(&self) -> Vec<String> {
        self.json()["diagnoses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap().to_string())
            .collect()
    }
}

fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    cmd
}

fn run_git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = git_command(dir, args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[test]
fn clean_repository_reports_all_clear() {
    let repo = Repo::with_commit();
    let text = repo.stdout(&[]);
    assert!(text.contains("✓ Nothing looks broken"), "{text}");
    assert!(text.contains("╰╴ nothing was changed"), "{text}");
    assert_eq!(repo.diagnosis_ids(), vec!["all_clear"]);
}

#[test]
fn dirty_working_tree_is_an_info_note() {
    let repo = Repo::with_commit();
    repo.write("README.md", "hello\nchanged\n");
    let text = repo.stdout(&[]);
    assert!(text.contains("Nothing looks broken"), "{text}");
    assert!(text.contains("uncommitted changes"), "{text}");
    assert!(
        repo.diagnosis_ids()
            .contains(&"dirty_working_tree".to_string())
    );
}

#[test]
fn staged_changes_are_an_info_note() {
    let repo = Repo::with_commit();
    repo.write("README.md", "hello\nstaged\n");
    repo.git(&["add", "README.md"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("Nothing looks broken"), "{text}");
    assert!(text.contains("staged, waiting for a commit"), "{text}");
    assert!(repo.diagnosis_ids().contains(&"staged_changes".to_string()));
}

#[test]
fn deleted_tracked_file_is_detected() {
    let repo = Repo::with_commit();
    fs::remove_file(repo.root.join("README.md")).unwrap();
    let text = repo.stdout(&[]);
    assert!(text.contains("Tracked files deleted"), "{text}");
    assert!(text.contains("README.md"), "{text}");
    assert!(
        repo.diagnosis_ids()
            .contains(&"tracked_files_deleted".to_string())
    );
}

#[test]
fn detached_head_is_detected() {
    let repo = Repo::with_commit();
    repo.git(&["checkout", "--detach"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("Detached HEAD"), "{text}");
    assert!(repo.diagnosis_ids().contains(&"detached_head".to_string()));
}

#[test]
fn ahead_of_upstream_is_an_info_note() {
    let repo = Repo::with_commit();
    repo.with_origin();
    repo.write("new.txt", "x\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "local only"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("Nothing looks broken"), "{text}");
    assert!(text.contains("not pushed yet"), "{text}");
    assert!(
        repo.diagnosis_ids()
            .contains(&"unpushed_commits".to_string())
    );
}

#[test]
fn behind_upstream_is_an_info_note() {
    let repo = Repo::with_commit();
    repo.write("second.txt", "x\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "second"]);
    repo.with_origin();
    repo.git(&["reset", "--hard", "HEAD~1"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("Nothing looks broken"), "{text}");
    assert!(text.contains("behind origin/main"), "{text}");
    assert!(
        repo.diagnosis_ids()
            .contains(&"behind_upstream".to_string())
    );
}

#[test]
fn diverged_branch_is_detected() {
    let repo = Repo::with_commit();
    repo.write("second.txt", "x\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "second"]);
    repo.with_origin();
    repo.git(&["reset", "--hard", "HEAD~1"]);
    repo.write("other.txt", "y\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "diverging"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("diverged"), "{text}");
    let ids = repo.diagnosis_ids();
    assert!(
        ids.contains(&"diverged_from_upstream".to_string()),
        "{ids:?}"
    );
    assert!(
        !text.contains("--force"),
        "must never suggest force push: {text}"
    );
}

fn conflicting_branches(repo: &Repo) {
    repo.git(&["switch", "-c", "feature"]);
    repo.write("README.md", "hello\nfeature version\n");
    repo.git(&["commit", "-am", "feature change"]);
    repo.git(&["switch", "main"]);
    repo.write("README.md", "hello\nmain version\n");
    repo.git(&["commit", "-am", "main change"]);
}

#[test]
fn merge_conflict_is_detected() {
    let repo = Repo::with_commit();
    conflicting_branches(&repo);
    repo.git_expect_failure(&["merge", "feature"]);
    let text = repo.stdout(&[]);
    // Piped output carries no color, so the glyphs and annotation text
    // must communicate severity and mutation on their own.
    assert!(text.contains("● Merge in progress"), "{text}");
    assert!(text.contains("needs attention"), "{text}");
    assert!(text.contains("git merge --abort"), "{text}");
    assert!(text.contains("changes state"), "{text}");
    assert!(!text.contains('\u{1b}'), "{text}");
    let status_line = text.lines().find(|l| l.contains("git status")).unwrap();
    assert!(
        !status_line.contains("changes state"),
        "read-only commands carry no mutation tag: {status_line}"
    );
    assert!(
        repo.diagnosis_ids()
            .contains(&"merge_in_progress".to_string())
    );
}

#[test]
fn rebase_conflict_is_detected() {
    let repo = Repo::with_commit();
    conflicting_branches(&repo);
    repo.git(&["switch", "feature"]);
    repo.git_expect_failure(&["rebase", "main"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("Rebase in progress"), "{text}");
    assert!(text.contains("feature"), "{text}");
    assert!(text.contains("git rebase --abort"), "{text}");
    let ids = repo.diagnosis_ids();
    assert_eq!(
        ids,
        vec!["rebase_in_progress"],
        "rebase should suppress tree noise"
    );
}

#[test]
fn cherry_pick_conflict_is_detected() {
    let repo = Repo::with_commit();
    conflicting_branches(&repo);
    repo.git_expect_failure(&["cherry-pick", "feature"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("Cherry-pick in progress"), "{text}");
    assert!(
        repo.diagnosis_ids()
            .contains(&"cherry_pick_in_progress".to_string())
    );
}

#[test]
fn revert_conflict_is_detected() {
    let repo = Repo::with_commit();
    repo.write("README.md", "hello\nsecond version\n");
    repo.git(&["commit", "-am", "second"]);
    repo.write("README.md", "hello\nthird version\n");
    repo.git(&["commit", "-am", "third"]);
    repo.git_expect_failure(&["revert", "--no-edit", "HEAD~1"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("Revert in progress"), "{text}");
    assert!(
        repo.diagnosis_ids()
            .contains(&"revert_in_progress".to_string())
    );
}

#[test]
fn stash_entry_is_an_info_note_never_a_problem() {
    let repo = Repo::with_commit();
    repo.write("README.md", "hello\nstash me\n");
    repo.git(&["stash", "push", "-m", "wip thing"]);
    let text = repo.stdout(&[]);
    assert!(text.contains("Nothing looks broken"), "{text}");
    assert!(text.contains("stash"), "{text}");

    let value = repo.json();
    let stash = value["diagnoses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "stash_entries")
        .expect("stash diagnosis present in JSON");
    assert_eq!(stash["severity"], "info");
}

/// The defensible wrong-branch shape: leaving a feature branch and
/// immediately committing on a long-lived branch is surfaced by default.
#[test]
fn commit_on_main_right_after_leaving_feature_branch_is_flagged() {
    let repo = Repo::with_commit();
    repo.git(&["branch", "feature"]);
    repo.git(&["switch", "feature"]);
    repo.git(&["switch", "main"]);
    repo.write("hotfix.txt", "x\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "quick fix"]);
    let text = repo.stdout(&[]);
    assert!(
        text.contains("○ Commit may be on the wrong branch"),
        "{text}"
    );
    assert!(
        !text.contains("Nothing looks broken"),
        "a surfaced suspicion must not sit under a healthy headline: {text}"
    );

    let value = repo.json();
    let finding = value["diagnoses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "possible_wrong_branch_commit")
        .expect("wrong-branch diagnosis present in JSON");
    assert_eq!(finding["severity"], "suspicious");
    assert_eq!(finding["confidence"], "medium");
}

/// Regression for the dogfooded false positive: switching from main to a
/// feature/CI branch and committing shortly afterward is normal usage, and
/// several old stashes are hygiene info — plain `oops` must stay calm.
#[test]
fn normal_feature_branch_commit_with_old_stashes_stays_calm() {
    let repo = Repo::with_commit();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for days in [9u64, 8, 7] {
        repo.write("README.md", &format!("hello\nwip {days}\n"));
        repo.git_at(
            now - days * 86_400,
            &["stash", "push", "-m", &format!("wip {days} days ago")],
        );
    }

    repo.git(&["switch", "-c", "ci/fix-pipeline"]);
    repo.write("ci.yml", "pipeline: fixed\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "ci: fix pipeline"]);

    let text = repo.stdout(&[]);
    assert!(text.contains("Nothing looks broken"), "{text}");
    assert!(
        !text.contains("wrong branch"),
        "normal switch-then-commit must not be flagged by default: {text}"
    );
    assert!(text.contains("3 stashes"), "{text}");
    assert!(text.contains("days ago"), "{text}");
    assert!(text.contains("oops --verbose"), "{text}");

    // The weak timing signal is still available through --verbose, labeled
    // as plain low-confidence info (its explanation says this is usually
    // normal, so calling it "suspicious" would be misleading) — and it must
    // not suggest any mutating command.
    let verbose = repo.stdout(&["--verbose"]);
    assert!(
        verbose.contains("Quick branch switch before the last commit"),
        "{verbose}"
    );
    assert!(verbose.contains("info · low confidence"), "{verbose}");
    assert!(
        !verbose.contains("suspicious · low confidence"),
        "{verbose}"
    );
    assert!(verbose.contains("usually completely normal"), "{verbose}");

    let value = repo.json();
    let weak = value["diagnoses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "possible_wrong_branch_commit")
        .expect("weak signal still present in JSON");
    assert_eq!(weak["severity"], "info");
    assert_eq!(weak["confidence"], "low");
    assert_eq!(weak["shown_by_default"], false);
    for suggestion in weak["suggestions"].as_array().unwrap() {
        assert_eq!(
            suggestion["mutates_repository"], false,
            "weak heuristic must be non-actionable"
        );
    }
}

#[test]
fn json_output_is_valid_and_stable() {
    let repo = Repo::with_commit();
    repo.write("README.md", "hello\nchanged\n");
    repo.git(&["stash", "push"]);
    let value = repo.json();

    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["changed_by_oops"], false);
    assert_eq!(value["repository"]["branch"], "main");
    assert_eq!(value["repository"]["detached"], false);
    assert_eq!(value["repository"]["operation"], "none");
    assert_eq!(value["repository"]["stash_count"], 1);
    assert!(value["repository"]["working_tree"]["staged"].is_number());
    assert!(value["diagnoses"].as_array().is_some_and(|d| !d.is_empty()));
    for diagnosis in value["diagnoses"].as_array().unwrap() {
        assert!(diagnosis["id"].is_string());
        assert!(matches!(
            diagnosis["severity"].as_str().unwrap(),
            "problem" | "suspicious" | "info"
        ));
        assert!(matches!(
            diagnosis["confidence"].as_str().unwrap(),
            "high" | "medium" | "low"
        ));
        assert!(diagnosis["summary"].is_string());
        assert!(diagnosis["shown_by_default"].is_boolean());
        for suggestion in diagnosis["suggestions"].as_array().unwrap() {
            let mutates = suggestion["mutates_repository"].as_bool().unwrap();
            assert_eq!(suggestion["warning"].is_string(), mutates);
        }
    }

    // No filesystem paths of the repo should leak into the JSON.
    let raw = repo.stdout(&["--json"]);
    assert!(!raw.contains(repo.root.to_str().unwrap()), "{raw}");
}

/// `--json` must emit pure JSON: no ANSI codes, no spinner frames or
/// loading text on either stream, nothing but the document itself.
#[test]
fn json_output_is_pure() {
    let repo = Repo::with_commit();
    let out = repo.oops(&["--json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.trim_start().starts_with('{'), "{stdout}");
    assert!(!stdout.contains('\u{1b}'), "no ANSI in JSON: {stdout}");
    assert!(!stdout.to_lowercase().contains("inspecting"), "{stdout}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.is_empty(), "no spinner or noise on stderr: {stderr}");
    serde_json::from_str::<serde_json::Value>(&stdout).expect("stdout parses as JSON");
}

/// Human output through a pipe must also be spinner-free on both streams.
#[test]
fn piped_human_output_has_no_spinner_artifacts() {
    let repo = Repo::with_commit();
    let out = repo.oops(&[]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(!stdout.to_lowercase().contains("inspecting"), "{stdout}");
    assert!(!stdout.contains('\r'), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn explain_and_verbose_show_repository_state() {
    let repo = Repo::with_commit();
    let explain = repo.stdout(&["explain"]);
    assert!(explain.contains("repository"), "{explain}");
    assert!(explain.contains("oops checked"), "{explain}");

    let verbose = repo.stdout(&["--verbose"]);
    assert!(verbose.contains("repository"), "{verbose}");
    assert!(verbose.contains("branch"), "{verbose}");
    assert!(verbose.contains("recent activity"), "{verbose}");
}

#[test]
fn output_has_no_ansi_escapes_when_piped() {
    let repo = Repo::with_commit();
    let text = repo.stdout(&[]);
    assert!(!text.contains('\u{1b}'), "piped output must be colorless");
}

#[test]
fn not_a_repository_exits_with_code_2() {
    let tmp = TempDir::new().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_oops"))
        .current_dir(tmp.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("git repository"), "{stderr}");
}

/// The core safety promise: running oops leaves the repository byte-for-byte
/// identical as far as git can tell — status, reflog and stash all unchanged.
#[test]
fn oops_never_changes_repository_state() {
    let repo = Repo::with_commit();
    conflicting_branches(&repo);
    repo.git_expect_failure(&["merge", "feature"]);

    let observe = |repo: &Repo| {
        let status = git_command(&repo.root, &["status", "--porcelain=v2"])
            .output()
            .unwrap();
        let reflog = git_command(&repo.root, &["reflog"]).output().unwrap();
        let stash = git_command(&repo.root, &["stash", "list"])
            .output()
            .unwrap();
        (status.stdout, reflog.stdout, stash.stdout)
    };

    let before = observe(&repo);
    repo.stdout(&[]);
    repo.stdout(&["--json"]);
    repo.stdout(&["--verbose"]);
    repo.stdout(&["explain"]);
    let after = observe(&repo);
    assert_eq!(before, after, "oops must not modify the repository");
}
