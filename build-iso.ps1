<#
.SYNOPSIS
    Build the ruos ISO from Windows, compiling every crate and submodule via WSL.

.DESCRIPTION
    ruos builds Linux-native; on Windows the toolchain lives in WSL. This script
    drives a full, from-clean-capable build inside WSL:

      1. ensure the ruos-desktop submodule (egui UI -> gui.cwasm): clones it on
         first run; an EXISTING checkout — and your local edits/commits in it —
         is left untouched unless you pass -SyncSubmodule;
      2. ensure the rustup wasm targets (wasm32-wasip1, wasm32-unknown-unknown);
      3. pre-build the kernel-embedded .cwasm guests (reactor / reactor_close /
         probe) BEFORE the kernel, because `make iso` compiles the kernel — which
         `include_bytes!`s them — before its sibling .cwasm prerequisites;
      4. `make iso` (compiles the kernel, all user/ WASI tools, the egui desktop
         from the submodule, the AOT precompiler, and assembles build/os.iso).

    With -BootChecks the extra boot-check guests are built too, including
    bringup.cwasm which needs the `wasm-tools` CLI (auto-installed if missing).

.PARAMETER Distro
    WSL distro name. Default: Ubuntu-22.04.

.PARAMETER Features
    Cargo features passed to the kernel build (CARGO_FEATURES). Free-form.

.PARAMETER BootChecks
    Shortcut for -Features boot-checks (in-boot self-tests). Also builds the
    boot-check .cwasm guests + bringup.cwasm (needs wasm-tools).

.PARAMETER UsbProbe
    Shortcut for -Features usb-probe (serial-less real-HW USB triage build).

.PARAMETER Clean
    Run `make clean` first for a fresh build (does NOT remove the cached Limine
    clone in third_party/).

.PARAMETER SyncSubmodule
    Reset the ruos-desktop submodule to the commit pinned by the superproject.
    DISCARDS local commits/edits in the submodule — use only when you intend to
    match the pinned version. Default: an existing submodule checkout is left
    as-is so your in-progress UI work builds into the image.

.EXAMPLE
    .\build-iso.ps1
    .\build-iso.ps1 -UsbProbe
    .\build-iso.ps1 -BootChecks -Clean
    .\build-iso.ps1 -Features "usb-probe panic-halt"
#>
[CmdletBinding()]
param(
    [string]$Distro = "Ubuntu-22.04",
    [string]$Features = "",
    [switch]$BootChecks,
    [switch]$UsbProbe,
    [switch]$Clean,
    [switch]$SyncSubmodule
)

$ErrorActionPreference = "Stop"

# --- Resolve features ------------------------------------------------------
$featList = @()
if ($Features)   { $featList += $Features }
if ($BootChecks) { $featList += "boot-checks" }
if ($UsbProbe)   { $featList += "usb-probe" }
$FeatureStr = ($featList -join " ").Trim()

# --- Windows repo path -> WSL path (the script lives in the repo root) -----
function Convert-ToWslPath([string]$winPath) {
    $p = $winPath -replace '\\', '/'
    if ($p -match '^([A-Za-z]):/(.*)$') {
        return "/mnt/$($matches[1].ToLower())/$($matches[2])"
    }
    return $p
}
$RepoWin = $PSScriptRoot
$RepoWsl = Convert-ToWslPath $RepoWin

Write-Host "ruos ISO build" -ForegroundColor Cyan
Write-Host "  distro   : $Distro"
Write-Host "  repo     : $RepoWin"
Write-Host "  repo(wsl): $RepoWsl"
Write-Host "  features : $(if ($FeatureStr) { $FeatureStr } else { '(none)' })"
Write-Host "  clean    : $Clean"
Write-Host ""

# --- The build, run inside WSL --------------------------------------------
# Single-quoted here-string: bash $vars stay literal; __PLACEHOLDERS__ are
# substituted from PowerShell below.
$bash = @'
set -euo pipefail
cd "__REPO__"
FEATURES="__FEATURES__"
DO_CLEAN="__CLEAN__"

echo "==> [1/5] git submodules (ruos-desktop -> gui.cwasm)"
git config --global --add safe.directory '*' >/dev/null 2>&1 || true
SYNC_SUB="__SYNC__"
if [ ! -e ruos-desktop/Cargo.toml ]; then
  echo "    ruos-desktop not initialized — cloning"
  git submodule update --init --recursive
elif [ "$SYNC_SUB" = "True" ]; then
  echo "    -SyncSubmodule: resetting ruos-desktop to the pinned commit (DISCARDS local submodule changes!)"
  git submodule update --init --recursive --force
else
  echo "    ruos-desktop already present — leaving its checkout as-is"
  echo "    (your local commits / edits are kept; pass -SyncSubmodule to reset to the pinned commit)"
fi

echo "==> [2/5] cargo env + wasm targets"
if [ -f "$HOME/.cargo/env" ]; then source "$HOME/.cargo/env"; fi
rustup target add wasm32-wasip1 wasm32-unknown-unknown >/dev/null

if [ "$DO_CLEAN" = "True" ]; then
  echo "==> make clean"
  make clean || true
fi

echo "==> [3/5] kernel-embedded .cwasm guests (build-order safe)"
# The kernel include_bytes!'s these unconditionally; build them before `make iso`
# compiles the kernel (the Makefile lists them as siblings to the right of the
# kernel target, so a from-clean `make iso` would compile the kernel first).
make kernel/src/wasm/wt/reactor.cwasm \
     kernel/src/wasm/wt/reactor_close.cwasm \
     kernel/src/wasm/wt/probe.cwasm

if echo " $FEATURES " | grep -q " boot-checks "; then
  echo "==> [3b] boot-check guests + bringup.cwasm (needs wasm-tools)"
  if ! command -v wasm-tools >/dev/null 2>&1; then
    echo "    wasm-tools not found — installing (cargo install wasm-tools, slow)…"
    cargo install wasm-tools
  fi
  make kernel/src/wasm/wt/hello.cwasm \
       kernel/src/wasm/wt/gfxtest.cwasm \
       kernel/src/wasm/wt/echo.cwasm \
       kernel/src/wasm/wt/cat.cwasm \
       kernel/src/wasm/wt/bringup.cwasm
fi

echo "==> [4/5] make iso"
if [ -n "$FEATURES" ]; then
  make iso CARGO_FEATURES="$FEATURES"
else
  make iso
fi

echo "==> [5/5] done"
ls -la --time-style=+%Y-%m-%d_%H:%M:%S build/os.iso
'@

$bash = $bash.Replace('__REPO__', $RepoWsl).
              Replace('__FEATURES__', $FeatureStr).
              Replace('__CLEAN__', [string]$Clean).
              Replace('__SYNC__', [string]$SyncSubmodule)

$sw = [System.Diagnostics.Stopwatch]::StartNew()
& wsl.exe -d $Distro -u root -e bash -lc $bash
$code = $LASTEXITCODE
$sw.Stop()

Write-Host ""
if ($code -eq 0 -and (Test-Path (Join-Path $RepoWin "build\os.iso"))) {
    $iso = Get-Item (Join-Path $RepoWin "build\os.iso")
    $mb  = [math]::Round($iso.Length / 1MB, 1)
    Write-Host "BUILD OK  ($([int]$sw.Elapsed.TotalSeconds)s)" -ForegroundColor Green
    Write-Host "  ISO: $($iso.FullName)  ($mb MB)"
    Write-Host "  Run: wsl -d $Distro -u root -e bash -lc 'cd $RepoWsl && make run'        # QEMU window"
    Write-Host "       wsl -d $Distro -u root -e bash -lc 'cd $RepoWsl && make run-test'   # headless smoke"
} else {
    Write-Host "BUILD FAILED (exit $code)" -ForegroundColor Red
    exit ($(if ($code -ne 0) { $code } else { 1 }))
}
