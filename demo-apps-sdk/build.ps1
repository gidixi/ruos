<#
.SYNOPSIS
  Build a ruos GUI window app to a deployable .cwasm.

.DESCRIPTION
  1) Ensures the ABI crates are present (runs bootstrap.ps1 -> vendor/ruos-desktop).
  2) Compiles <App> (cdylib in apps/) to wasm32-wasip1 in WSL.
  3) AOT-precompiles it to deploy/<Id>.cwasm using the OS repo's wt-precompile
     (its Wasmtime tunables must match the kernel that will run it - that's why we
     reuse the ruos checkout's copy rather than a divergent one).

.PARAMETER RuosRoot
  Path to a ruos OS checkout (needs tools/wt-precompile). Default W:\Work\GitHub\ruos.

.EXAMPLE
  .\build.ps1                                  # hello-window -> deploy\hello.cwasm
  .\build.ps1 -App browser -Id browser
#>
param(
  # Project root (where the app/workspace lives). Default = this script's folder.
  [string]$Path     = $PSScriptRoot,
  [string]$App      = "hello-window",
  [string]$Id       = "hello",
  [string]$RuosRoot = "W:\Work\GitHub\ruos",
  [string]$Distro   = "Ubuntu-22.04"
)
$ErrorActionPreference = "Stop"
$proj = (Resolve-Path $Path).Path

function To-Wsl([string]$p) {
  $full = (Resolve-Path $p).Path
  $drive = $full.Substring(0,1).ToLower()
  $rest = $full.Substring(2) -replace '\\','/'
  return "/mnt/$drive$rest"
}

# 1) ABI crates + skeleton (idempotent). bootstrap operates on the same -Path.
if (-not (Test-Path (Join-Path $proj "vendor\ruos-desktop"))) {
  & (Join-Path $PSScriptRoot "bootstrap.ps1") -Path $proj
}

# Resolve the OS repo (for wt-precompile).
if (-not (Test-Path (Join-Path $RuosRoot "tools\wt-precompile\Cargo.toml"))) {
  throw "wt-precompile not found under -RuosRoot '$RuosRoot' (need a ruos checkout)"
}

$sdk  = To-Wsl $proj
$ruos = To-Wsl $RuosRoot
$lib  = $App -replace '-','_'

$tpl = @'
set -e
cd 'SDK'
mkdir -p deploy
echo "== building wt-precompile (from RuosRoot) =="
cargo build --release --manifest-path 'RUOS/tools/wt-precompile/Cargo.toml'
echo "== building APP (wasm32-wasip1) =="
cargo build -p 'APP' --target wasm32-wasip1 --release
echo "== precompiling -> deploy/ID.cwasm =="
'RUOS/tools/wt-precompile/target/release/wt-precompile' target/wasm32-wasip1/release/LIB.wasm deploy/ID.cwasm
ls -la deploy/ID.cwasm
'@

$bash = $tpl.Replace('SDK',$sdk).Replace('RUOS',$ruos).Replace('APP',$App).Replace('LIB',$lib).Replace('ID',$Id)

wsl -d $Distro -u root -e bash -lc $bash
if ($LASTEXITCODE -ne 0) { throw "build failed (exit $LASTEXITCODE)" }
Write-Host "OK: deploy/$Id.cwasm  (run .\deploy.ps1 -RuosRoot '$RuosRoot' to bundle into ruos)" -ForegroundColor Green
