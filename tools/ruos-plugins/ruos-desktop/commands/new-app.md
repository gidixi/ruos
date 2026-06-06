---
description: Scaffold a new ruos desktop app (DeskApp + crate + full wiring)
argument-hint: <id> [Title] [WxH]
---

Scaffold a brand-new desktop app for ruos end-to-end, following
`ruos-desktop/docs/adding-an-app.md`. Work from the ruos repo root; the submodule
is at `ruos-desktop/`.

## Arguments

Parse `$ARGUMENTS`:
- `$1` = **id** (required): lowercase, kebab-case, e.g. `notes`. This is the
  `.cwasm`/spawn id, the DeskApp `id()`, the `CATALOG` id, and the crate base name
  (`<id>-app`). The wasm output is `<id>_app.wasm` (hyphen→underscore).
- `$2` = **Title** (optional): display name (titlebar/launcher/taskbar). Default =
  `$1` with the first letter upper-cased.
- `$3` = **WxH** (optional): initial window size, e.g. `560x420`. Default `520x380`.

Derive **`<Type>`** = PascalCase of the id (e.g. `notes` → `Notes`, `file-browser`
→ `FileBrowser`).

If `$1` is missing, ask for the id and stop. If
`ruos-desktop/crates/gui-core/src/desktop/apps/<id>.rs` or `ruos-desktop/apps/<id>-app/`
already exist, stop and report — do not overwrite.

## Steps (do all, in order)

### 1. DeskApp UI — `ruos-desktop/crates/gui-core/src/desktop/apps/<id>.rs`
```rust
use crate::desktop::app_trait::DeskApp;

/// <Title> — finestra del desktop ruos.
#[derive(Default)]
pub struct <Type> {
    // Stato dell'app (persiste tra i frame). Esempio:
    // count: u32,
}

impl DeskApp for <Type> {
    fn id(&self) -> &'static str { "<id>" }
    fn title(&self) -> &'static str { "<Title>" }
    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("<Title>");
        ui.label("Nuova app — sostituisci con la tua UI.");
        // Vedi docs/implementing-an-app.md per widget, tabelle, painter, animazione.
    }
}
```

### 2. Register it — `ruos-desktop/crates/gui-core/src/desktop/apps/mod.rs`
- Add `pub mod <id>;` (keep the `pub mod` lines alphabetically sorted).
- Add `Box::new(<id>::<Type>::default()),` to the `default_apps()` vec.

### 3. App crate — `ruos-desktop/apps/<id>-app/`
`Cargo.toml`:
```toml
[package]
name = "<id>-app"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
ruos-window = { path = "../../crates/ruos-window" }
gui-core = { path = "../../crates/gui-core" }
egui = { workspace = true }

[profile.release]
panic = "abort"
lto = true
```
`src/lib.rs` (substitute `<W>`/`<H>` from `$3`):
```rust
//! `<id>` — finestra <Title> (thin reactor su `ruos-window` che avvolge il DeskApp
//! `gui_core::desktop::apps::<id>::<Type>`).
use ruos_window::{frame_once, WindowState};
use gui_core::desktop::app_trait::DeskApp;
use gui_core::desktop::apps::<id>::<Type>;

const W: u32 = <W>;
const H: u32 = <H>;

static mut S: Option<WindowState> = None;
static mut APP: Option<<Type>> = None;

#[no_mangle]
pub extern "C" fn frame() {
    #[allow(static_mut_refs)]
    unsafe {
        if S.is_none() { S = Some(WindowState::new()); }
        if APP.is_none() { APP = Some(<Type>::default()); }
        let s = S.as_mut().unwrap();
        let app = APP.as_mut().unwrap();
        let title = app.title();
        frame_once(s, title, W, H, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.ui(ui));
        });
    }
}

#[no_mangle]
pub extern "C" fn _start() {}
```

### 4. Workspace member — `ruos-desktop/Cargo.toml`
Add `"apps/<id>-app",` to `members` (in the `apps/` group, before `apps/compositor-app`).

### 5. Parent Makefile — `Makefile` (ruos repo root)
- Add a build rule next to the other `build/<x>.cwasm` app rules (after
  `build/system.cwasm`):
```makefile
build/<id>.cwasm: $(WT_PRECOMPILE) $(APP_SRCS) $(wildcard $(RUOS_DESKTOP)/apps/<id>-app/src/*.rs $(RUOS_DESKTOP)/apps/<id>-app/Cargo.toml)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && cargo build -p <id>-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/<id>_app.wasm build/<id>.cwasm
```
- Add `build/<id>.cwasm` to the prerequisite list of BOTH the `iso:` and
  `test-boot:` targets (next to `build/system.cwasm`).
- Add a copy line to BOTH targets (next to the `system.cwasm` cp):
```makefile
	cp build/<id>.cwasm $(ISO_ROOT)/bin/<id>.cwasm
```

### 6. Shell launcher — `ruos-desktop/apps/shell/src/lib.rs`
Add to `CATALOG`:
```rust
    ShellAppEntry { id: "<id>", title: "<Title>" },
```

### 7. Build & report
Build via WSL (the only toolchain host):
```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/c/SVILUPPO/Github.com/ruos && make iso'
```
Report which files were created/edited and the build result. Remind the user:
- iterate the UI on PC with `cargo run -p pc-backend` (it shows every `default_apps()` app),
- launch on-device from `☰ Apps → <Title>` after `make run`,
- the new files span the submodule (gui-core + app crate) AND the parent (Makefile)
  — committing means a submodule commit + a parent gitlink bump, in lockstep.

Do NOT commit or push unless the user explicitly asks.
