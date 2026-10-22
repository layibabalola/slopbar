# SlopBar Ultra roadmap

Owner-approved on 2026-10-21: build every item below. Work drains in waves of at most three
parallel sessions, each in its own worktree branched from `claude/subscription-value`, so sessions
rarely edit the same Settings files at once. A wave's branches merge back into
`claude/subscription-value` (merge commits, no rebase) before the next wave starts. Every item
lands with tests on synthetic identities, a door (MCP) for anything the window can do, and a check
through `inspect_window`, never through the user's desktop.

This file lives only on the fork's `ultra` branch; status is updated here.

Status: `running` (a session has it), `queued`, `done` (merged; the commit is noted).

**Upstream PRs.** Every item also goes to the official product as its own pull request on
thelioo/slopbar. Its branch is cut at the item's merge commit, it targets `main`, and its
description names the PRs it stacks on. The work before this roadmap is #6, then #8, #9, #10 and
#11, merged in that order. This roadmap file itself stays on the fork and is never part of an
upstream PR.

## Wave 1: done (merged at b25d07f on claude/subscription-value)

| # | Item | Status |
|---|---|---|
| 1 | **Renewal warnings.** A notification 3 days before an unused or mostly idle plan renews, naming the account, its verdict and cost, with Manage plan one click away. A softer one for any paid plan renewing within 24 hours. | done (3aea1a9) |
| 2 | **Runs-out-at forecast.** "Hits its weekly limit Thursday around 3 PM" in the flyout and on Subscriptions, plus an optional heads-up for the account in use. | done (3aea1a9) |
| 3 | **Unexpected-use alert.** With sharing on everywhere, usage rising while no machine reports the account in use means the web or mobile app, or a leaked sign-in. | done (b25d07f) |
| 17 | **`recommend_account` door.** An agent asks which account has headroom for its job; SlopBar ranks them and can switch. | done (5ebc677) |
| 18 | **`get_headroom` door.** A long-running agent slows down or pauses before a limit instead of failing mid-task. | done (5ebc677) |

## Wave 2: running

| # | Item |
|---|---|
| 4 | **Pro 200 allowance change on 2026-10-29.** Flag which Pro 200 accounts will start running out once the grandfathered allowance ends. Time-sensitive: first in this wave. |
| 5 | **Smarter auto-switch.** Prefer the account whose weekly reset or renewal comes soonest and still has headroom; skip accounts in use on another machine and plans being cancelled. |
| 10 | **Pool sizing.** Weekly peaks per provider against the capacity owned: "your Codex usage fits in 3 Pro 200 plans with 30% spare; you hold 7". |
| 11 | **Cost per use.** Dollars per thread and per percent used, per account and blended per provider. |
| 20 | **Credit balance.** Show Codex purchased credits, what they are worth, and when they get spent. |
| 15 | **Self-check panel.** Monitoring sign-ins about to expire, reconnects needed, stale accounts, door and rclone health, each with its one-click fix. |
| 16 | **Telemetry policy.** Local only by default; at most an opt-in crash report inside the existing diagnostics export. Recorded in SECURITY.md and built with item 15. |

## Wave 3: queued

| # | Item |
|---|---|
| 6 | **An account per project.** Pin a repo to an account through the CLIs' config-folder environment variables, so projects stop fighting over the global login. |
| 7 | **Quick switcher.** A global hotkey opens a type-to-filter list of accounts by email, with headroom. |
| 8 | **Bulk actions.** Connect monitoring for all; remove lapsed accounts once their paid period ends. |
| 9 | **Renewal calendar.** Export renewal dates as an `.ics` feed for Outlook or Google Calendar. |
| 12 | **When you work.** A heatmap of usage by hour and weekday from local Codex and Claude logs, to size accounts for peak hours. |
| 13 | **Monday digest.** One notification with spend, waste, renewals this week, and accounts that ran out. |
| 14 | **CSV export** of weekly history. |
| 19 | **Claude Code status line.** The active account's headroom and time-to-limit in the status line. |

## Done before this roadmap (branch `claude/subscription-value`; upstream thelioo/slopbar#8–#11, stacked on #6)

Subscriptions page with verdicts and renewal dates; Codex inspection from local history;
renewal dates from the providers and user corrections; ChatGPT Pro tier detection
(`prolite`/`pro`/`promax`); accounts in use on other computers through synced folders or rclone;
`inspect_window`, `list_sharing_locations` and `open_billing_page` doors.
