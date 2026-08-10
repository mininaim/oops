# oops

You broke something. `oops` figures out what.

Run it inside a git repository after something confusing happens. It reads
the recent git state, explains what probably went on, and suggests safe
next steps. Then it gets out of your way.

```text
$ oops

  ● Rebase in progress
    1 conflict needs attention

    cart.py

    next
    › git status               see where the rebase stopped
    › git rebase --continue    continue after resolving · changes state
    › git rebase --abort       abandon and restore feat/checkout · changes state

    · review before running — "changes state" commands modify the repository

  ╰╴ nothing was changed
```

The full explanation, the evidence trail, and confidence levels live in
`oops --verbose`, along with a small reflog timeline of what HEAD has
been up to recently.

## What it does

Detects, deterministically: merges, rebases, cherry-picks and reverts in
progress; detached HEAD; unpushed, behind, and diverged branches; deleted
tracked files; dirty or staged-but-uncommitted trees; lingering stashes; and
the classic "switched branches and committed a bit too fast" mistake. Each
diagnosis comes with its confidence, the evidence behind it, and suggested
commands — read-only ones marked as such, mutating ones flagged with a ⚠.

- `oops` — the main event
- `oops explain` — the same, with the repository state it saw
- `oops --verbose` — diagnosis plus collected state
- `oops --json` — stable structured output for scripts

## What it does NOT do

It never changes your repository. Not a file, not a ref, not the index.
Every git command it runs is checked against a read-only allowlist at
runtime, and it asks git to skip even optional lock files. It doesn't run
recovery commands — it shows them to you. It will never suggest
`git reset --hard`, `git clean -fd`, or a force push as the easy way out.

No network, no telemetry, no AI, no shell hooks, no daemon. It also stays
out of your file contents — it reads git metadata, not your code, and
certainly not your `.env`.

## Install

On macOS:

```bash
brew install mininaim/tap/oops
```

Prebuilt Linux binaries live on the
[releases page](https://github.com/mininaim/oops/releases). To build from
source instead (local development):

```bash
cargo install --path .
```

Requires a `git` binary on PATH. Exit code is 0 when oops ran (even if it
found problems), 2 when it couldn't (not a repository, git missing).

## Safety philosophy

Git accidents are stressful, and stressed people paste destructive commands.
`oops` is built on the opposite idea: look carefully, explain calmly, prefer
reversible paths, and be honest about uncertainty — a heuristic gets a
`low confidence` label, not a scary warning. The tool that diagnoses the
mess must never be able to make one.

Respects `NO_COLOR`. Works on macOS and Linux.
