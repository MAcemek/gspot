<#
.SYNOPSIS
  Builds the signed GSpot Windows installer and the updater manifest (latest.json).

.DESCRIPTION
  Run from anywhere (takes ~15 minutes). Reads the version from src-tauri/tauri.conf.json,
  signs the build with the updater private key, runs `pnpm build` with
  bundle.createUpdaterArtifacts turned on, then writes latest.json next to the installer.
  Upload all three printed files to the GitHub release tagged v<version>-windows.

.PARAMETER KeyPath
  Updater private key file. Default: $env:USERPROFILE\.tauri\gspot-updater.key

.PARAMETER Notes
  Release notes shown in the update prompt.
#>
param(
  [string]$KeyPath = (Join-Path $env:USERPROFILE ".tauri\gspot-updater.key"),
  [string]$Notes = "See the release notes on GitHub."
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$version = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
if (-not (Test-Path $KeyPath)) { throw "Updater key not found: $KeyPath" }

# The key travels as the file's contents; the key has an empty password.
$env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $KeyPath -Raw).Trim()
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""

# Root `build` is `cd src-tauri && tauri build`, so extra args land on `tauri build`.
Push-Location $root
try {
  pnpm build -- --config '{\"bundle\":{\"createUpdaterArtifacts\":true}}'
  if ($LASTEXITCODE -ne 0) { throw "pnpm build failed ($LASTEXITCODE)" }
} finally { Pop-Location }

# Cargo target dir: .cargo/config.toml build.target-dir, else src-tauri/target.
$targetDir = Join-Path $root "src-tauri\target"
$cfg = Join-Path $root ".cargo\config.toml"
if (Test-Path $cfg) {
  $m = Select-String -Path $cfg -Pattern '^\s*target-dir\s*=\s*"([^"]+)"' | Select-Object -First 1
  if ($m) { $targetDir = $m.Matches[0].Groups[1].Value }
}
$nsis = Join-Path $targetDir "release\bundle\nsis"
$exe = Join-Path $nsis "GSpot_${version}_x64-setup.exe"
$sig = "$exe.sig"
if (-not (Test-Path $exe)) { throw "Installer not found: $exe" }
if (-not (Test-Path $sig)) { throw "Signature not found: $sig" }

$manifest = [ordered]@{
  version   = $version
  notes     = $Notes
  pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
  platforms = [ordered]@{
    "windows-x86_64" = [ordered]@{
      signature = (Get-Content $sig -Raw).Trim()
      url       = "https://github.com/MAcemek/gspot/releases/download/v$version-windows/GSpot_${version}_x64-setup.exe"
    }
  }
}
$latest = Join-Path $nsis "latest.json"
[System.IO.File]::WriteAllText($latest, ($manifest | ConvertTo-Json -Depth 5), (New-Object System.Text.UTF8Encoding($false)))

Write-Host "Upload these to the v$version-windows release:"
Write-Host "  $exe"
Write-Host "  $sig"
Write-Host "  $latest"
