<#
.SYNOPSIS
    Build the ruos ISO from Windows, compiling every crate and submodule via WSL.

.DESCRIPTION
    ruos builds Linux-native; on Windows the toolchain lives in WSL. This script
    drives a full, from-clean-capable build inside WSL and is meant to work on a
    *fresh* machine — it provisions anything that is missing:

      0. locate a usable WSL distro: if -Distro is omitted (or names a distro
         that isn't installed) one is auto-selected, preferring Ubuntu/Debian and
         skipping docker-desktop / rancher-desktop helper distros;
      1. ensure system build deps via apt (build-essential, xorriso, mtools,
         qemu-system-x86, python3, curl, git) — only the missing ones are
         installed;
      2. ensure the Rust toolchain: install rustup non-interactively if absent,
         then add the wasm targets (wasm32-wasip1, wasm32-unknown-unknown). The
         pinned nightly + components come from rust-toolchain.toml automatically;
      3. ensure the ruos-desktop submodule (egui UI -> *.cwasm app windows):
         clones it on first run; an EXISTING checkout — and your local
         edits/commits in it — is left untouched unless you pass -SyncSubmodule;
      4. pre-build the kernel-embedded .cwasm guests (reactor / reactor_close /
         probe / egui_demo) BEFORE the kernel, because `make iso` compiles the
         kernel — which `include_bytes!`s them — before its sibling .cwasm
         prerequisites, so a from-clean `make iso` would otherwise fail;
      5. `make iso` (compiles the kernel, all user/ WASI tools, the egui desktop
         from the submodule, the AOT precompiler, and assembles build/os.iso).

    With -BootChecks the extra boot-check guests are built too, including
    bringup.cwasm which needs the `wasm-tools` CLI (auto-installed if missing).

.PARAMETER Distro
    WSL distro name. Default: auto-detect (prefers an installed Ubuntu, then
    Debian, then the first non-helper distro). Pass an explicit name to override.

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

.PARAMETER SkipDeps
    Skip the apt system-dependency check/install step (step 1). Use when you
    know the distro is already provisioned and want a faster start, or when you
    lack sudo/root for apt.

.EXAMPLE
    .\build-iso.ps1
    .\build-iso.ps1 -Distro Ubuntu
    .\build-iso.ps1 -UsbProbe
    .\build-iso.ps1 -BootChecks -Clean
    .\build-iso.ps1 -Features "usb-probe panic-halt"
#>
[CmdletBinding()]
param(
    [string]$Distro = "",
    [string]$Features = "",
    [switch]$BootChecks,
    [switch]$UsbProbe,
    [switch]$Clean,
    [switch]$SyncSubmodule,
    [switch]$SkipDeps
)

$ErrorActionPreference = "Stop"

# --- Resolve features ------------------------------------------------------
$featList = @()
if ($Features)   { $featList += $Features }
if ($BootChecks) { $featList += "boot-checks" }
if ($UsbProbe)   { $featList += "usb-probe" }
$FeatureStr = ($featList -join " ").Trim()

# --- Locate WSL + pick a usable distro -------------------------------------
if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
    Write-Host "WSL not found. Install it with:  wsl --install" -ForegroundColor Red
    Write-Host "then reboot and re-run this script." -ForegroundColor Red
    exit 1
}

function Get-WslDistros {
    # wsl.exe emits UTF-16LE; force the console encoding so we read it cleanly,
    # then strip any stray non-printable bytes and blank lines.
    $prev = [Console]::OutputEncoding
    try { [Console]::OutputEncoding = [System.Text.Encoding]::Unicode } catch {}
    try   { $raw = & wsl.exe --list --quiet 2>$null }
    finally { try { [Console]::OutputEncoding = $prev } catch {} }
    return @($raw | ForEach-Object { ($_ -replace '[^\x20-\x7E]', '').Trim() } | Where-Object { $_ })
}

$allDistros = Get-WslDistros
if (-not $allDistros -or $allDistros.Count -eq 0) {
    Write-Host "No WSL distro installed. Install one, e.g.:  wsl --install -d Ubuntu" -ForegroundColor Red
    exit 1
}

if ($Distro -and ($allDistros -notcontains $Distro)) {
    Write-Warning "Requested distro '$Distro' is not installed. Installed: $($allDistros -join ', ')"
    $Distro = ""
}
if (-not $Distro) {
    $candidates = @($allDistros | Where-Object { $_ -notmatch '^docker-desktop' -and $_ -notmatch '^rancher' })
    $Distro = ($candidates | Where-Object { $_ -match 'Ubuntu' } | Select-Object -First 1)
    if (-not $Distro) { $Distro = ($candidates | Where-Object { $_ -match 'Debian' } | Select-Object -First 1) }
    if (-not $Distro) { $Distro = ($candidates | Select-Object -First 1) }
    if (-not $Distro) {
        Write-Host "No suitable Linux distro found (only helper distros present): $($allDistros -join ', ')" -ForegroundColor Red
        Write-Host "Install one with:  wsl --install -d Ubuntu" -ForegroundColor Red
        exit 1
    }
    Write-Host "Auto-selected WSL distro: $Distro" -ForegroundColor DarkCyan
}

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
Write-Host "  skipDeps : $SkipDeps"
Write-Host ""

# --- The build, run inside WSL --------------------------------------------
# Single-quoted here-string: bash $vars stay literal; __PLACEHOLDERS__ are
# substituted from PowerShell below.
$bash = @'
set -euo pipefail
cd "__REPO__"
FEATURES="__FEATURES__"
DO_CLEAN="__CLEAN__"
SYNC_SUB="__SYNC__"
SKIP_DEPS="__SKIPDEPS__"

echo "==> [1/5] system build dependencies"
if [ "$SKIP_DEPS" = "True" ]; then
  echo "    -SkipDeps: skipping apt check"
elif command -v apt-get >/dev/null 2>&1; then
  # cmd:apt-package pairs — install only what's actually missing.
  NEED=""
  for pair in xorriso:xorriso mtools:mtools qemu-system-x86_64:qemu-system-x86 \
              gcc:build-essential make:build-essential python3:python3 \
              curl:curl git:git; do
    cmd="${pair%%:*}"; pkg="${pair##*:}"
    command -v "$cmd" >/dev/null 2>&1 || NEED="$NEED $pkg"
  done
  NEED="$(printf '%s\n' $NEED | sort -u | tr '\n' ' ')"
  if [ -n "${NEED// /}" ]; then
    echo "    installing:$NEED"
    export DEBIAN_FRONTEND=noninteractive
    SUDO=""; [ "$(id -u)" -eq 0 ] || SUDO="sudo"
    $SUDO apt-get update -qq
    $SUDO apt-get install -y -qq $NEED
  else
    echo "    all present (xorriso, mtools, qemu, build-essential, python3, curl, git)"
  fi
else
  echo "    WARNING: non-apt distro — ensure xorriso, mtools, qemu, gcc, make, python3 are installed"
fi

echo "==> [2/5] Rust toolchain + wasm targets"
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
if ! command -v rustup >/dev/null 2>&1; then
  echo "    rustup not found — installing (non-interactive)…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --no-modify-path
  source "$HOME/.cargo/env"
fi
# The pinned nightly + components (rust-src, llvm-tools-preview) come from
# rust-toolchain.toml automatically on first use; targets must be explicit.
rustup target add wasm32-wasip1 wasm32-unknown-unknown

echo "==> [3/5] git submodule (ruos-desktop -> egui app .cwasm)"
git config --global --add safe.directory '*' >/dev/null 2>&1 || true
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

if [ "$DO_CLEAN" = "True" ]; then
  echo "==> make clean"
  make clean || true
fi

echo "==> [4/5] kernel-embedded .cwasm guests (build-order safe)"
# The kernel include_bytes!'s these four unconditionally; build them before
# `make iso` compiles the kernel (the Makefile lists them as siblings to the
# right of the kernel target, so a from-clean `make iso` would compile the
# kernel first and fail on the missing files). egui_demo.cwasm needs the
# ruos-desktop submodule, hence after step 3.
make kernel/src/wasm/wt/reactor.cwasm \
     kernel/src/wasm/wt/reactor_close.cwasm \
     kernel/src/wasm/wt/probe.cwasm \
     kernel/src/wasm/wt/egui_demo.cwasm

if echo " $FEATURES " | grep -q " boot-checks "; then
  echo "==> [4b] boot-check guests + bringup.cwasm (needs wasm-tools)"
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

echo "==> [5/5] make iso"
if [ -n "$FEATURES" ]; then
  make iso CARGO_FEATURES="$FEATURES"
else
  make iso
fi

echo "==> done"
ls -la --time-style=+%Y-%m-%d_%H:%M:%S build/os.iso
'@

$bash = $bash.Replace('__REPO__', $RepoWsl).
              Replace('__FEATURES__', $FeatureStr).
              Replace('__CLEAN__', [string]$Clean).
              Replace('__SYNC__', [string]$SyncSubmodule).
              Replace('__SKIPDEPS__', [string]$SkipDeps)

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
