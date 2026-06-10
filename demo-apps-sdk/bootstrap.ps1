<#
.SYNOPSIS
  Initialize a ruos app project: scaffold the workspace skeleton (if missing) and
  pull the minimal ABI crates. Run this first in any folder that has these scripts.

.DESCRIPTION
  1) Scaffolds (only if absent - never overwrites): Cargo.toml (workspace, members
     glob apps/*), .cargo/config.toml, rust-toolchain.toml, .gitignore,
     templates/window-app/ (the app skeleton), apps/ (empty).
  2) Shallow-clones ruos-desktop-ui into vendor/ruos-desktop and prunes it to just
     crates/{gui-core,ruos-window} - all `cargo build` needs.

  After this: .\new-app.ps1 -Name <name>  then  .\build.ps1 / .\deploy.ps1.
  Idempotent. -Update re-clones + re-prunes the vendored crates.

.EXAMPLE
  .\bootstrap.ps1
  .\bootstrap.ps1 -Update
#>
param(
  # Project root to scaffold/build into. Default = this script's folder (run in
  # place). Pass -Path to target another folder (created if missing).
  [string]$Path = $PSScriptRoot,
  # ruos OS checkout - used to copy docs\app-api.md into the project as APP-API.md.
  [string]$RuosRoot = "W:\Work\GitHub\ruos",
  [string]$DesktopUrl = "https://github.com/gidixi/ruos-desktop-ui.git",
  [switch]$Update
)
$ErrorActionPreference = "Stop"
if (-not (Test-Path $Path)) { New-Item -ItemType Directory -Force -Path $Path | Out-Null }
$root = (Resolve-Path $Path).Path

# --- write a file only if it doesn't exist (never clobber user edits) --------
function New-IfMissing([string]$rel, [string]$content) {
  $p = Join-Path $root $rel
  if (Test-Path $p) { return }
  $dir = Split-Path $p -Parent
  if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
  Set-Content -Path $p -Value $content -NoNewline
  Write-Host "  + $rel" -ForegroundColor DarkGreen
}

function Scaffold {
  Write-Host "scaffolding project skeleton (missing files only) ..." -ForegroundColor Cyan

  New-IfMissing "Cargo.toml" @'
# Workspace for ruos GUI window apps (.cwasm). Apps live in apps/ (glob members).
# ABI crates are pulled into vendor/ by bootstrap.ps1 and EXCLUDED here so they
# keep their own dependency pins.
[workspace]
resolver = "2"
members = ["apps/*"]
exclude = ["vendor"]

# egui pin MUST match ruos-desktop's workspace (epaint mesh ABI stable per minor).
[workspace.dependencies]
egui = "0.31"

[profile.release]
panic = "abort"
lto = true
opt-level = "s"
'@

  New-IfMissing ".cargo/config.toml" @'
# wasm32-wasip1 link flags for ruos window apps - MUST match ruos-desktop's.
# 8 MiB shadow stack (egui + glyph raster overflow the default 1 MiB) + 48 MiB
# initial linear memory (the kernel demand-pages it, so only touched pages cost).
[target.wasm32-wasip1]
rustflags = ["-C", "link-arg=-zstack-size=8388608", "-C", "link-arg=--initial-memory=50331648"]
'@

  New-IfMissing "rust-toolchain.toml" @'
[toolchain]
channel = "nightly-2026-05-26"
components = ["rust-src"]
targets = ["wasm32-wasip1"]
'@

  New-IfMissing ".gitignore" @'
/target
/deploy/*.cwasm
/vendor/
'@

  New-IfMissing "apps/.gitkeep" ""

  New-IfMissing "templates/window-app/Cargo.toml" @'
[package]
name = "window-app"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
# Pulled by bootstrap.ps1 into vendor/. Same relative depth as apps/<name>/, so
# the path is identical after new-app.ps1 copies this template into apps/.
ruos-window = { path = "../../vendor/ruos-desktop/crates/ruos-window" }
egui = { workspace = true }
'@

  New-IfMissing "templates/window-app/src/lib.rs" @'
//! ruos GUI window app skeleton. new-app.ps1 copies this into apps/<name> and
//! rewrites the package name + manifest. Exports: manifest() (launcher entry),
//! frame() (per-frame egui UI), _start() (wasip1 reactor init).

use ruos_window::{frame_once, WindowState};

// id MUST equal the .cwasm stem (the spawn key). Rewritten by new-app.ps1.
ruos_window::declare_manifest!("window-app", "Window App", 480, 320);

const W: u32 = 480;
const H: u32 = 320;

static mut S: Option<WindowState> = None;
static mut COUNT: i64 = 0;

#[no_mangle]
pub extern "C" fn frame() {
    #[allow(static_mut_refs)]
    unsafe {
        if S.is_none() { S = Some(WindowState::new()); }
        let s = S.as_mut().unwrap();
        frame_once(s, "Window App", W, H, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Window App");
                ui.separator();
                ui.label("Edit src/lib.rs to build your UI.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Click me").clicked() { COUNT += 1; }
                    ui.label(format!("clicks: {}", COUNT));
                });
            });
        });
    }
}

#[no_mangle]
pub extern "C" fn _start() {}
'@
}

# --- vendor: shallow clone + prune to the 2 crates ---------------------------
$vendor  = Join-Path $root "vendor"
$desktop = Join-Path $vendor "ruos-desktop"

function Prune-Vendor([string]$d) {
  $kill = @("apps","assets","backends","docs","xtask","crates\ruos-assets",
            ".git",".github","CHANGELOG","Cargo.lock")
  foreach ($k in $kill) { Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $d $k) }
  Get-ChildItem $d -Recurse -Filter *.md -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
  $ct = Join-Path $d "Cargo.toml"
  $c  = Get-Content $ct -Raw
  $c  = $c -replace '(?s)members = \[[^\]]*\]', 'members = ["crates/gui-core", "crates/ruos-window"]'
  $c  = $c -replace '(?s)default-members = \[[^\]]*\]', 'default-members = ["crates/gui-core"]'
  Set-Content $ct $c -NoNewline
}

Scaffold

# Copy the host API manual into the project as api/ (refreshed every run).
$apiSrc = Join-Path $RuosRoot "docs\api"
if (Test-Path $apiSrc) {
  $apiDst = Join-Path $root "api"
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $apiDst
  Copy-Item $apiSrc $apiDst -Recurse -Force
  Write-Host "  + api/ (host API manual)" -ForegroundColor DarkGreen
} else {
  Write-Host "  (skip api/: docs\api not found under -RuosRoot '$RuosRoot')" -ForegroundColor DarkGray
}

if ((Test-Path $desktop) -and -not $Update) {
  Write-Host "vendor/ruos-desktop present (use -Update to refresh)" -ForegroundColor DarkGray
} else {
  Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $desktop
  New-Item -ItemType Directory -Force -Path $vendor | Out-Null
  Write-Host "pulling ruos-desktop (shallow) -> vendor/ruos-desktop ..." -ForegroundColor Cyan
  git clone --depth 1 $DesktopUrl $desktop
  if ($LASTEXITCODE -ne 0) { throw "git clone failed" }
  Write-Host "pruning to crates/{gui-core,ruos-window} ..." -ForegroundColor Cyan
  Prune-Vendor $desktop
}
Write-Host "OK: project ready. Next: .\new-app.ps1 -Name <name>" -ForegroundColor Green
