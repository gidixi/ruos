# Compositor egui SP-E — port DeskApps as windows + retire gui.cwasm — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

> **Spec:** `docs/superpowers/specs/2026-06-05-egui-compositor-sp-e-apps-as-windows-design.md` (read first).

**Goal:** About / Files / Terminal / System Monitor open as compositor windows from the desktop launcher (their existing `DeskApp` UI, placeholder data); `gui.cwasm` leaves the default build/ISO (code kept). The SP-D shell's "☰ Apps" launcher entries (already listed) light up.

**Architecture (crate strategy A):** four thin `wasm32-wasip1` cdylib crates in `ruos-desktop` (`about-app`/`files-app`/`terminal-app`/`system-app`), each wrapping one gui-core `DeskApp` via the `ruos-window` SDK (`frame_once(title, |ctx| CentralPanel{ app.ui(ui) })`). Built → `.cwasm`, shipped to `/bin/<id>.cwasm`, mounted by limine, resolved by `wm.spawn(id)`. gui.cwasm is removed from `iso:`/`test-boot:` (the `gui` command resolves via `/bin/gui.cwasm` — no kernel special-case — so unshipping disables it).

**Tech Stack:** kernel `no_std` wasmtime AOT (unchanged this SP — no kernel edits); guests `wasm32-wasip1` on `ruos-window`+`gui-core`+`egui`(+`egui_extras` for System). Build via WSL (`-d Ubuntu`). Verify: build + QEMU QMP screendump per app + VBox; gui.cwasm absent.

---

## File Structure

| File | Responsibility |
|---|---|
| `ruos-desktop/{about,files,terminal,system}-app/{Cargo.toml,src/lib.rs}` | NEW — four thin window crates, each wrapping a DeskApp. |
| `ruos-desktop/Cargo.toml` | Add the four crates to `members`. |
| `Makefile` | Four `build/<id>.cwasm` rules (wasip1→wt-precompile) + ship to `$(ISO_ROOT)/bin/<id>.cwasm` in `iso:`/`test-boot:`. Remove `build/gui.cwasm` from `iso:`/`test-boot:` prereqs + its `cp`. |
| `limine.conf` | Four `/bin/<id>.cwasm` module entries (so the VFS mounts them). |

No kernel source changes (the mechanism is all SP-C/SP-D). No gui-core changes (the DeskApp structs are already `pub`).

---

## Task 1: The four window crates

Each crate is the `compositor-app` shape (read `ruos-desktop/compositor-app/src/lib.rs`) but wrapping a `DeskApp`. All four are identical except the import path, the struct, its constructor, and W/H.

**Files:** Create `ruos-desktop/{about,files,terminal,system}-app/{Cargo.toml,src/lib.rs}`; modify `ruos-desktop/Cargo.toml`.

- [ ] **Step 1: workspace members.** In `ruos-desktop/Cargo.toml` add `"about-app", "files-app", "terminal-app", "system-app"` to `members`.

- [ ] **Step 2: `Cargo.toml` per crate** (e.g. `about-app/Cargo.toml`; same shape for the other three, change `name`):
```toml
[package]
name = "about-app"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
ruos-window = { path = "../ruos-window" }
gui-core = { path = "../gui-core" }
egui = { workspace = true }

[profile.release]
panic = "abort"
lto = true
```
For `system-app` ALSO add `egui_extras = { workspace = true }` if the crate references it directly (System's `ui` uses `egui_extras::TableBuilder`, but that's inside gui-core's `system.rs` — the dep is transitive via gui-core; only add `egui_extras` to system-app if a direct reference is needed; default: don't, gui-core re-exports nothing extra needed).

- [ ] **Step 3: `src/lib.rs` per crate.** The template (this is `about-app`):
```rust
//! About window — a thin app on the `ruos-window` SDK wrapping gui-core's
//! `AboutRuos` DeskApp. `frame()` drives the SDK; the closure renders the app's
//! `ui` in a CentralPanel under the CSD title bar.
use ruos_window::{frame_once, WindowState};
use gui_core::desktop::app_trait::DeskApp;
use gui_core::desktop::apps::about::AboutRuos;

const W: u32 = 560;
const H: u32 = 420;

static mut S: Option<WindowState> = None;
static mut APP: Option<AboutRuos> = None;

#[no_mangle]
pub extern "C" fn frame() {
    #[allow(static_mut_refs)]
    unsafe {
        if S.is_none() { S = Some(WindowState::new()); }
        if APP.is_none() { APP = Some(AboutRuos); }   // <-- per-app construction (table below)
        let s = S.as_mut().unwrap();
        let app = APP.as_mut().unwrap();
        let title = app.title();
        frame_once(s, title, W, H, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.ui(ui));
        });
    }
}
#[no_mangle] pub extern "C" fn _start() {}
```
The **four differences** (apply to each crate):

| crate | import | struct type | `APP = Some(...)` | W×H |
|---|---|---|---|---|
| about-app | `apps::about::AboutRuos` | `AboutRuos` | `AboutRuos` | 560×420 |
| files-app | `apps::files::Files` | `Files` | `Files` | 560×420 |
| terminal-app | `apps::terminal::Terminal` | `Terminal` | `Terminal::default()` | 560×420 |
| system-app | `apps::system::System` | `System` | `System::default()` | 720×520 |

(`app.title()` comes from the `DeskApp` trait — already returns "About ruOS"/"Files"/"Terminal"/"System Monitor". `app.id()` is the launcher id but the title is what the CSD bar shows.)

- [ ] **Step 4: Build the four guests.** `cd ruos-desktop && cargo build --release -p about-app -p files-app -p terminal-app -p system-app --target wasm32-wasip1 2>&1 | tail -20` → all `Finished`. For each, `wasm-tools print ruos-desktop/target/wasm32-wasip1/release/<crate_underscored>.wasm | grep -E 'import \"wm\"|export .*frame'` → imports `wm.{commit,poll_event,...}`, exports `frame`. (System Monitor compiling to wasip1 confirms `egui_extras::TableBuilder` works there — it already does in gui.cwasm.)

- [ ] **Step 5: Commit** (submodule): `cd ruos-desktop && git add Cargo.toml about-app files-app terminal-app system-app && git commit -m "feat(apps): four thin window crates wrapping gui-core DeskApps (about/files/terminal/system)"`.

---

## Task 2: Makefile — build + ship the four app .cwasm

**Files:** `Makefile`.

- [ ] **Step 1: Four build rules.** Mirror the `gui.cwasm` rule (line ~132 — an app `.cwasm` in `build/`, NOT embedded in the kernel; loaded from `/bin` via the VFS). For each app, add (example `about`; the wasm output is in the workspace target dir `ruos-desktop/target/wasm32-wasip1/release/about_app.wasm`):
```makefile
# Desktop app windows (SP-E): each gui-core DeskApp wrapped as a wasip1 window crate,
# AOT-precompiled, shipped to /bin/<id>.cwasm and spawned by the shell launcher.
APP_SRCS := $(shell find $(RUOS_DESKTOP)/gui-core/src $(RUOS_DESKTOP)/ruos-window/src -name '*.rs' 2>/dev/null) \
            $(wildcard $(RUOS_DESKTOP)/Cargo.toml $(RUOS_DESKTOP)/Cargo.lock)
build/about.cwasm: $(WT_PRECOMPILE) $(APP_SRCS) $(wildcard $(RUOS_DESKTOP)/about-app/src/*.rs $(RUOS_DESKTOP)/about-app/Cargo.toml)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && cargo build -p about-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/about_app.wasm build/about.cwasm
```
Repeat for `files`/`terminal`/`system` (crate `files-app`→`files_app.wasm`→`build/files.cwasm`, etc.). (Confirm the wasm output filename underscoring: `about-app` → `about_app.wasm`.)

- [ ] **Step 2: Add to `iso:` + `test-boot:`.** Add `build/about.cwasm build/files.cwasm build/terminal.cwasm build/system.cwasm` to BOTH targets' prereq lists. In BOTH recipes, after the egui-demo/shell `cp` lines, add:
```makefile
	cp build/about.cwasm $(ISO_ROOT)/bin/about.cwasm
	cp build/files.cwasm $(ISO_ROOT)/bin/files.cwasm
	cp build/terminal.cwasm $(ISO_ROOT)/bin/terminal.cwasm
	cp build/system.cwasm $(ISO_ROOT)/bin/system.cwasm
```
(`/bin/<id>.cwasm` matches the shell CATALOG ids `about/files/terminal/system` → `wm.spawn(id)` resolves.)

- [ ] **Step 3: Build.** `wsl ... make build/about.cwasm build/files.cwasm build/terminal.cwasm build/system.cwasm 2>&1 | tail -8` → four `wrote …` lines. (Don't build the ISO yet — Task 4 does, after Task 3's gui.cwasm removal.)

- [ ] **Step 4: Commit.** `git add Makefile && git commit -m "build: build + ship four desktop app .cwasm to /bin"`.

---

## Task 3: limine.conf — mount the four apps + retire gui.cwasm

**Files:** `limine.conf`, `Makefile`.

- [ ] **Step 1: limine.conf module entries.** Read how `shell.cwasm`/`egui-demo.cwasm` are declared (a `module_path: boot():/bin/<id>.cwasm` + `module_cmdline: /bin/<id>.cwasm` pair). Add the same pair for `about.cwasm`, `files.cwasm`, `terminal.cwasm`, `system.cwasm` (so the VFS `/bin` mounts them — `module_by_name`/`wm.spawn` read the VFS, populated by limine boot-modules).

- [ ] **Step 2: Retire gui.cwasm from the build.** In `Makefile`: remove `build/gui.cwasm` from the `iso:` prereq list (line ~200) and the `test-boot:` prereq list (line ~433); remove the `cp build/gui.cwasm $(ISO_ROOT)/bin/gui.cwasm` line in BOTH recipes (grep `gui.cwasm` to find them). Leave the `build/gui.cwasm:` rule + `RUOS_DESKTOP_SRCS` definition in place (so `make build/gui.cwasm` still works for anyone who wants it — just not built/shipped by default). Also remove the `/bin/gui.cwasm` limine.conf module entry if present (so it's not mounted).
  - The `gui` shell command: there is NO kernel special-case (grep confirmed `gui.cwasm`/`"gui"` absent in `kernel/src`); `gui` resolved via the generic `/bin/gui.cwasm` path. Unshipping it makes `gui` "command not found" — no kernel edit needed.

- [ ] **Step 3: Build the ISO.** `wsl ... make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso 2>&1 | tail -6` → ISO written. Confirm from the make output: the four `cp ... about/files/terminal/system.cwasm` ran AND there is NO `gui.cwasm` cp. Also `make iso ISO=build/cmtest.iso` (default init) builds clean WITHOUT gui.cwasm.

- [ ] **Step 4: Commit.** `git add limine.conf Makefile && git commit -m "build: mount four app .cwasm; retire gui.cwasm from the default ISO (code kept, command unshipped)"`.

---

## Task 4: Visual verification (QEMU+KVM, VBox) + gui.cwasm-gone check

**Files:** `build/spe_verify.py`.

- [ ] **Step 1: Build the GUI ISO** (Task 3 Step 3 produced `build/comptest.iso`).

- [ ] **Step 2: QMP driver `build/spe_verify.py`** (model on `build/spd_verify.py`): boot headless; wait ~18s; `screendump build/spe-0-desktop.png` (the desktop shell). For each app, open the "☰ Apps" menu (click ~ (30,12)) then click the app's row (egui-demo row y≈37, About ≈58, Files ≈79, Terminal ≈100, System ≈121 — from the SP-D menu screendump) → wait ~2s → screendump:
  - About → `build/spe-1-about.png` (CSD window "About ruOS" + its content).
  - Files → `build/spe-2-files.png`.
  - Terminal → `build/spe-3-terminal.png` (text field).
  - System Monitor → `build/spe-4-system.png` (the process table + CPU charts).
  - (Optionally a multi-window shot with 2–3 open.)
  Boot QEMU `-machine q35,accel=kvm:tcg -cpu max -m 512 -no-reboot -display none -serial file:build/spe-serial.log -qmp unix:/tmp/qmp.sock,server,nowait -device qemu-xhci -cdrom build/comptest.iso`. Each app spawn logs `wm.spawn ok name='<id>'` in the serial.
  - **Heap note:** opening all four + the shell = 5 egui instances ≈ 240 MB (256 MB heap). If the 4th/5th spawn fails (`wm.spawn` returns 0, no window — check serial for `failed to allocate`), that's the documented budget limit; verify the apps individually (close one before opening the next) and NOTE it. This is expected, not a bug — SP-F can tune the heap.

- [ ] **Step 3: Assert + report.** `grep -E "wm.spawn ok name='(about|files|terminal|system)'" build/spe-serial.log` (the launcher spawned the apps). Send the screendumps to the controller to VIEW (the real proof: four DeskApps as windows). Report which opened + any heap-limit hit.

- [ ] **Step 4: gui.cwasm gone.** Confirm the ISO has no `/bin/gui.cwasm`: `ls build/iso_root/bin/ | grep gui || echo "no gui.cwasm (retired)"` (after a `make iso`). And the `Desktop`/`ruos-backend` still BUILDS: `wsl ... cd ruos-desktop && cargo build -p ruos-backend --target wasm32-wasip1 --release 2>&1 | tail -3` → `Finished` (code kept, just unshipped).

- [ ] **Step 5: VBox** sanity (`[[vbox-test-harness]]`): boot `build/comptest.iso`, screenshot the desktop, open one app (inject a click or just confirm the desktop+launcher boot), restore os.iso.

---

## Task 5: Changelog + final review

- [ ] **Step 1:** `CHANGELOG/NN-26-06-05-egui-compositor-sp-e.md` (next free NN — ~300). Summarize: the four thin window crates (about/files/terminal/system on ruos-window wrapping the gui-core DeskApps); Makefile build+ship to /bin + limine mounts; the launcher entries now spawn real app windows; gui.cwasm retired from the default ISO (code kept, `gui` command unshipped). Verification (screendumps per app + `wm.spawn ok name=...`). Note the ~5-window heap budget + that real data (System proc::list, Terminal PTY) is SP-F. Reference the spec/plan + `[[vbox-test-harness]]`.
- [ ] **Step 2:** Commit. Final code-reviewer over the four app crates (the static-mut pattern, the per-app construction, sizes) + the Makefile/limine changes (four apps shipped+mounted, gui.cwasm cleanly removed without breaking the build).

---

## Provides (the Model-A desktop is complete)
- Four desktop apps as real compositor windows; the compositor is the sole GUI (gui.cwasm retired).
- The "app = a thin `ruos-window` crate wrapping a `DeskApp`" pattern: a new app = a new crate + a shell CATALOG entry + a Makefile/limine line.
- SP-F (future): a kernel data host fn feeding real CPU/mem/`proc::list` to System Monitor (replacing the simulation) + a real Terminal (PTY-in-window); heap tuning for more concurrent windows.

## Self-Review notes
- **Spec coverage:** four crates (Task 1), build+ship (Task 2), mount + retire gui.cwasm (Task 3), visual + gui-gone + Desktop-still-builds (Task 4). Out-of-scope real-data deferred to SP-F.
- **Placeholders:** the crate template is shown in full + the four-row difference table (import/struct/ctor/size) — the compiler enforces each; the Makefile rule is shown for `about` + "repeat for the other three" with the exact filename mapping; the limine entry says "mirror shell/egui-demo's pair". No vague TODOs.
- **Type consistency:** `AboutRuos`/`Files`/`Terminal::default()`/`System::default()`, `frame_once`, `/bin/<id>.cwasm` ids matching the SP-D CATALOG (about/files/terminal/system), screendump markers — consistent.
