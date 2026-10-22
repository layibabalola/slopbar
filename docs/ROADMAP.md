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

## Wave 2: done (merged at ed8d1f6 on claude/subscription-value)

| # | Item | Status |
|---|---|---|
| 4 | **Pro 200 allowance change on 2026-10-29.** Flag which Pro 200 accounts will start running out once the grandfathered allowance ends. Time-sensitive: first in this wave. | done (d271ebd) |
| 5 | **Smarter auto-switch.** Prefer the account whose weekly reset or renewal comes soonest and still has headroom; skip accounts in use on another machine and plans being cancelled. | done (d271ebd) |
| 10 | **Pool sizing.** Weekly peaks per provider against the capacity owned: "your Codex usage fits in 3 Pro 200 plans with 30% spare; you hold 7". | done (d3565b6) |
| 11 | **Cost per use.** Dollars per thread and per percent used, per account and blended per provider. | done (d3565b6) |
| 20 | **Credit balance.** Show Codex purchased credits, what they are worth, and when they get spent. | done (d3565b6) |
| 15 | **Self-check panel.** Monitoring sign-ins about to expire, reconnects needed, stale accounts, door and rclone health, each with its one-click fix. | done (ed8d1f6) |
| 16 | **Telemetry policy.** Local only by default; at most an opt-in crash report inside the existing diagnostics export. Recorded in SECURITY.md and built with item 15. | done (ed8d1f6) |

## Wave 3: done (merged at a153ddc on claude/subscription-value)

| # | Item | Status |
|---|---|---|
| 6 | **An account per project.** Pin a repo to an account through the CLIs' config-folder environment variables, so projects stop fighting over the global login. | done (a153ddc) |
| 7 | **Quick switcher.** A global hotkey opens a type-to-filter list of accounts by email, with headroom. | done (a153ddc) |
| 8 | **Bulk actions.** Connect monitoring for all; remove lapsed accounts once their paid period ends. | done (a153ddc) |
| 9 | **Renewal calendar.** Export renewal dates as an `.ics` feed for Outlook or Google Calendar. | done (520049a) |
| 12 | **When you work.** A heatmap of usage by hour and weekday from local Codex and Claude logs, to size accounts for peak hours. | done (5cabf91) |
| 13 | **Monday digest.** One notification with spend, waste, renewals this week, and accounts that ran out. | done (520049a) |
| 14 | **CSV export** of weekly history. | done (5cabf91) |
| 21 | **Usage by project and model, per account.** Which repos each account worked in, which models it used and how many tokens each, from local Claude Code and Codex logs; attributed to accounts by the thread record (Codex) or by which account was active at the time (Claude). Added by the owner on 2026-10-21. | done (5cabf91) |
| 22 | **Auto-connect monitoring.** When an unmonitored account is the one in use, start its monitoring sign-in automatically (one browser approval), with backoff and a setting. Added by the owner on 2026-10-21. | done (7e7ba16) |
| 19 | **Claude Code status line.** The active account's headroom and time-to-limit in the status line. | done (520049a) |

## Wave 4: done (added by the owner on 2026-10-22; merged at d4c2800, releases on ultra)

| # | Item | Status |
|---|---|---|
| 23 | **Encrypt the account vault** with DPAPI like the monitoring store, with a safe one-time migration and rollback. | done (da3ba6f) |
| 24 | **Green tests and CI**: fix the failing login-script test and the flaky http_diagnostics test; run cargo test on Windows in GitHub Actions for every PR. | done (da3ba6f) |
| 25 | **Token health and revoke-all**: detect revoked sign-ins; one action revokes every SlopBar-held session of an account. | done (da3ba6f) |
| 26 | **Plan change advisor**: concrete cancel/downgrade moves with projected savings and remaining headroom. | done (d4c2800) |
| 27 | **Claude extra usage**: track pay-as-you-go overage (extra_usage limit and credits used) and alert before it bills. | done (d4c2800) |
| 28 | **Spend ledger**: month-by-month cost per plan, year-to-date totals, optional tax rate and currency. | done (d4c2800) |
| 29 | **SlopBar Ultra releases and updates**: signed installers from the fork; Ultra's updater follows the fork's releases. | done (ultra-releases ac7e652) |

## Wave 5: done (merged at 40439db)

| # | Item | Status |
|---|---|---|
| 30 | **Translations** of every new string into the nine other supported languages. | done (40439db) |
| 31 | **Exact Claude attribution**: a Claude Code SessionStart hook recording which account each session started on. | done (40439db) |
| 32 | **Account leases**: agents claim and release an account for a job through the door. | done (40439db) |
| 33 | **Smart launcher**: slopbar-run combines project pins with recommend_account. | done (40439db) |
| 34 | **Sparklines and compact mode** in the flyout. | done (40439db) |
| 35 | **Onboarding** for first run. | done (40439db) |
| 36 | **Accessibility pass**: keyboard, screen reader, contrast. | done (40439db) |

## Wave 6: done (added by the owner on 2026-10-22; merged at 40439db)

| # | Item | Status |
|---|---|---|
| 37 | **Install lock and strict single instance**: a machine-wide install lock; a second instance always hands over and exits; Health flags duplicates. Prompted by two instances running after racing installs on 2026-10-22. | done (40439db) |
| 38 | **MCP server cleanup**: slopbar-mcp exits when its agent disconnects; Health counts lingering servers. | done (40439db) |
| 39 | **Faster refresh**: per-phase timing; non-essential work off the refresh path; under 5 s for 15 accounts. | done (40439db) |
| 40 | **Runaway-agent alert**: abnormal burn per account, with the repo and model behind it. | done (40439db) |
| 41 | **Reset-credit advisor**: spend a banked Codex reset or switch; remind before resets expire. | done (40439db) |
| 42 | **Capacity-shortage advice**: the cheapest fix when the pool is short. | done (40439db) |
| 43 | **Encrypted backup and restore** of settings, plan details and history (never sign-ins). | done (40439db) |
| 44 | **Opt-in phone alerts** through a self-hostable push service (ntfy). | done (40439db) |

## Wave 7: done (added by the owner on 2026-10-22; merged at ac9e355)

| # | Item | Status |
|---|---|---|
| 45 | **Smaller footprint without losing features**: Settings, switcher and flyout built on demand; one shared WebView2 process group; quiet idle timers; capped diagnostics exports and install rollbacks. Measured: 404 MB -> 171 MB private and 4.88% -> 0.73% of a core at idle. | done (07416eb) |
| 46 | **Native taskbar widget**: draw the always-visible widget with Windows' own drawing instead of a web view, for a small (~80-100 MB) idle footprint. | done (ac9e355) |
| 47 | **Flyout on demand**: the flyout is built on click and closed when dismissed, so no WebView2 process runs at idle. Done in the native-widget session. | done (a74cf4b); replaced by 3418c53/d2fe406/7109682: the delay on click was too slow, so the flyout is now built hidden at startup and shown instantly (checked on screen: drawn within 0.3 s of a click) |
| 48 | **Full account tooltips** in the compact flyout and on the native widget (the same detail as the full view), **sparklines fixed** (a key-case mismatch had left them empty), **WSL never started needlessly** (`wsl_accounts` auto/on/off), and **the app no longer quits when the flyout closes**. Upstream thelioo/slopbar#33. | done (4194a17) |
| 30b | **Translate the 130 strings added in waves 5 and 6.** | done (aab0f12) |

## Done before this roadmap (branch `claude/subscription-value`; upstream thelioo/slopbar#8–#11, stacked on #6)

Subscriptions page with verdicts and renewal dates; Codex inspection from local history;
renewal dates from the providers and user corrections; ChatGPT Pro tier detection
(`prolite`/`pro`/`promax`); accounts in use on other computers through synced folders or rclone;
`inspect_window`, `list_sharing_locations` and `open_billing_page` doors.
