# demo-apps-sdk - build ruos GUI apps as `.cwasm`

A self-contained toolkit (4 PowerShell scripts) to scaffold, build and deploy
**ruos GUI window apps** without touching the OS sources. Drop these scripts in a
folder, run `bootstrap.ps1`, and you get a complete app project. A new app shows up
in the ruos desktop launcher automatically - no kernel or Makefile change.

## Prerequisites

- **Windows + WSL** distro `Ubuntu-22.04` with the ruos toolchain (rustup nightly
  + `wasm32-wasip1` target) - the same env that builds ruos.
- **git on Windows** + network (first `bootstrap` pulls the ABI crates).
- A **ruos OS checkout** to deploy into (build the ISO). Default
  `W:\Work\GitHub\ruos`; override with `-RuosRoot`.
- Scripts are ASCII-only and run on Windows PowerShell 5.1 and PowerShell 7+.

## The 4 scripts

| Script | Does |
|--------|------|
| `bootstrap.ps1` | Scaffolds the project skeleton (workspace, config, `templates/`, `apps/`) **and** pulls+prunes the ABI crates into `vendor/`. Idempotent. |
| `new-app.ps1` | Scaffolds a new app `apps/<name>` from `templates/window-app`. |
| `build.ps1` | Compiles an app to `wasm32-wasip1` and AOT-precompiles -> `deploy/<id>.cwasm`. Auto-runs `bootstrap` if needed. |
| `deploy.ps1` | Copies `deploy/*.cwasm` into `<RuosRoot>\apps\` and runs `make iso`. |

---

## Recommended: build in a SEPARATE folder (keeps the SDK clean)

Use `-Path` to put the project (apps/, vendor/, deploy/, target/) in another
folder; the scripts stay where they are. **Set `$P` ONCE and reuse it for all four
commands in the SAME PowerShell session** - mixing folders is the #1 mistake (see
troubleshooting).

```powershell
$P = "W:\Work\GitHub\ruos-test"          # project folder, OUTSIDE the SDK
cd W:\Work\GitHub\ruos\demo-apps-sdk

.\bootstrap.ps1 -Path $P                  # scaffold + pull ABI (once)
.\new-app.ps1 -Name testapp -Id testapp -Title "Test" -Width 640 -Height 480 -Path $P
.\build.ps1  -App testapp -Id testapp -Path $P     # -> $P\deploy\testapp.cwasm
.\deploy.ps1 -Path $P -Run                         # ISO + boot QEMU -> launcher
```

After boot: desktop launcher -> **Test** -> egui window (heading + a "Click me"
counter). Everything lives in `$P`; `demo-apps-sdk\` stays just the 4 scripts.

### Verified pipeline

```
build.ps1 -Path  ->  $P\deploy\testapp.cwasm
deploy.ps1 -Path ->  <RuosRoot>\apps\testapp.cwasm  ->  make iso
                 ->  build\iso_root\bin\testapp.cwasm  ->  launcher (manifest scan)
```

---

## Example: a "Test" app, start to finish (verified)

```powershell
# 1) project folder (outside the SDK) + one session-wide variable
$P = "W:\Work\GitHub\ruos-test"
cd W:\Work\GitHub\ruos\demo-apps-sdk

# 2) scaffold the project + pull the ABI crates (once per project)
.\bootstrap.ps1 -Path $P

# 3) create the app
.\new-app.ps1 -Name testapp -Id testapp -Title "Test" -Width 640 -Height 480 -Path $P

# 4) build -> $P\deploy\testapp.cwasm
.\build.ps1 -App testapp -Id testapp -Path $P

# 5) deploy into ruos and boot (launcher -> "Test")
.\deploy.ps1 -Path $P -Run
```

Result: `testapp.cwasm` lands in `<RuosRoot>\apps\` then the ISO `/bin`; boot, open
the launcher, click **Test** -> an egui window with a heading and a "Click me"
counter.

### Customize the UI

Edit `W:\Work\GitHub\ruos-test\apps\testapp\src\lib.rs` - the egui goes inside the
`frame_once` closure. Example with a text field + button + label:

```rust
use ruos_window::{frame_once, WindowState};

ruos_window::declare_manifest!("testapp", "Test", 640, 480);
const W: u32 = 640;
const H: u32 = 480;

struct App { name: String, greets: i64 }
static mut S: Option<WindowState> = None;
static mut APP: Option<App> = None;

#[no_mangle]
pub extern "C" fn frame() {
    #[allow(static_mut_refs)]
    unsafe {
        if S.is_none()  { S = Some(WindowState::new()); }
        if APP.is_none() { APP = Some(App { name: String::new(), greets: 0 }); }
        let s = S.as_mut().unwrap();
        let app = APP.as_mut().unwrap();
        frame_once(s, "Test", W, H, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Test app");
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut app.name);
                });
                if ui.button("Greet").clicked() { app.greets += 1; }
                if !app.name.is_empty() {
                    ui.label(format!("Hello, {}! ({} greets)", app.name, app.greets));
                }
            });
        });
    }
}

#[no_mangle]
pub extern "C" fn _start() {}
```

Then rebuild + redeploy:

```powershell
.\build.ps1 -App testapp -Id testapp -Path $P
.\deploy.ps1 -Path $P -Run
```

---

## In place (no -Path)

Omit `-Path` to scaffold/build inside the SDK folder itself (simplest, but mixes
project files with the scripts):

```powershell
cd W:\Work\GitHub\ruos\demo-apps-sdk
.\bootstrap.ps1
.\new-app.ps1 -Name browser            # interactive: prompts for id/title/size
.\build.ps1 -App browser -Id browser
.\deploy.ps1 -Run
```

## Parameters

```powershell
.\bootstrap.ps1 [-Path <dir>] [-Update]
.\new-app.ps1 -Name <kebab> [-Id <stem>] [-Title <str>] [-Width <n>] [-Height <n>] [-Path <dir>]
.\build.ps1   -App <name>  -Id <stem>  [-Path <dir>] [-RuosRoot <path>] [-Distro <wsl-distro>]
.\deploy.ps1  [-Path <dir>] [-RuosRoot <path>] [-Run] [-Distro <wsl-distro>]
```

- **`-Path`** = PROJECT root to scaffold/build into. Default = the scripts' folder.
  Created if missing. **Use the SAME `-Path` for all four** (build.ps1 passes it to
  bootstrap automatically).
- **`-Id` MUST equal the `.cwasm` stem** = the launcher spawn key
  (`declare_manifest!("<id>", ...)`). Default `= Name`.
- **`-App`** = the app's package/dir name under `apps/` (kebab-case).
- **`-RuosRoot`** = path to the ruos OS checkout (default `W:\Work\GitHub\ruos`) -
  needed for `tools/wt-precompile` and `make iso`.
- **`-Run`** (deploy) = also boot QEMU with a display (needs WSLg on Win11).
- **`-Distro`** = WSL distro (default `Ubuntu-22.04`).
- **`-Update`** (bootstrap) = re-clone + re-prune the vendored ABI crates.

## Edit, rebuild, more apps

```powershell
# edit the UI: apps\<name>\src\lib.rs  (egui inside the frame_once closure)
.\build.ps1 -App testapp -Id testapp -Path $P     # rebuild after editing
.\deploy.ps1 -Path $P -Run

# a second app (workspace picks it up via the apps/* glob - no manifest edit)
.\new-app.ps1 -Name notes -Title "Notes" -Width 560 -Height 420 -Path $P
.\build.ps1 -App notes -Id notes -Path $P
.\deploy.ps1 -Path $P

.\bootstrap.ps1 -Path $P -Update                  # refresh vendored ABI later
```

## Host API manual (`api/`)

`bootstrap` copies the OS's host-API manual (`<RuosRoot>\docs\api\`) into the
project as **`api/`** (refreshed every run) - one page per import module, crate-docs
style:

- `api/wm.md`, `api/sys.md`, `api/term.md` - GUI window apps (what the SDK builds).
- `api/ruos.md`, `api/wasi.md` - CLI tools.
- `api/wit.md` - the component-model bridge.
- `api/README.md` - index + conventions.

It travels offline with an independent project. The OS keeps it current via a
`CLAUDE.md` rule (every new app-facing host fn updates the matching page). Most GUI
apps only need `ruos-window` (`frame_once` + egui); reach for the raw modules
(`wm.spawn`, `term`, `sys` telemetry) when needed.

## How it works

- **ABI**: `bootstrap` shallow-clones `ruos-desktop-ui` and prunes it to
  `crates/{gui-core,ruos-window}` (the window SDK + portable egui raster) under
  `vendor/`. That is all `cargo build` needs.
- **wt-precompile**: AOT-compiles the `.wasm` to a `.cwasm` whose Wasmtime tunables
  must match the kernel - that is why it comes from the ruos checkout (`-RuosRoot`),
  not a vendored copy.
- **Deploy**: the `.cwasm` is copied to `<RuosRoot>\apps\`. The OS Makefile has a
  generic hook that bundles `apps\*.cwasm` into the ISO `/bin`; the compositor
  scans `/bin` for `manifest()` exports, so the app appears in the launcher.
- **Runtime drop (no rebuild)**: you can also copy a `.cwasm` into the FAT data
  partition's `/mnt/apps/` on a running system; the launcher rescans within ~1s.
  `/bin` wins on name clashes.

## Layout (after bootstrap, under -Path)

```
<project>/
  Cargo.toml             workspace (members glob apps/*, excludes vendor)
  .cargo/config.toml     wasm stack + initial-memory link flags
  rust-toolchain.toml    nightly pinned to the kernel
  templates/window-app/  app skeleton (new-app copies from here)
  apps/<name>/           your apps  (apps/<name>/src/lib.rs = your UI)
  vendor/ruos-desktop/   pulled ABI crates: crates/{gui-core,ruos-window}
  deploy/<id>.cwasm      build output
  target/                cargo build dir
```

## App anatomy

`apps/<name>/src/lib.rs` is a `cdylib` exporting three symbols:

- `manifest() -> i64` via `ruos_window::declare_manifest!("<id>", "<Title>", W, H)`
  - the launcher entry. **id = the `.cwasm` stem = the spawn key.**
- `frame()` - called by the compositor each frame; build egui UI inside the
  `frame_once(...)` closure.
- `_start()` - wasip1 reactor init (empty).

Draw plain egui in the closure, or pull in `gui-core` widgets/DeskApps.

## Troubleshooting

- **`no .cwasm in deploy - run .\build.ps1 first`** - almost always `$P` not set
  (or different) between `build` and `deploy`, so they used different folders. Set
  `$P` once and pass the SAME `-Path` to every command in the SAME session. Check:
  `Test-Path "$P\deploy\*.cwasm"`.
- **`templates\window-app missing`** - run `.\bootstrap.ps1 -Path $P` first.
- **`wt-precompile not found under -RuosRoot`** - pass a valid ruos checkout:
  `... -RuosRoot D:\src\ruos`.
- **Parse errors / weird tokens in a `.ps1`** - the file picked up a non-ASCII
  character (e.g. a smart dash) and Windows PowerShell 5.1 read it as cp1252. Keep
  the scripts ASCII-only.
- **`-Run` shows no window** - needs WSLg (Win11). Otherwise deploy without `-Run`
  (builds the ISO), then `make run` from the ruos repo, or run QEMU on the ISO.
- **Glyphs garbled / raster mismatch** - the vendored `ruos-desktop` HEAD drifted
  from your kernel's submodule commit; check out a matching commit in
  `vendor\ruos-desktop`, or rebuild ruos against `ruos-desktop` HEAD.
