pub mod snapshot;

use anyhow::{Context, Result, bail};
use std::process::Command;

#[derive(Debug)]
pub struct GitOutput {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Everything oops learns from git goes through this trait, so the diagnosis
/// engine can be tested without real repositories.
pub trait GitRunner {
    fn run(&self, args: &[&str]) -> Result<GitOutput>;
}

/// The complete set of git invocations oops is allowed to make. Anything not
/// on this list is refused at runtime, so no code path can mutate a repo.
pub fn is_read_only(args: &[&str]) -> bool {
    match args.first().copied() {
        Some("rev-parse" | "log") => true,
        Some("status") => args.contains(&"--porcelain=v2"),
        Some("reflog") => args.get(1).copied() == Some("show"),
        Some("stash") => args.get(1).copied() == Some("list"),
        _ => false,
    }
}

pub struct SystemGit;

impl GitRunner for SystemGit {
    fn run(&self, args: &[&str]) -> Result<GitOutput> {
        if !is_read_only(args) {
            bail!(
                "internal safety check: refusing to run `git {}` (not on the read-only allowlist)",
                args.join(" ")
            );
        }
        let output = Command::new("git")
            .args(args)
            // Even `git status` may opportunistically write the index;
            // this tells git to never take optional locks.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .context("failed to run git — is it installed and on PATH?")?;
        Ok(GitOutput {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_accepts_the_commands_oops_uses() {
        assert!(is_read_only(&["rev-parse", "--is-inside-work-tree"]));
        assert!(is_read_only(&["rev-parse", "--absolute-git-dir"]));
        assert!(is_read_only(&[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal"
        ]));
        assert!(is_read_only(&["log", "-n", "10", "--format=%h"]));
        assert!(is_read_only(&["reflog", "show", "-n", "25"]));
        assert!(is_read_only(&["stash", "list"]));
    }

    #[test]
    fn allowlist_rejects_every_known_mutating_command() {
        let forbidden: &[&[&str]] = &[
            &["commit", "-m", "x"],
            &["merge", "--abort"],
            &["rebase", "--abort"],
            &["rebase", "--continue"],
            &["cherry-pick", "--abort"],
            &["revert", "--abort"],
            &["reset", "--hard"],
            &["clean", "-fd"],
            &["checkout", "."],
            &["switch", "main"],
            &["restore", "."],
            &["push"],
            &["push", "--force"],
            &["pull"],
            &["fetch"],
            &["stash", "push"],
            &["stash", "pop"],
            &["stash", "apply"],
            &["stash", "drop"],
            &["stash", "clear"],
            &["reflog", "expire", "--all"],
            &["reflog", "delete", "HEAD@{0}"],
            &["gc"],
            &["prune"],
            &["add", "."],
            &["rm", "-r", "."],
            &["branch", "-D", "main"],
            &["tag", "-d", "v1"],
            &["update-ref", "-d", "refs/heads/main"],
            &["symbolic-ref", "HEAD", "refs/heads/other"],
            &["config", "user.name", "x"],
            &["status"], // without --porcelain=v2 it is not something oops issues
        ];
        for args in forbidden {
            assert!(!is_read_only(args), "must reject: git {}", args.join(" "));
        }
    }

    #[test]
    fn system_git_refuses_non_allowlisted_commands_before_spawning() {
        let err = SystemGit.run(&["reset", "--hard"]).unwrap_err();
        assert!(err.to_string().contains("refusing to run"));
    }
}
