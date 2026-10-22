# SlopBar Ultra releases and updates

SlopBar Ultra ships its own releases from this fork (`layibabalola/slopbar-ultra`) and updates
itself from them only. This is fork-only: it lives on the `ultra` branch and never goes to
`claude/subscription-value` or upstream pull requests.

## How it works

| Build | How it is made | Updates |
| --- | --- | --- |
| Local build (`scripts/install-local-build.ps1`) | `cargo build --release --features tauri/custom-protocol` | Manual only ("Manual updates only in this build") |
| Ultra release | `.github/workflows/ultra-release.yml` on a tag `ultra-v*` | Automatic, from this fork only |
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

## Owner steps

### 1. Add the two GitHub secrets (once)

The keypair was generated on 2026-10-22 and lives outside the repository, readable only by your
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

### 2. Cut the first release

Make sure Actions are enabled on the fork (Settings → Actions). Then, with `ultra` merged and
pushed:

```powershell
git fetch origin
git tag ultra-v0.1.0-ultra.1 origin/ultra
git push origin ultra-v0.1.0-ultra.1
gh run watch -R layibabalola/slopbar-ultra
```

When the run is green:

- Release `ultra-v0.1.0-ultra.1` has `SlopBar_0.1.0-ultra.1_x64-setup.exe`, its `.sig`, and
  `latest.json`.
- Release `ultra-latest` (a pre-release, created on the first run) holds the same
  `latest.json`.

Check the channel:
`gh release download ultra-latest -R layibabalola/slopbar-ultra -p latest.json -O -`

For later releases, tag the new `ultra` tip with the next number (`ultra-v0.1.0-ultra.2`, …).
Never move or reuse a tag.

### 3. Install on another machine

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

### 4. Optional: Authenticode signing (paid, not purchased)

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
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.slopbar-ultra\updater.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Get-Content -Raw "$env:USERPROFILE\.slopbar-ultra\updater.key.password"
pnpm tauri build --features ultra-updates --config '{"version":"0.1.0-ultra.0"}'
```

This produces a signed `src-tauri/target/release/bundle/nsis/SlopBar_0.1.0-ultra.0_x64-setup.exe`
and `.sig`. Don't publish local builds. Releases come only from the workflow.
