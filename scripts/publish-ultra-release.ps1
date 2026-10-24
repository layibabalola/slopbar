#Requires -Version 7
<#
Publishes a SlopBar Ultra release from this machine and moves the update channel to it, with no
GitHub secrets: the fork's updater key stays in %USERPROFILE%\.slopbar-ultra. See docs/ULTRA-RELEASES.md.

  pwsh scripts/publish-ultra-release.ps1            # next build number
  pwsh scripts/publish-ultra-release.ps1 -Build 7   # a given one (must be higher than any before)

1. Checks this checkout is clean and exactly origin/ultra, and picks version <package>-ultra.<n>
   higher than every ultra-v tag and the channel.
2. Builds the installer with ultra-updates and no signing key in the environment, and checks the
   executable reports this commit.
3. Signs the installer with `tauri signer sign`. Only that child process gets the key's password,
   through its environment; neither the key nor the password is printed or passed as an argument.
4. Checks the signature was made by the fork's key, writes latest.json, and publishes release
   ultra-v<version> with exactly the installer, its .sig and latest.json. Downloads them again and
   compares hashes.
5. Moves the channel (release ultra-latest) to this latest.json and reads it back.

Staging files live in %TEMP%, never in the repository.
#>
param(
  [int]$Build = 0,
  [string]$Repo = 'layibabalola/slopbar-ultra',
  [string]$KeyDir = (Join-Path $env:USERPROFILE '.slopbar-ultra')
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$channelTag = 'ultra-latest'

function Invoke-Checked([string]$what, [scriptblock]$block) {
  & $block
  if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)." }
}

# The minisign key id: bytes 2..9 of the key or signature line (after the comment line).
function Get-KeyId([string]$base64) {
  $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($base64.Trim()))
  $line = ($text -split "`r?`n")[1]
  $bytes = [Convert]::FromBase64String($line)
  return [BitConverter]::ToString($bytes[2..9])
}

# 1. Source and version.
Invoke-Checked 'git fetch' { git -C $root fetch -q origin ultra --tags }
if (git -C $root status --porcelain) { throw 'The checkout has uncommitted changes.' }
$head = (git -C $root rev-parse HEAD).Trim()
if ($head -ne (git -C $root rev-parse origin/ultra).Trim()) { throw "HEAD $head is not origin/ultra; release only the pushed ultra tip." }
$conf = Get-Content (Join-Path $root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
$package = $conf.version
if ($package -notmatch '^\d+\.\d+\.\d+$') { throw "Package version $package is not x.y.z." }
$pubkey = $conf.plugins.updater.pubkey
if (@($conf.plugins.updater.endpoints) -join ' ' -ne "https://github.com/$Repo/releases/download/$channelTag/latest.json") {
  throw 'The updater must point only at the Ultra update channel.'
}

$taken = git -C $root ls-remote --tags origin "refs/tags/ultra-v$package-ultra.*" |
  ForEach-Object { if ($_ -match "ultra-v$([regex]::Escape($package))-ultra\.(\d+)$") { [int]$Matches[1] } }
$highest = ($taken | Measure-Object -Maximum).Maximum ?? 0
if ($Build -eq 0) { $Build = $highest + 1 }
if ($Build -le $highest) { throw "Build $Build is not higher than ultra-v$package-ultra.$highest." }
$version = "$package-ultra.$Build"
$tag = "ultra-v$version"

$stage = Join-Path ([IO.Path]::GetTempPath()) "slopbar-ultra-release-$Build-$PID"
$null = New-Item -ItemType Directory -Force $stage
$current = $null
gh release download $channelTag -R $Repo -p latest.json -D (Join-Path $stage 'channel') 2>$null
if ($LASTEXITCODE -eq 0) {
  $current = (Get-Content (Join-Path $stage 'channel/latest.json') -Raw | ConvertFrom-Json).version
  if ($current -notmatch '^(\d+\.\d+\.\d+)-ultra\.(\d+)$') { throw "The channel holds $current, not an Ultra version." }
  if ([version]$Matches[1] -gt [version]$package -or ([version]$Matches[1] -eq [version]$package -and [int]$Matches[2] -ge $Build)) {
    throw "The channel already holds $current; $version would not be newer."
  }
}
Write-Output "Publishing $version from $head (channel now: $($current ?? 'none'))."

# 2. Build, with no signing key anywhere in this process.
# `tauri build` rewrites Cargo.toml with LF line ends. A CRLF checkout (core.autocrlf) changes size,
# so git, and the build's dirty check, would call the clean tree modified. Write it as LF first and
# restage it (the same blob) so git knows that size.
$manifestPath = Join-Path $root 'src-tauri/Cargo.toml'
$text = [IO.File]::ReadAllText($manifestPath)
if ($text.Contains("`r`n")) { [IO.File]::WriteAllText($manifestPath, $text.Replace("`r`n", "`n")) }
Invoke-Checked 'git add Cargo.toml' { git -C $root add src-tauri/Cargo.toml }
if (git -C $root status --porcelain) { throw 'Cargo.toml differs from HEAD beyond line ends.' }
Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PATH, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { $env:Path = "$env:ProgramFiles\nodejs;$env:Path" }
# The installer hook sets a running slopbar-mcp.exe aside so the new one is written (agents' servers
# lock it; a quiet install would skip it).
$override = Join-Path $stage 'build.conf.json'
@{
  version = $version
  bundle  = @{
    createUpdaterArtifacts = $false
    windows                = @{ nsis = @{ installerHooks = (Join-Path $root 'src-tauri/nsis/ultra-hooks.nsh') } }
  }
} | ConvertTo-Json -Depth 5 | Set-Content $override
$pnpm = if (Get-Command pnpm -ErrorAction SilentlyContinue) { @('pnpm') } else { @('corepack', 'pnpm') }
$pnpmArgs = @($pnpm | Select-Object -Skip 1)
Push-Location $root
try {
  Invoke-Checked 'pnpm install' { & $pnpm[0] @pnpmArgs install --frozen-lockfile }
  Invoke-Checked 'Unit tests' { cargo test --locked --no-default-features --lib --manifest-path src-tauri/Cargo.toml }
  Invoke-Checked 'Build' { & $pnpm[0] @pnpmArgs tauri build --features ultra-updates --config $override }
} finally { Pop-Location }

$exe = Join-Path $root 'src-tauri/target/release/slopbar.exe'
$infoPath = Join-Path $stage 'build-info.json'
$p = Start-Process -FilePath $exe -ArgumentList @('--build-info-to', $infoPath) -WindowStyle Hidden -Wait -PassThru
if ($p.ExitCode -ne 0) { throw 'Build metadata command failed.' }
$info = Get-Content $infoPath -Raw | ConvertFrom-Json
if ($info.commit -ne $head -or $info.dirty -or $info.tauri_dev -ne $false -or -not $info.upstream_updates) {
  throw "The built executable is not a clean ultra-updates build of $head."
}
# The installer packs agents' MCP server from here, with the hook that sets a running one aside.
$mcpHash = (Get-FileHash (Join-Path $root 'src-tauri/target/release/slopbar-mcp.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
$nsi = Get-Content (Join-Path $root 'src-tauri/target/release/nsis/x64/installer.nsi') -Raw
if ($nsi -notmatch '/oname=slopbar-mcp\.exe' -or ($nsi -notmatch 'slopbar-mcp\.old-' -and $nsi -notmatch 'ultra-hooks\.nsh')) {
  throw 'The installer does not carry slopbar-mcp.exe with the hook that sets a running one aside.'
}
$setupName = "SlopBar_${version}_x64-setup.exe"
$setup = Join-Path $stage $setupName
Copy-Item (Join-Path $root "src-tauri/target/release/bundle/nsis/$setupName") $setup

# 3. Sign. The password reaches only the signer's environment.
$keyPath = Join-Path $KeyDir 'updater.key'
$start = [Diagnostics.ProcessStartInfo]::new((Get-Command node).Source)
$tauriCli = Join-Path $root 'node_modules/@tauri-apps/cli/tauri.js'
foreach ($a in @($tauriCli, 'signer', 'sign', '--private-key-path', $keyPath, $setup)) { $start.ArgumentList.Add($a) }
$start.UseShellExecute = $false
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.Environment['TAURI_SIGNING_PRIVATE_KEY_PASSWORD'] = (Get-Content (Join-Path $KeyDir 'updater.key.password') -Raw).TrimEnd("`r", "`n")
try {
  $signer = [Diagnostics.Process]::Start($start)
  $null = $signer.StandardOutput.ReadToEnd(); $null = $signer.StandardError.ReadToEnd()
  $signer.WaitForExit()
  if ($signer.ExitCode -ne 0) { throw "Signing failed (exit $($signer.ExitCode)); check the key and password files." }
} finally {
  $start.Environment.Remove('TAURI_SIGNING_PRIVATE_KEY_PASSWORD') | Out-Null
  $start = $null
}
$signature = (Get-Content "$setup.sig" -Raw).Trim()
if ((Get-KeyId $signature) -ne (Get-KeyId $pubkey)) { throw 'The signature was not made with the key in tauri.conf.json.' }

# 4. Publish the version.
$url = "https://github.com/$Repo/releases/download/$tag/$setupName"
$manifest = [ordered]@{
  version   = $version
  notes     = "SlopBar Ultra $version, built from $head on the ultra branch."
  pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'")
  platforms = [ordered]@{ 'windows-x86_64' = [ordered]@{ signature = $signature; url = $url } }
}
$latest = Join-Path $stage 'latest.json'
$manifest | ConvertTo-Json -Depth 5 | Set-Content $latest -Encoding utf8NoBOM
$files = @($setup, "$setup.sig", $latest)
Invoke-Checked 'gh release create' {
  gh release create $tag @files -R $Repo --target $head --latest=false `
    --title "SlopBar Ultra $version" `
    --notes "SlopBar Ultra $version, built from $head on the ultra branch. Installs update themselves from this fork only.`n`nslopbar-mcp.exe SHA-256: $mcpHash"
}
$check = Join-Path $stage 'check'
Invoke-Checked 'Download of the release' { gh release download $tag -R $Repo -D $check }
foreach ($f in $files) {
  $back = Join-Path $check (Split-Path -Leaf $f)
  if ((Get-FileHash $f).Hash -ne (Get-FileHash $back).Hash) { throw "$(Split-Path -Leaf $f) on the release differs from the one built." }
}

# 5. Move the channel.
gh release view $channelTag -R $Repo *> $null
if ($LASTEXITCODE -ne 0) {
  Invoke-Checked 'Creating the channel' {
    gh release create $channelTag -R $Repo --target $head --prerelease --latest=false --title 'SlopBar Ultra update channel' `
      --notes 'Holds latest.json for automatic updates of SlopBar Ultra. Download the installer from the newest ultra-v release instead.'
  }
}
Invoke-Checked 'Moving the channel' { gh release upload $channelTag $latest -R $Repo --clobber }
Remove-Item -Recurse -Force (Join-Path $stage 'channel') -ErrorAction SilentlyContinue
Invoke-Checked 'Reading the channel back' { gh release download $channelTag -R $Repo -p latest.json -D (Join-Path $stage 'channel') }
if ((Get-FileHash $latest).Hash -ne (Get-FileHash (Join-Path $stage 'channel/latest.json')).Hash) { throw 'The channel does not hold this latest.json.' }

Write-Output "Published $version"
Write-Output "  release: https://github.com/$Repo/releases/tag/$tag"
Write-Output "  channel: https://github.com/$Repo/releases/download/$channelTag/latest.json"
Write-Output "  installer: $setup"
Write-Output "  slopbar-mcp.exe sha256: $mcpHash"
