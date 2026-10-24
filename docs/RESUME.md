# Resume SlopBar Ultra work

Pointer-only. Nothing here is state. Derive every fact from the commands below at read time.

## Announce

"Resuming SlopBar Ultra (fork `layibabalola/slopbar-ultra`, local `C:\code\slopbar`)."

## Read first

1. `AGENTS.md` (push the work branch after committing; never force-push; keep account data,
   credentials, diagnostics and backups out of the repo; synthetic test identities).
2. Project memory: `~/.claude/projects/C--code-slopbar/memory/MEMORY.md`. Read the fork layout, test
   before done, emails as labels, auto-start sessions and other-machines-over-SSH entries.
3. `docs/ROADMAP.md` on this branch, for what has shipped and in which waves.

## Derive the state

```powershell
cd C:\code\slopbar; git fetch -q --all --prune
git log --oneline -3 origin/claude/subscription-value      # integration (upstream-clean)
git log --oneline -3 origin/ultra                          # fork build branch (installs come from here)
git log --oneline --no-merges origin/ultra..origin/claude/subscription-value   # not yet in ultra
gh pr list --repo thelioo/slopbar --author layibabalola --state open --limit 5  # upstream stack head
$h = gh pr view <top PR number> --repo thelioo/slopbar --json headRefOid --jq .headRefOid
git diff --stat "$h" origin/claude/subscription-value      # work with no upstream PR yet (by content)
git cherry -v "$h" origin/claude/subscription-value        # its commits ('+' lines; some are reworked copies)
git worktree list                                           # leftover session worktrees
```

Installed build and health on this PC, through the app's door (the slopbar MCP server, or the
`door.ps1` helper in `~/.claude/ops/remote-slopbar/`): `get_app_info`, `get_health`.

Other machines: `ssh ultramagnus` and `ssh virtualten` (key auth, PowerShell). Update and visually check
them with `~/.claude/ops/remote-slopbar/README.md`.

## Standing procedure

- Feature work goes on a branch cut from `origin/claude/subscription-value`, merged back with a normal
  merge, and pushed.
- Then merge into `ultra` in `C:\code\slopbar-worktrees\ultra`. On conflict, keep ultra's
  `tauri.conf.json` `plugins.updater` block and re-brand visible strings. Then run
  `cargo build --locked --release --features tauri/custom-protocol` in `src-tauri` and
  `scripts\install-local-build.ps1`.
- Verify on screen before calling anything done: `scripts\test-flyout-click.ps1` must PASS, and you
  look at the PNG. Unit tests alone are not verification.
- Each feature goes upstream as a PR to thelioo/slopbar, cut as a pinned `pr/<name>` branch and
  stacked on the previous one: cherry-pick the commits onto the previous PR's head, because the PR
  branches are linear copies, not ancestors of `claude/subscription-value` (so `git log $h..` overcounts).
  Update `docs/ROADMAP.md` on `ultra`.
- Never ask Layi to restart SlopBar: the install script restarts it through the door.

## Known open items (re-derive; don't trust this list)

- Work after the top upstream PR with no PR yet (the derive command above counts it).
- No Ultra release has been published yet: the GitHub secrets in `docs/ULTRA-RELEASES.md` are an
  owner-only step. Until then, other machines are updated over SSH.
- Leftover worktrees from finished sessions.
