<#
.SYNOPSIS
  Install/update the codex fork to its latest GitHub Release on Windows amd64.

.DESCRIPTION
  Windows counterpart of codex-update.sh. Downloads the latest
  x86_64-pc-windows-msvc .zip published by fork-sync-release.yml, verifies its
  sha256, extracts it under %LOCALAPPDATA%\codex-fork\releases\<tag>, repoints a
  `current` junction, and makes sure `current\bin` is on the user PATH so
  `codex` resolves to the fork build. Idempotent: exits if already on latest.

.EXAMPLE
  pwsh -File fork-tools\codex-update.ps1
#>
[CmdletBinding()]
param(
  [string]$RepoSlug = $(if ($env:CODEX_FORK_REPO_SLUG) { $env:CODEX_FORK_REPO_SLUG } else { 'its-mash/codex' }),
  [string]$Target   = 'x86_64-pc-windows-msvc'
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Root        = Join-Path $env:LOCALAPPDATA 'codex-fork'
$ReleasesDir = Join-Path $Root 'releases'
$CurrentLink = Join-Path $Root 'current'
$StateDir    = Join-Path $Root 'state'
$MarkerFile  = Join-Path $StateDir 'installed-tag'
New-Item -ItemType Directory -Force -Path $ReleasesDir, $StateDir | Out-Null

function Notify([string]$msg) { Write-Host "[codex-update] $msg" }

# --- discover the latest release (public repo -> no auth needed) ---
$api = "https://api.github.com/repos/$RepoSlug/releases/latest"
$headers = @{ 'User-Agent' = 'codex-fork-update'; 'Accept' = 'application/vnd.github+json' }
if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
$release = Invoke-RestMethod -Uri $api -Headers $headers

$tag = $release.tag_name
if (-not $tag) { throw "latest release has no tag" }

$asset = $release.assets | Where-Object { $_.name -like "*-$Target.zip" } | Select-Object -First 1
if (-not $asset) { throw "release $tag has no *-$Target.zip asset" }
$sumAsset = $release.assets | Where-Object { $_.name -eq ($asset.name + '.sha256') } | Select-Object -First 1

$installed = if (Test-Path $MarkerFile) { Get-Content $MarkerFile -Raw } else { '' }
if ($installed.Trim() -eq $tag) { Notify "already on latest release $tag"; return }

Notify "updating: $($installed.Trim()) -> $tag"

# --- download ---
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("codex-fork-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
  $zip = Join-Path $tmp $asset.name
  Invoke-WebRequest -Uri $asset.browser_download_url -Headers $headers -OutFile $zip

  # --- verify checksum if present ---
  if ($sumAsset) {
    $expected = ((Invoke-WebRequest -Uri $sumAsset.browser_download_url -Headers $headers).Content -split '\s+')[0]
    $actual = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLower()
    if ($expected.ToLower() -ne $actual) { throw "checksum mismatch for $($asset.name)" }
    Notify "checksum verified"
  }

  # --- extract ---
  Expand-Archive -Path $zip -DestinationPath $tmp -Force
  $pkgDir = Get-ChildItem -Path $tmp -Directory | Where-Object { $_.Name -like "codex-*-$Target" } | Select-Object -First 1
  if (-not $pkgDir -or -not (Test-Path (Join-Path $pkgDir.FullName 'bin\codex.exe'))) {
    throw "unexpected package layout in $($asset.name)"
  }

  # --- install into releases\<tag> and repoint the `current` junction ---
  $dest = Join-Path $ReleasesDir "$tag-$Target"
  if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
  Move-Item $pkgDir.FullName $dest

  if (Test-Path $CurrentLink) {
    (Get-Item $CurrentLink).Delete()
  }
  New-Item -ItemType Junction -Path $CurrentLink -Target $dest | Out-Null
  Set-Content -Path $MarkerFile -Value $tag -NoNewline
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# --- ensure current\bin is on the user PATH ---
$binDir = Join-Path $CurrentLink 'bin'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $binDir) {
  [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $binDir), 'User')
  Notify "added $binDir to your user PATH (open a new terminal to pick it up)"
}

$version = try { & (Join-Path $binDir 'codex.exe') --version 2>$null } catch { $tag }
Notify "codex updated: now on $tag ($version)"

# --- prune old fork releases, keep newest 3 ---
Get-ChildItem -Path $ReleasesDir -Directory -Filter "fork-*-$Target" |
  Sort-Object LastWriteTime -Descending | Select-Object -Skip 3 |
  Where-Object { $_.FullName -ne (Get-Item $CurrentLink).Target } |
  ForEach-Object { Remove-Item -Recurse -Force $_.FullName -ErrorAction SilentlyContinue }
