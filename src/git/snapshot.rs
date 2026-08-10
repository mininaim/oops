use super::GitRunner;
use anyhow::{Result, bail};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// One changed path from `git status --porcelain=v2`.
/// `staged` / `worktree` hold the raw XY status characters ('.' = unchanged).
#[derive(Debug, Clone)]
pub struct StatusEntry {
    pub staged: char,
    pub worktree: char,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub sha: String,
    pub time: u64,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub struct StashEntry {
    pub name: String,
    pub time: u64,
    pub subject: String,
}

#[derive(Debug, Clone, Default)]
pub struct RebaseInfo {
    pub branch: Option<String>,
    pub onto: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Operation {
    #[default]
    None,
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Operation::None => "none",
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
        }
    }
}

/// Everything oops knows about the repository, collected once, read-only.
#[derive(Debug, Clone, Default)]
pub struct RepositorySnapshot {
    /// Current branch name, `None` when HEAD is detached or the repo is unborn.
    pub branch: Option<String>,
    pub detached: bool,
    /// Short sha of HEAD, `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    /// (ahead, behind) relative to upstream, when git could compute it.
    pub ahead_behind: Option<(u32, u32)>,
    pub entries: Vec<StatusEntry>,
    pub conflicted: Vec<String>,
    pub untracked: usize,
    pub operation: Operation,
    pub rebase: RebaseInfo,
    pub commits: Vec<LogEntry>,
    pub reflog: Vec<LogEntry>,
    pub stashes: Vec<StashEntry>,
    /// Unix time when the snapshot was taken; injected so rules are testable.
    pub now: u64,
}

impl RepositorySnapshot {
    pub fn staged(&self) -> Vec<&StatusEntry> {
        self.entries.iter().filter(|e| e.staged != '.').collect()
    }

    pub fn unstaged_modified(&self) -> Vec<&StatusEntry> {
        self.entries
            .iter()
            .filter(|e| e.worktree != '.' && e.worktree != 'D')
            .collect()
    }

    pub fn unstaged_deleted(&self) -> Vec<&StatusEntry> {
        self.entries.iter().filter(|e| e.worktree == 'D').collect()
    }

    pub fn head_subject(&self) -> Option<&str> {
        self.commits.first().map(|c| c.subject.as_str())
    }
}

pub fn collect(git: &dyn GitRunner) -> Result<RepositorySnapshot> {
    let inside = git.run(&["rev-parse", "--is-inside-work-tree"])?;
    if !inside.ok {
        bail!("this doesn't look like a git repository");
    }
    if inside.stdout.trim() != "true" {
        bail!("this looks like a bare repository — oops needs a working tree");
    }

    let status = git.run(&[
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=normal",
    ])?;
    if !status.ok {
        bail!("git status failed: {}", status.stderr.trim());
    }
    let mut snap = parse_status(&status.stdout);

    let log = git.run(&["log", "-n", "10", "--format=%h%x09%ct%x09%s"])?;
    if log.ok {
        snap.commits = parse_log_lines(&log.stdout);
    }

    // %gd with --date=unix carries the time the ref update happened;
    // %ct would give the target commit's own time, which can be years off.
    let reflog = git.run(&[
        "reflog",
        "show",
        "-n",
        "25",
        "--date=unix",
        "--format=%h%x09%gd%x09%gs",
    ])?;
    if reflog.ok {
        snap.reflog = parse_reflog_lines(&reflog.stdout);
    }

    let stash = git.run(&["stash", "list", "--format=%gd%x09%ct%x09%gs"])?;
    if stash.ok {
        snap.stashes = parse_stash_lines(&stash.stdout);
    }

    let git_dir = git.run(&["rev-parse", "--absolute-git-dir"])?;
    if git_dir.ok {
        detect_operation(Path::new(git_dir.stdout.trim()), &mut snap);
    }

    snap.now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(snap)
}

fn parse_status(text: &str) -> RepositorySnapshot {
    let mut snap = RepositorySnapshot::default();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            if let Some(oid) = header.strip_prefix("branch.oid ") {
                if oid != "(initial)" {
                    snap.head = Some(oid.chars().take(7).collect());
                }
            } else if let Some(head) = header.strip_prefix("branch.head ") {
                if head == "(detached)" {
                    snap.detached = true;
                } else {
                    snap.branch = Some(head.to_string());
                }
            } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
                snap.upstream = Some(upstream.to_string());
            } else if let Some(ab) = header.strip_prefix("branch.ab ") {
                snap.ahead_behind = parse_ahead_behind(ab);
            }
        } else if let Some(rest) = line.strip_prefix("1 ") {
            // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
            if let Some(entry) = parse_change_entry(rest, 8) {
                snap.entries.push(entry);
            }
        } else if let Some(rest) = line.strip_prefix("2 ") {
            // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\t<origPath>
            if let Some(mut entry) = parse_change_entry(rest, 9) {
                if let Some((new_path, _)) = entry.path.split_once('\t') {
                    entry.path = new_path.to_string();
                }
                snap.entries.push(entry);
            }
        } else if let Some(rest) = line.strip_prefix("u ") {
            // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
            let path = rest.splitn(10, ' ').nth(9);
            if let Some(path) = path {
                snap.conflicted.push(path.to_string());
            }
        } else if line.starts_with("? ") {
            snap.untracked += 1;
        }
    }
    snap
}

/// Splits a porcelain change line into `fields` space-separated columns and
/// builds an entry from the XY column and the trailing path column.
fn parse_change_entry(rest: &str, fields: usize) -> Option<StatusEntry> {
    let mut parts = rest.splitn(fields, ' ');
    let xy = parts.next()?;
    let path = parts.nth(fields - 2)?;
    let mut chars = xy.chars();
    Some(StatusEntry {
        staged: chars.next()?,
        worktree: chars.next()?,
        path: path.to_string(),
    })
}

fn parse_ahead_behind(text: &str) -> Option<(u32, u32)> {
    let (ahead, behind) = text.split_once(' ')?;
    Some((
        ahead.strip_prefix('+')?.parse().ok()?,
        behind.strip_prefix('-')?.parse().ok()?,
    ))
}

fn parse_log_lines(text: &str) -> Vec<LogEntry> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            Some(LogEntry {
                sha: parts.next()?.to_string(),
                time: parts.next()?.parse().ok()?,
                subject: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Parses `%h\t%gd\t%gs` reflog lines where `%gd` looks like `HEAD@{1735689600}`.
fn parse_reflog_lines(text: &str) -> Vec<LogEntry> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let sha = parts.next()?.to_string();
            let selector = parts.next()?;
            let time = selector
                .split_once('{')?
                .1
                .trim_end_matches('}')
                .parse()
                .ok()?;
            Some(LogEntry {
                sha,
                time,
                subject: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

fn parse_stash_lines(text: &str) -> Vec<StashEntry> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            Some(StashEntry {
                name: parts.next()?.to_string(),
                time: parts.next()?.parse().ok()?,
                subject: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Detects in-progress operations from state files inside the git directory.
/// This mirrors git's own precedence: rebase > merge > cherry-pick > revert
/// (a conflicted rebase step can leave CHERRY_PICK_HEAD around, so order matters).
fn detect_operation(git_dir: &Path, snap: &mut RepositorySnapshot) {
    let rebase_merge = git_dir.join("rebase-merge");
    let rebase_apply = git_dir.join("rebase-apply");
    if rebase_merge.is_dir() || rebase_apply.is_dir() {
        snap.operation = Operation::Rebase;
        let dir = if rebase_merge.is_dir() {
            rebase_merge
        } else {
            rebase_apply
        };
        snap.rebase.branch = read_state_file(&dir.join("head-name"))
            .map(|s| s.trim_start_matches("refs/heads/").to_string());
        snap.rebase.onto = read_state_file(&dir.join("onto")).map(|s| s.chars().take(7).collect());
    } else if git_dir.join("MERGE_HEAD").is_file() {
        snap.operation = Operation::Merge;
    } else if git_dir.join("CHERRY_PICK_HEAD").is_file() {
        snap.operation = Operation::CherryPick;
    } else if git_dir.join("REVERT_HEAD").is_file() {
        snap.operation = Operation::Revert;
    }
}

fn read_state_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{GitOutput, is_read_only};
    use std::cell::RefCell;

    const CLEAN_STATUS: &str = "\
# branch.oid 1234567890abcdef1234567890abcdef12345678
# branch.head main
# branch.upstream origin/main
# branch.ab +0 -0
";

    #[test]
    fn parses_clean_status_with_upstream() {
        let snap = parse_status(CLEAN_STATUS);
        assert_eq!(snap.branch.as_deref(), Some("main"));
        assert_eq!(snap.head.as_deref(), Some("1234567"));
        assert_eq!(snap.upstream.as_deref(), Some("origin/main"));
        assert_eq!(snap.ahead_behind, Some((0, 0)));
        assert!(!snap.detached);
        assert!(snap.entries.is_empty());
    }

    #[test]
    fn parses_detached_head() {
        let snap = parse_status(
            "# branch.oid 1234567890abcdef1234567890abcdef12345678\n# branch.head (detached)\n",
        );
        assert!(snap.detached);
        assert_eq!(snap.branch, None);
    }

    #[test]
    fn parses_unborn_repository() {
        let snap = parse_status("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(snap.head, None);
        assert_eq!(snap.branch.as_deref(), Some("main"));
    }

    #[test]
    fn parses_ahead_behind_counts() {
        let snap = parse_status(
            "# branch.oid 1234567890abcdef1234567890abcdef12345678\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -3\n",
        );
        assert_eq!(snap.ahead_behind, Some((2, 3)));
    }

    #[test]
    fn parses_change_entries_untracked_and_conflicts() {
        let text = "\
# branch.oid 1234567890abcdef1234567890abcdef12345678
# branch.head main
1 .M N... 100644 100644 100644 abc def src/main.rs
1 D. N... 100644 000000 000000 abc 000 gone.txt
1 .D N... 100644 100644 000000 abc def also gone.txt
2 R. N... 100644 100644 100644 abc def R100 new name.rs\told.rs
u UU N... 100644 100644 100644 100644 a b c conflicted file.rs
? untracked.txt
? another.txt
";
        let snap = parse_status(text);
        assert_eq!(snap.entries.len(), 4);
        assert_eq!(snap.unstaged_modified().len(), 1);
        assert_eq!(snap.unstaged_modified()[0].path, "src/main.rs");
        assert_eq!(snap.staged().len(), 2);
        assert_eq!(snap.unstaged_deleted().len(), 1);
        assert_eq!(snap.unstaged_deleted()[0].path, "also gone.txt");
        assert_eq!(snap.entries[3].path, "new name.rs");
        assert_eq!(snap.conflicted, vec!["conflicted file.rs"]);
        assert_eq!(snap.untracked, 2);
    }

    #[test]
    fn parses_log_and_stash_lines() {
        let log = parse_log_lines(
            "abc1234\t1700000000\tfix: a thing\ndef5678\t1699990000\tfeat: another\n",
        );
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].sha, "abc1234");
        assert_eq!(log[0].time, 1_700_000_000);
        assert_eq!(log[1].subject, "feat: another");

        let stash = parse_stash_lines("stash@{0}\t1700000000\tWIP on main: abc1234 x\n");
        assert_eq!(stash[0].name, "stash@{0}");
    }

    #[test]
    fn parses_reflog_lines_using_ref_update_time() {
        let entries =
            parse_reflog_lines("abc1234\tHEAD@{1735689600}\tcheckout: moving from main to feat\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].time, 1_735_689_600);
        assert_eq!(entries[0].subject, "checkout: moving from main to feat");
    }

    /// Records every git command issued during collection and answers with
    /// canned output, proving the collector only ever asks read-only questions.
    struct SpyGit {
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl GitRunner for SpyGit {
        fn run(&self, args: &[&str]) -> anyhow::Result<GitOutput> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            let stdout = match args.first().copied() {
                Some("rev-parse") if args.contains(&"--is-inside-work-tree") => "true\n",
                Some("rev-parse") => "/nonexistent/git/dir\n",
                Some("status") => CLEAN_STATUS,
                _ => "",
            };
            Ok(GitOutput {
                ok: true,
                stdout: stdout.to_string(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn collection_only_issues_read_only_git_commands() {
        let spy = SpyGit {
            calls: RefCell::new(Vec::new()),
        };
        let snap = collect(&spy).unwrap();
        assert_eq!(snap.branch.as_deref(), Some("main"));

        let calls = spy.calls.borrow();
        assert!(!calls.is_empty());
        for call in calls.iter() {
            let args: Vec<&str> = call.iter().map(String::as_str).collect();
            assert!(
                is_read_only(&args),
                "collector issued a non-allowlisted command: git {}",
                call.join(" ")
            );
        }
    }
}
