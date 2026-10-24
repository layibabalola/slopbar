# SlopBar Ultra releases and updates

SlopBar Ultra ships its own releases from this fork (`layibabalola/slopbar-ultra`) and updates
itself from them only. This is fork-only: it lives on the `ultra` branch and never goes to
`claude/subscription-value` or upstream pull requests.

## How it works

| Build | How it is made | Updates |
| --- | --- | --- |
| Local build (`scripts/install-local-build.ps1`) | `cargo build --release --features tauri/custom-protocol` | Manual only ("Manual updates only in this build") |
| Ultra release | `scripts/publish-ultra-release.ps1` on the owner's PC (or the workflow on a tag `ultra-v*`, once its secrets are set) | Automatic, from this fork only |
| Upstream SlopBar | upstream's `release.yml` with `upstream-updates` | From `thelioo/slopbar` |

- **Feature `ultra-updates`** (`src-tauri/Cargo.toml`) turns on automatic updates for Ultra
  release builds only. Local and dev builds don't enable it, so they stay manual.
- **Update channel.** Ultra installs read
  `https://github.com/layibabalola/slopbar-ultra/releases/download/ultra-latest/latest.json`.
  That is a fixed release named `ultra-latest`, not `releases/latest`, so no other release on this
  fork (such as "SlopBar Enhanced") can become an Ultra update. Each Ultra release moves the
  channel to itself.
- **Fork key.** Updates must be signed with the fork's own updater key (key id
  `593D8A8B75A9A204`). Its public key is in `tauri.conf.json` and `src-tauri/src/ultra_release.rs`.
  The private key is only on the owner's machine and in the GitHub secret.
- **An Ultra install never updates to upstream SlopBar**, because of three separate checks:
  1. `updates.rs` pins the Ultra channel URL and fork key in code under `ultra-updates`, so a
     config change merged from upstream can't point the app back at `thelioo/slopbar`.
  2. Upstream releases are signed with upstream's key, so their signature check fails.
  3. The app only accepts versions shaped `<x.y.z>-ultra.<n>` that are newer than the installed
     one. Upstream's `0.2.N` versions are refused.

  A unit test (`ultra_release::tests`, which runs in CI and in the release workflow) fails if
  `tauri.conf.json` stops pointing at the Ultra channel and key.

### Version scheme

The package version stays as it is (`tauri.conf.json` `version`, currently `0.1.0`). An Ultra
release adds a build suffix: version `0.1.0-ultra.<n>`, tag `ultra-v0.1.0-ultra.<n>`, release name
`SlopBar Ultra 0.1.0-ultra.<n>`. `<n>` must go up with every release. The workflow refuses a tag
whose package part doesn't match `tauri.conf.json`, and a tagged commit that isn't on `ultra`.

## Publishing a release (one command)

Releases are published from the owner's PC, where the fork key lives. No GitHub secret is needed.
In a clean checkout whose `HEAD` is the pushed `origin/ultra` (a detached worktree such as
`C:\code\slopbar-worktrees\ultra-release` keeps the working `ultra` checkout free):

```powershell
git -C C:\code\slopbar-worktrees\ultra-release fetch -q origin
git -C C:\code\slopbar-worktrees\ultra-release checkout -q --detach origin/ultra
pwsh C:\code\slopbar-worktrees\ultra-release\scripts\publish-ultra-release.ps1
```

The script picks the next build number. It then:

1. Builds with `ultra-updates`, with no signing variables in its environment, and checks that the
   executable reports this commit.
2. Signs the installer with `tauri signer sign --private-key-path`. Only the signer's process
   gets the password, through its environment. Neither value is printed or passed as an argument.
3. Checks that the signature's key id is the one in `tauri.conf.json`, publishes `ultra-v<version>`
   with exactly the installer, its `.sig` and `latest.json`, and compares them with what it
   downloads back.
4. Refuses a version that is not newer than the channel's. It then moves `ultra-latest` and reads
   it back.

Installed copies pick the release up at their next check: about 20 seconds after SlopBar starts,
then every 6 hours. A restart through the door (`restart_app`) makes the check happen now.

### Agents' MCP server (`slopbar-mcp.exe`)

The installer carries `slopbar-mcp.exe` beside `slopbar.exe` (Tauri bundles every binary of the
crate). Agents run it for their whole session, though, and a running executable can't be written,
so a quiet update used to skip it and leave the old build behind. Windows does allow renaming it.
The script's `--config` override adds the hook `src-tauri/nsis/ultra-hooks.nsh`, so release
installers first rename the file to `slopbar-mcp.old-<time>-<n>.exe` and then write the new one.
Running servers carry on from the old copy, and new agent sessions start the new one. At every
start the app deletes old copies that nothing runs any more (`src-tauri/src/mcp_update.rs`), and
the uninstaller removes them with the app. The script refuses to publish an installer without
the server and the hook.

The release notes give the server's SHA-256. `get_app_info` (door) reports `mcp.sha256` and
`mcp.set_aside`, the number of old copies still running agents' servers.

**After moving machines to releases, don't run `scripts/install-local-build.ps1` on them.** A
local build reports version `0.1.0`, which SemVer ranks above every `0.1.0-ultra.<n>`, and it
can't update itself. So that machine stops taking releases until a release build is installed over
it again.

`.github/workflows/ultra-release.yml` does the same job in GitHub Actions once the owner adds the
two secrets (below). Until then its tag runs end at the `gate` job, so the tags this script pushes
don't start a second publisher.

### Optional: add the GitHub secrets

Do this only if releases should come from Actions instead. The keypair was generated on 2026-10-22 and lives outside the repository, readable only by your
Windows user:

- `%USERPROFILE%\.slopbar-ultra\updater.key`: private key (encrypted with the password)
- `%USERPROFILE%\.slopbar-ultra\updater.key.password`: its password
- `%USERPROFILE%\.slopbar-ultra\updater.key.pub`: public key (already committed)

Back up the folder somewhere private, such as a password manager. **If the key is lost, installed
copies can't verify any future update.** You would have to reinstall them by hand with a build
that carries a new key.

From PowerShell, without showing either value:

```powershell
Get-Content -Raw "$env:USERPROFILE\.slopbar-ultra\updater.key" | gh secret set ULTRA_TAURI_SIGNING_PRIVATE_KEY -R layibabalola/slopbar-ultra
Get-Content -Raw "$env:USERPROFILE\.slopbar-ultra\updater.key.password" | gh secret set ULTRA_TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R layibabalola/slopbar-ultra
gh secret list -R layibabalola/slopbar-ultra
```

The names are `ULTRA_`-prefixed on purpose. Upstream's `release.yml` is also on this fork and
reads `TAURI_SIGNING_PRIVATE_KEY`. With that name unset, it can never sign a release with the
fork's key, so it can't produce anything an Ultra install would accept.

## Installing on another machine

1. Download `SlopBar_<version>_x64-setup.exe` from the newest `ultra-v…` release:
   `gh release download ultra-v0.1.0-ultra.1 -R layibabalola/slopbar-ultra -p "*-setup.exe"`,
   or use the release page.
2. Run it. The installer is per-user and needs no admin rights. It's unsigned (see below), so
   SmartScreen may say "Windows protected your PC": choose **More info → Run anyway**.
3. Sign in to accounts in SlopBar Ultra as usual. On a fresh install, "Update automatically" is
   on by default (Settings → Updates). The app
   checks about 20 seconds after launch and then every 6 hours, and installs Ultra releases
   quietly.

On a machine that already runs a local build (`scripts/install-local-build.ps1`), installing a
release over it makes updates possible. The saved settings keep "Update automatically" off,
though, so turn it on under Settings → Updates. Running the local install script again later puts
back a manual build.

## Optional: Authenticode signing (paid, not purchased)

The updater signature above proves that an update came from this fork. It doesn't sign the `.exe`
for Windows, so SmartScreen warns on first download until the file gains reputation. To remove the
warning, the installer needs an Authenticode signature. Options, none bought:

- **Azure Trusted Signing** (Microsoft's managed signing, about US$10/month at the time of writing;
  identity validation required). Integrates with GitHub Actions through
  `azure/trusted-signing-action` or Tauri's `bundle.windows.signCommand`.
- **OV or EV code-signing certificate** from a CA (roughly US$200–600/year; since 2023 the key
  must live on a hardware token or cloud HSM). EV used to get SmartScreen reputation instantly,
  but Microsoft no longer guarantees that.

Whichever you choose, wire it in through `bundle.windows.signCommand` in `tauri.conf.json`, or a
signing step before `tauri-action` uploads. Store its credentials as further `ULTRA_*` secrets. The
updater key stays as it is.

## Verifying locally

```powershell
cargo test --locked --no-default-features --lib --manifest-path src-tauri/Cargo.toml ultra_release
```

Don't set the signing variables in a shell by hand: they pass to every process it starts. To sign,
use `scripts/publish-ultra-release.ps1`, which gives the password to the signer's process alone.

## Rolling back a bad release

Installed copies move only forward, so a bad release is replaced by a newer one, never by
rewinding the channel:

1. Turn off "Update automatically" on machines that haven't taken it yet (door `update_settings`).
2. On a machine that took it, put the previous executables back from
   `%LOCALAPPDATA%\SlopBar-Rollback\<time>` (the remote swap keeps one) and restart through the door.
3. Revert the fix on `ultra`, push, and publish again. The script gives it the next build number.
