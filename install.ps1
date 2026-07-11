#!/usr/bin/env pwsh
# sonify-health quick start for Windows.  Downloads the latest released daemon
# for this machine's architecture, lays down the Star Trek bridge preset as a
# writable config, and runs it — paste-and-go, no toolchain and no choices.
# Intended to be run as `irm <url>/install.ps1 | iex`; everything lands in the
# current directory.  Piping through iex runs the text directly, so the default
# Restricted execution policy — which governs script *files* on disk, not
# expressions — never blocks it, and no administrator step is required.
#
# Runs on Windows PowerShell 5.1 (present on every Windows 10 and 11) as well as
# PowerShell 7+.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# Invoke-WebRequest renders a per-chunk progress bar that dominates its runtime
# on 5.1; silencing it makes the download return promptly.
$ProgressPreference = 'SilentlyContinue'

# Windows PowerShell 5.1 negotiates an older TLS floor on some builds, which
# GitHub rejects; pin TLS 1.2 so the API and download calls succeed there.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repo = 'LoganBarnett/sonify-health'

# Map the running architecture to a release-asset label.  PROCESSOR_ARCHITECTURE
# reads x86 under a 32-bit (WOW64) shell, so consult PROCESSOR_ARCHITEW6432,
# which the host sets to the true architecture in exactly that case.
$arch = $env:PROCESSOR_ARCHITEW6432
if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
switch ($arch) {
  'AMD64' { $label = 'x86_64-windows' }
  'ARM64' { $label = 'aarch64-windows' }
  default {
    throw "sonify-health: no prebuilt binary for architecture '$arch'.  Build from source instead — see the README."
  }
}

# Resolve the most recent release tag.  Invoke-RestMethod parses the API's JSON
# for us, so there is no grep/cut step like the shell installer needs.
$tag = (Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest").tag_name
if (-not $tag) {
  throw 'sonify-health: no published release yet — build from source (see README).'
}

# Download and unpack the daemon.  Expand-Archive ships with Windows PowerShell
# 5.1, so no external unzip is needed.
$asset = "sonify-health-server-$tag-$label.zip"
Write-Host "sonify-health: downloading $tag for $label..."
Invoke-WebRequest -Uri "https://github.com/$repo/releases/download/$tag/$asset" -OutFile $asset
Expand-Archive -LiteralPath $asset -DestinationPath . -Force
Remove-Item -LiteralPath $asset

# Lay down a working, writable config unless the directory already has one, so a
# re-run does not clobber edits.
if (-not (Test-Path -LiteralPath 'config.toml')) {
  Invoke-WebRequest `
    -Uri "https://raw.githubusercontent.com/$repo/main/examples/connectivity-and-cpu-star-trek.toml" `
    -OutFile 'config.toml'
}

Write-Host 'sonify-health: starting — Ctrl-C to stop.'
& '.\sonify-health-server.exe' --config config.toml
