<#
.SYNOPSIS
  Install Pulsate (pulsate + p8) from prebuilt GitHub release binaries on Windows.
.EXAMPLE
  irm https://raw.githubusercontent.com/squaretick/pulsate/main/scripts/install.ps1 | iex
.NOTES
  Env overrides: $env:PULSATE_VERSION (e.g. v0.1.0), $env:INSTALL_DIR.
#>
$ErrorActionPreference = 'Stop'
$Repo = 'squaretick/pulsate'
$InstallDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\pulsate" }

$teal = "$([char]27)[38;2;61;163;140m"; $reset = "$([char]27)[0m"
Write-Host "$teal== Pulsate installer ==$reset"

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$target = "$arch-pc-windows-msvc"

$version = $env:PULSATE_VERSION
if (-not $version) {
  $rel = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
  $version = $rel.tag_name
}
if (-not $version) { throw "Could not resolve latest release; set PULSATE_VERSION." }

$asset = "pulsate-$version-$target.zip"
$url   = "https://github.com/$Repo/releases/download/$version/$asset"
$tmp   = Join-Path $env:TEMP "pulsate-$version"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$zip = Join-Path $tmp $asset

Write-Host "> downloading $asset"
Invoke-WebRequest -Uri $url -OutFile $zip

Write-Host "> verifying checksum"
try {
  Invoke-WebRequest -Uri "$url.sha256" -OutFile "$zip.sha256"
  $expected = ((Get-Content "$zip.sha256") -split '\s+')[0]
  $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
  if ($expected.ToLower() -ne $actual) { throw "checksum mismatch (expected $expected, got $actual)" }
  Write-Host "  checksum verified"
} catch { throw "checksum verification failed: $_" }

Expand-Archive -Path $zip -DestinationPath $tmp -Force
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
foreach ($bin in 'pulsate.exe','p8.exe') {
  $src = Get-ChildItem -Path $tmp -Recurse -Filter $bin -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($src) { Copy-Item $src.FullName (Join-Path $InstallDir $bin) -Force; Write-Host "  installed $bin -> $InstallDir" }
}

if (";$env:Path;" -notlike "*;$InstallDir;*") {
  Write-Host "$teal! $reset Add $InstallDir to your PATH:"
  Write-Host "    setx PATH `"$InstallDir;`$env:PATH`""
}
Write-Host "Done. Try: pulsate --version"
