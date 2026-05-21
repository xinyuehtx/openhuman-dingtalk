#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Fork-release helper for openhuman-dingtalk on Windows x64.

.DESCRIPTION
  Builds an UNSIGNED Windows x64 bundle (msi + nsis exe) and uploads
  the installers to a GitHub Release on this fork. Run on a Windows
  x64 machine. macOS arm64 builds: see scripts/release-fork.sh.

  Requirements:
    - Windows x64
    - cargo + cargo-tauri (vendored CEF-aware CLI; pnpm dev:app installs it)
    - gh (GitHub CLI) authenticated against the fork
    - pnpm

.EXAMPLE
  .\scripts\release-fork.ps1
  .\scripts\release-fork.ps1 -DryRun
  .\scripts\release-fork.ps1 -Tag v0.55.0
  .\scripts\release-fork.ps1 -SkipBuild
#>

param(
  [switch]$DryRun,
  [switch]$SkipBuild,
  [string]$Tag,
  [string]$Notes
)

$ErrorActionPreference = "Stop"

$Repo = if ($env:RELEASE_FORK_REPO) { $env:RELEASE_FORK_REPO } else { "xinyuehtx/openhuman-dingtalk" }

if ($env:OS -ne "Windows_NT") {
  Write-Error "This script targets Windows x64. For macOS arm64, run scripts/release-fork.sh."
  exit 1
}

foreach ($cmd in @("gh", "cargo", "pnpm")) {
  if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
    Write-Error "Missing required command: $cmd"
    exit 1
  }
}

$pkg = Get-Content -Raw -Path "app\package.json" | ConvertFrom-Json
$version = $pkg.version
if (-not $version) {
  Write-Error "Could not read version from app\package.json"
  exit 1
}

if (-not $Tag) { $Tag = "v$version" }
$Target = "x86_64-pc-windows-msvc"
$BundleDir = "target\$Target\release\bundle"

Write-Host "[release-fork] repo=$Repo version=$version tag=$Tag target=$Target"

if ($DryRun) {
  Write-Host "[release-fork] DRY RUN - skipping cargo build and gh upload"
  & gh auth status --hostname github.com *> $null
  if ($LASTEXITCODE -eq 0) {
    Write-Host "[release-fork] gh auth: ok"
  } else {
    Write-Host "[release-fork] gh auth: NOT authenticated - run 'gh auth login' before a real release"
  }
  Write-Host "[release-fork] would build: cargo tauri build --target $Target --bundles msi nsis"
  Write-Host "[release-fork] would look for artifacts under $BundleDir\msi and $BundleDir\nsis"
  Write-Host "[release-fork] would upload to: $Repo@$Tag"
  exit 0
}

if (-not $SkipBuild) {
  Write-Host "[release-fork] running cargo tauri build (unsigned, msi + nsis)"
  Push-Location app
  try { pnpm tauri:ensure } finally { Pop-Location }
  & cargo tauri build `
    --config app/src-tauri/tauri.conf.json `
    --target $Target `
    --bundles msi nsis
  if ($LASTEXITCODE -ne 0) { Write-Error "cargo tauri build failed"; exit 1 }
} else {
  Write-Host "[release-fork] -SkipBuild set; using existing artifacts under $BundleDir"
}

$msi = Get-ChildItem -Path "$BundleDir\msi" -Filter "OpenHuman_*_x64*.msi" -ErrorAction SilentlyContinue | Select-Object -First 1
$exe = Get-ChildItem -Path "$BundleDir\nsis" -Filter "OpenHuman_*_x64-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1

$artifacts = @()
if ($msi) { $artifacts += $msi.FullName }
if ($exe) { $artifacts += $exe.FullName }

if ($artifacts.Count -eq 0) {
  Write-Error "Could not find any OpenHuman_*_x64*.msi or OpenHuman_*_x64-setup.exe under $BundleDir"
  exit 1
}

foreach ($path in $artifacts) {
  $hash = (Get-FileHash -Path $path -Algorithm SHA256).Hash.ToLowerInvariant()
  Write-Host "[release-fork] artifact: $(Split-Path -Leaf $path) ($hash)"
}

# Create release if missing.
$exists = $true
& gh release view $Tag --repo $Repo *> $null
if ($LASTEXITCODE -ne 0) { $exists = $false }

if (-not $exists) {
  Write-Host "[release-fork] creating draft release $Tag on $Repo"
  $body = if ($Notes) { $Notes } else { "Fork build of OpenHuman $version. Unsigned Windows x64 installers." }
  & gh release create $Tag --repo $Repo --title "OpenHuman 钉钉 $version" --notes $body --draft
  if ($LASTEXITCODE -ne 0) { Write-Error "gh release create failed"; exit 1 }
} else {
  Write-Host "[release-fork] reusing existing release $Tag on $Repo"
}

foreach ($path in $artifacts) {
  $name = Split-Path -Leaf $path
  Write-Host "[release-fork] uploading $name"
  & gh release upload $Tag $path --repo $Repo --clobber
  if ($LASTEXITCODE -ne 0) { Write-Error "gh release upload failed for $name"; exit 1 }

  # Sidecar sha256 for integrity verification.
  $hash = (Get-FileHash -Path $path -Algorithm SHA256).Hash.ToLowerInvariant()
  $shaFile = "$path.sha256"
  "$hash  $name" | Out-File -FilePath $shaFile -Encoding ascii -NoNewline
  & gh release upload $Tag $shaFile --repo $Repo --clobber | Out-Null
}

Write-Host "[release-fork] done."
Write-Host "[release-fork] release page: https://github.com/$Repo/releases/tag/$Tag"
Write-Host "[release-fork] note: release is still a DRAFT - publish via the GitHub UI or:"
Write-Host "    gh release edit $Tag --repo $Repo --draft=false"
