<#
.SYNOPSIS
  Deploy built .cwasm app(s) into a ruos OS checkout and rebuild the ISO.

.DESCRIPTION
  Copies every deploy/*.cwasm into <RuosRoot>/apps/ (the OS prebuilt-app drop
  folder), then runs `make iso` there. The Makefile's generic hook bundles
  apps/*.cwasm into /bin; the compositor's manifest() scan makes them appear in
  the launcher - no kernel or per-app Makefile change. Pass -Run to also boot QEMU.

.PARAMETER RuosRoot
  Path to the ruos OS checkout to deploy into. Default W:\Work\GitHub\ruos.

.EXAMPLE
  .\deploy.ps1
  .\deploy.ps1 -Run
  .\deploy.ps1 -RuosRoot D:\src\ruos
#>
param(
  # Project root (where deploy/*.cwasm was produced). Default = this script's folder.
  [string]$Path     = $PSScriptRoot,
  [string]$RuosRoot = "W:\Work\GitHub\ruos",
  [string]$Distro   = "Ubuntu-22.04",
  [switch]$Run
)
$ErrorActionPreference = "Stop"
$proj = (Resolve-Path $Path).Path

function To-Wsl([string]$p) {
  $full = (Resolve-Path $p).Path
  $drive = $full.Substring(0,1).ToLower()
  $rest = $full.Substring(2) -replace '\\','/'
  return "/mnt/$drive$rest"
}

$cwasm = Get-ChildItem (Join-Path $proj "deploy") -Filter *.cwasm -ErrorAction SilentlyContinue
if (-not $cwasm) { throw "no .cwasm in deploy - run .\build.ps1 first" }

$appsDir = Join-Path $RuosRoot "apps"
if (-not (Test-Path $appsDir)) { throw "drop folder not found: '$appsDir' (is -RuosRoot a ruos checkout with apps/?)" }
Copy-Item $cwasm.FullName -Destination $appsDir -Force
Write-Host ("dropped into $appsDir : " + ($cwasm.Name -join ", ")) -ForegroundColor Cyan

$ruos = To-Wsl $RuosRoot
$target = if ($Run) { "run" } else { "iso" }
wsl -d $Distro -u root -e bash -lc "set -e; cd '$ruos'; make $target"
if ($LASTEXITCODE -ne 0) { throw "make $target failed (exit $LASTEXITCODE)" }
Write-Host "OK: bundled into ruos /bin (appears in the launcher on next boot)" -ForegroundColor Green
