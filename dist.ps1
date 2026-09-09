# SPDX-License-Identifier: GPL-3.0-or-later
<#
.SYNOPSIS
  Assemble a portable Turing Smart Screen (Rust) folder: release binaries,
  resources, default config and version stamp. Replaces today's manual copy.
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File dist.ps1
  powershell -ExecutionPolicy Bypass -File dist.ps1 -OutDir C:\Deploy\Turing -Version 0.2.0
#>
param(
    [string]$OutDir = (Join-Path (Get-Location) 'dist\turing-smart-screen-rust'),
    [string]$Version = ''
)

$ErrorActionPreference = 'Stop'

if (-not $Version) {
    $m = Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    $Version = $m.Matches[0].Groups[1].Value
}
Write-Host "Building release v$Version ..."
cargo build --release --bins
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
Copy-Item target\release\turing-smart-screen.exe (Join-Path $OutDir '') -Force
Copy-Item target\release\turing-configure.exe (Join-Path $OutDir '') -Force
Copy-Item res (Join-Path $OutDir '') -Recurse -Force
if (-not (Test-Path (Join-Path $OutDir 'config.yaml'))) {
    Copy-Item config.yaml (Join-Path $OutDir '')  # never overwrite user settings
} else {
    Write-Host "Keeping existing config.yaml"
}
$Version | Out-File (Join-Path $OutDir 'version.txt') -Encoding ascii -NoNewline

Write-Host ""
Write-Host "Portable folder ready: $OutDir"
Write-Host "  1. (admin, once) install the sensor driver: external\PawnIO\PawnIO_setup.exe"
Write-Host "  2. Create the logon task (see README) or run: turing-smart-screen.exe --daemon"
