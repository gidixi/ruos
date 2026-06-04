# Typed gfx over core-module (wit-bindgen) + poweroff button — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Replace the hand-marshalled `ruos_gfx` raw-pointer host ABI with a single `wit/ruos-gui.wit` that generates TYPED guest bindings (wit-bindgen, **core-module mode**) decoded by a small kernel Canonical-ABI codec — and deliver the desktop **power-off button** as the first capability added through the new typed layer.

**Architecture:** The egui desktop guest stays a `wasm32-wasip1` **core module** on the existing `run_cwasm` path (WASI Preview 1 / `wasi.rs` UNCHANGED, no launch switch, no WASI-p2, no component runtime for the GUI). `ruos-backend` uses `wit_bindgen::generate!` (core-module guest, NO `wasm-tools component new`) so its imports lower via the Canonical ABI into named core-module imports; the kernel `func_wrap`s those import names and decodes the Canonical ABI with `wt/mem.rs`. Host return types are kept to **scalars + fixed records** (`get-info` via a return-area pointer; `poll-event -> option<gfx-event>` called in a loop) so there are **no host-returned lists/strings → no `cabi_realloc` re-entrancy**. This is the design spec's Approach B scoped to gfx/system (see spec Appendix C); full Component Model (A) is deferred until WASI-on-component exists.

**Tech Stack:** Rust pinned nightly-2026-05-26; guest `wasm32-wasip1` + `wit-bindgen 0.57` (`default-features=false, features=["macros","realloc","bitflags"]` — proven in CHANGELOG 274); kernel wasmtime 45 **core** `Linker` (no component-model needed for this path); `wasm-tools 1.251.0` for inspection; built via WSL `make`.

**Verification idiom:** boot self-test markers + **visual QEMU screendump** (egui text must stay crisp — this re-routes gfx through a new ABI, so the garble class must be re-verified) + a power-off functional check. Use `ISO=build/cmtest.iso` for boot tests; for the GUI use `make iso INIT_SCRIPT=user-bin/wt-gui-init.sh ISO=build/guitest.iso` then QEMU. Never overwrite `build/os.iso`.

---

## File Structure

- `wit/ruos-gui.wit` — **Create.** Interfaces `gfx` (get-info, blit, poll-event, pending, wall-seconds, debug-log) + `power` (poweroff, reboot); `world ruos-gui` importing both. Single source of truth for the desktop surface.
- `ruos-desktop/ruos-backend/src/main.rs` — **Modify.** Replace the `#[link(wasm_import_module="ruos_gfx")] extern "C"` block + the `u32_at`/`f32::from_bits` unpacking with `wit_bindgen::generate!` typed calls; map the generated `gfx`/`power` types ↔ `gui_core::abi::{GfxInfo, GfxEvent, MouseButton}` inside `RuosPlatform`.
- `ruos-desktop/ruos-backend/Cargo.toml` — **Modify.** Add `wit-bindgen` dep (0.57, the feature set proven in CHANGELOG 274).
- `kernel/src/wasm/wt/gui.rs` — **Create.** `add_to_linker` registering `func_wrap`s for the `ruos:gui/gfx` + `ruos:gui/power` import names, decoding the Canonical-ABI args via `crate::wasm::wt::mem` and calling `crate::gfx::*` / `crate::power::*`.
- `kernel/src/wasm/wt/mod.rs` — **Modify.** `pub mod gui;` + register `gui::add_to_linker` in `run_cwasm`'s linker (alongside `wasi` + the legacy `gfx`).
- `ruos-desktop/gui-core/src/platform.rs` — **Modify.** Add `fn poweroff(&mut self) {}` (default no-op).
- `ruos-desktop/gui-core/src/desktop/{mod.rs, panel.rs}` + `lib.rs` — **Modify.** A `⏻` button in the panel sets a flag; `Gui::frame` calls `platform.poweroff()` when set.
- `ruos-desktop/pc-backend/src/main.rs` — **Modify.** Implement `poweroff` (e.g. `event_loop.exit()` / `std::process::exit(0)`) so desktop dev still exercises the path.
- `Makefile` — **Modify (likely none).** The existing `build/gui.cwasm` rule already rebuilds `ruos-backend` + precompiles; it now also depends on `wit/ruos-gui.wit` (add to prereqs). It stays `precompile_module` (core module) — NOT `--component`.

---

## Task 1: Author `wit/ruos-gui.wit`

**Files:** Create `wit/ruos-gui.wit`

- [ ] **Step 1: Write the WIT** (return types are scalar/record/option only — no host-returned list/string → no cabi_realloc)

```wit
package ruos:gui;

interface gfx {
  record gfx-info { width: u32, height: u32, stride: u32, format: u32 }
  // Mirrors the current 16-byte event {kind,p0,p1,p2}; kept flat for a trivial
  // return-area encode. kind: 0=key,1=mousemove,2=mousebtn,3=resize,4=quit.
  record gfx-event { kind: u32, p0: u32, p1: u32, p2: u32 }

  get-info: func() -> gfx-info;
  blit: func(pixels: list<u8>, x: u32, y: u32, w: u32, h: u32);
  poll-event: func() -> option<gfx-event>;   // called in a loop until none
  pending: func() -> u32;
  wall-seconds: func() -> f64;
  debug-log: func(msg: string);
}

interface power {
  poweroff: func();
  reboot: func();
}

world ruos-gui {
  import gfx;
  import power;
}
```
(Note: this world has NO `export run` — the desktop's entry stays `_start`/`main` via the existing WASI path; the WIT only declares the host IMPORTS. The guest `generate!` will emit import bindings only.)

- [ ] **Step 2: Validate**

```
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && wasm-tools component wit wit/ruos-gui.wit >/dev/null && echo WIT-OK'
```
Expected: `WIT-OK`.

- [ ] **Step 3: Commit**

```
cd /e/MinimalOS/BasicOperatingSystem && git add wit/ruos-gui.wit && git commit -m "feat(wasm): ruos:gui WIT (typed gfx + power surface)"
```

---

## Task 2: Guest — wit-bindgen typed imports in `ruos-backend`

**Files:** Modify `ruos-desktop/ruos-backend/Cargo.toml`, `ruos-desktop/ruos-backend/src/main.rs`

- [ ] **Step 1: Cargo.toml** — add the dep (feature set proven in CHANGELOG 274):

```toml
wit-bindgen = { version = "0.57", default-features = false, features = ["macros", "realloc", "bitflags"] }
```

- [ ] **Step 2: Generate typed imports + rewrite the platform.** In `ruos-backend/src/main.rs`, DELETE the `#[link(wasm_import_module = "ruos_gfx")] extern "C" { ... }` block and the `u32_at` helper. Add at the top:

```rust
wit_bindgen::generate!({
    path: "../../wit/ruos-gui.wit",
    world: "ruos-gui",
});
```
This is a `wasm32-wasip1` **std** crate (keep it std — DO NOT add `#![no_std]`; WASI stays). `generate!` emits import modules; the exact paths follow wit-bindgen 0.57 (likely `crate::ruos::gui::gfx::{get_info, blit, poll_event, pending, wall_seconds, debug_log}` returning generated `GfxInfo`/`Option<GfxEvent>` types, and `crate::ruos::gui::power::{poweroff, reboot}`). Build and reconcile to the ACTUAL generated paths/types (the compiler names them).

Rewrite `RuosPlatform` to call these typed fns and map the generated `gfx::GfxInfo`/`gfx::GfxEvent` to `gui_core::abi::{GfxInfo, GfxEvent, MouseButton}`:
- `surface_info` → `let i = gfx::get_info(); GfxInfo { width: i.width, height: i.height, stride: i.stride, format: i.format }`.
- `poll_events` → loop `while let Some(e) = gfx::poll_event() { out.push(map_event(e)); }` where `map_event` translates `kind/p0/p1/p2` exactly as today (kind 1 → `MouseMove{ x: f32::from_bits(p0), y: f32::from_bits(p1) }`, etc.). The old `gfx_pending` gate → `gfx::pending()`.
- `present` → `gfx::blit(buf, x, y, w, h)` (buf: `&[u8]` → the generated `list<u8>` param).
- `wall_clock_secs` → `gfx::wall_seconds()`.
- `dbg(s)` → `gfx::debug_log(s)`.
- Implement `poweroff` (the trait method added in Task 5) → `power::poweroff()`.

- [ ] **Step 3: Build the guest core module + inspect the imports (feeds Task 3's codec)**

```
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-desktop && cargo build -p ruos-backend --target wasm32-wasip1 --release 2>&1 | tail -20 && cd .. && wasm-tools print ruos-desktop/target/wasm32-wasip1/release/gui.wasm | grep -E "import \"ruos:gui/(gfx|power)\"" '
```
Expected: the build finishes, and the grep lists the import module names + function names + their flattened signatures (e.g. `(func (param i32))` for `get-info`'s return-area, `(func (param i32 i32 i32 i32 i32 i32))` for `blit`). **Record these exact names + signatures — Task 3's `func_wrap`s must match them byte-for-byte.** Keeping it a core module (no `component new`) means these are plain named imports the kernel core Linker resolves; the existing `wasi_snapshot_preview1` imports are still present and unchanged.

- [ ] **Step 4: Commit**

```
cd /e/MinimalOS/BasicOperatingSystem && git add ruos-desktop/ruos-backend/Cargo.toml ruos-desktop/ruos-backend/src/main.rs && git commit -m "feat(gui): ruos-backend uses typed ruos:gui imports (wit-bindgen)"
```
(Submodule commit — controller bumps the gitlink later. Do NOT touch other submodule files, esp. `about.rs`.)

---

## Task 3: Kernel — Canonical-ABI host codec for `ruos:gui`

**Files:** Create `kernel/src/wasm/wt/gui.rs`; modify `kernel/src/wasm/wt/mod.rs`

- [ ] **Step 1: Write `kernel/src/wasm/wt/gui.rs`.** Mirror `kernel/src/wasm/wt/gfx.rs` (same `mem::read`/`mem::write` helpers) but register the `ruos:gui/gfx` + `ruos:gui/power` import names with the EXACT signatures captured in Task 2 Step 3. Encode/decode the Canonical ABI:
  - `get-info` lowers to `(param i32 retptr)` → write the 16-byte `gfx-info` record (4×u32 LE: width,height,stride,format) at `retptr` via `mem::write` (identical bytes to today's `gfx_info`).
  - `blit` lowers to `(param ptr len x y w h)` → `mem::read(ptr,len)` then `crate::gfx::blit(&bytes, x, y, w, h)` (identical to today's `gfx_blit`).
  - `poll-event` lowers to `(param i32 retptr)` → write an `option<gfx-event>`: byte 0 = discriminant (0=none, 1=some), then (if some) the 16-byte event at the record offset the ABI dictates (confirm the exact layout/offset from the Task 2 `wasm-tools print` of how the guest READS the return-area; encode to match). Pull one event via `crate::gfx::pop()` (drain one; the guest loops). Fold the PS/2 mouse first via `crate::gfx::fold_mouse()` on the first poll of a frame — match current semantics (today `gfx_poll_event` folds then drains up to max; here fold on `pending`/first `poll`).
  - `pending` → `()-> i32`: `crate::gfx::fold_mouse(); crate::gfx::pending() as i32`.
  - `wall-seconds` → `() -> f64`: `crate::wasm::wt::gfx::` reuse the existing `wall_secs()` (move/share it) so monotonic behavior is identical.
  - `debug-log` → `(param ptr len)`: `mem::read` → utf8 → `kprintln!("[gui] {}", s)`.
  - `power.poweroff` → `()`: `crate::power::poweroff()`. `power.reboot` → `crate::power::reboot()`.
  - Provide `pub fn add_to_linker(linker: &mut Linker<WtState>) -> wasmtime::Result<()>`.

  IMPORTANT: confirm each `func_wrap` closure's params EXACTLY match the flattened signature from Task 2 (count + i32/i64/f32/f64). A mismatch = link/instantiate error. Use `wasm-tools print` output as the contract.

- [ ] **Step 2: Register in `run_cwasm`** (`kernel/src/wasm/wt/mod.rs`): add `pub mod gui;` and, after the existing `gfx::add_to_linker(&mut linker)` call, add `gui::add_to_linker(&mut linker)?` (keep the legacy `gfx` linker during migration so nothing else breaks).

- [ ] **Step 3: Build the kernel (release, no boot-checks needed)**

```
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && cd kernel && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -20'
```
Expected: `Finished`.

- [ ] **Step 4: Commit**

```
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/gui.rs kernel/src/wasm/wt/mod.rs && git commit -m "feat(wasm): kernel canonical-ABI codec for ruos:gui imports"
```

---

## Task 4: Build + visually verify the desktop renders through the typed path

**Files:** Modify `Makefile` (add `wit/ruos-gui.wit` to the `build/gui.cwasm` prereqs)

- [ ] **Step 1: Makefile prereq.** Add `wit/ruos-gui.wit` (and the ruos-backend sources, already covered by `RUOS_DESKTOP_SRCS` from CHANGELOG 271) to the `build/gui.cwasm` rule's prerequisites so editing the WIT rebuilds the cwasm. Keep the rule using the non-component precompile (it stays a core module).

- [ ] **Step 2: Build a GUI ISO + boot it headless with a screendump.** Use QMP screendump (see prior cursor/garble verification): boot `make iso INIT_SCRIPT=user-bin/wt-gui-init.sh ISO=build/guitest.iso`, launch QEMU `-enable-kvm -cpu host` with a QMP socket, wait ~4s for the desktop, `screendump build/gui-typed.ppm`, convert + inspect.

```
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso INIT_SCRIPT=user-bin/wt-gui-init.sh ISO=build/guitest.iso 2>&1 | tail -5'
```
Then launch + screendump (reuse the QMP screendump approach from the garble/cursor work in build/). Inspect the PPM: **egui text must be crisp (no garble), the wallpaper gradient correct, the mouse cursor present, the clock showing HH:MM**. This is the decisive re-verification that routing gfx through the new Canonical-ABI codec did not corrupt rendering.

- [ ] **Step 3:** If rendering is correct, **remove the legacy `ruos_gfx` path**: drop `gfx::add_to_linker` from `run_cwasm` and delete the now-unused `ruos_gfx` `func_wrap`s in `kernel/src/wasm/wt/gfx.rs` (keep the non-host helpers like `crate::gfx::*` which `gui.rs` calls; only remove the wasmtime `ruos_gfx` linker fns). Rebuild + re-verify the screendump still renders. If anything regresses, keep both paths and report DONE_WITH_CONCERNS.

- [ ] **Step 4: Commit**

```
cd /e/MinimalOS/BasicOperatingSystem && git add Makefile kernel/src/wasm/wt/gfx.rs kernel/src/wasm/wt/mod.rs && git commit -m "feat(wasm): route desktop gfx through typed ruos:gui; drop legacy ruos_gfx linker"
```

---

## Task 5: Power-off button (first capability through the typed layer)

**Files:** Modify `ruos-desktop/gui-core/src/platform.rs`, `ruos-desktop/gui-core/src/desktop/{mod.rs,panel.rs}`, `ruos-desktop/gui-core/src/lib.rs`, `ruos-desktop/pc-backend/src/main.rs`, `ruos-desktop/ruos-backend/src/main.rs`

- [ ] **Step 1: Platform trait** (`platform.rs`): add `fn poweroff(&mut self) {}` (default no-op so pc/harness compile).

- [ ] **Step 2: UI flag + button.** In `desktop/mod.rs` add `poweroff: bool` to `Desktop`; pass `&mut self.poweroff` to `panel::show`; in `panel.rs` add a button in the right-to-left layout next to the clock: `if ui.button("⏻").clicked() { *poweroff = true; }`. Add `pub fn poweroff_requested(&self) -> bool { self.poweroff }` on `Desktop` and surface it via `App`.

- [ ] **Step 3: Frame loop wiring** (`lib.rs::Gui::frame`): after `self.ctx.run(...)`, `if self.app.poweroff_requested() { platform.poweroff(); }`.

- [ ] **Step 4: Backends.** `ruos-backend`: `fn poweroff(&mut self) { power::poweroff(); }` (the generated typed import). `pc-backend`: `fn poweroff(&mut self) { std::process::exit(0); }`.

- [ ] **Step 5: Build + verify power-off.** Rebuild the GUI ISO, boot under QEMU, move the mouse to the `⏻` button, click via QMP `input-send-event`, and confirm QEMU exits / the guest powers off (serial shows the power port write or QEMU terminates). On PC: `cargo run -p pc-backend`, click ⏻, window closes.

```
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso INIT_SCRIPT=user-bin/wt-gui-init.sh ISO=build/guitest.iso 2>&1 | tail -3'
```
(then QMP click + observe poweroff; QEMU q35 ACPI shutdown port 0xB004 / isa-debug-exit handle `crate::power::poweroff`'s writes.)

- [ ] **Step 6: Commit** (two commits — submodule gui-core/pc-backend, then the controller bumps the gitlink)

```
cd /e/MinimalOS/BasicOperatingSystem/ruos-desktop && git add gui-core/src/platform.rs gui-core/src/desktop/mod.rs gui-core/src/desktop/panel.rs gui-core/src/lib.rs pc-backend/src/main.rs ruos-backend/src/main.rs && git commit -m "feat(gui): power-off button via typed power.poweroff"
```

---

## Task 6: VBox verify + CHANGELOG + final review

- [ ] **Step 1:** Boot the GUI ISO on VBox (per project memory, verify VBox for anything power/MSR-adjacent): confirm the desktop renders + the ⏻ button powers off the VM.
- [ ] **Step 2:** Write `CHANGELOG/NN-...` (next free number) summarizing: typed gfx via wit-bindgen core-module + kernel Canonical-ABI codec; legacy `ruos_gfx` removed; power-off button as the first capability; rendering re-verified (no garble). Note the submodule gitlink bump.
- [ ] **Step 3:** Bump the `ruos-desktop` submodule gitlink in the superproject + commit. Then dispatch a final code-reviewer over the whole diff.

---

## Self-Review notes
- **Spec coverage:** implements spec Appendix C (Approach B for the desktop surface) — typed `.wit`, wit-bindgen core-module guest, kernel codec, no WASI-p2, GUI stays on `run_cwasm`. Power-off = the first capability (spec §5 Step 4 intent, via B instead of A). Full Component Model / surfaces-as-resources / WASI fold-in remain deferred.
- **No placeholders:** WIT is concrete; the one reconciliation point (exact wit-bindgen 0.57 import paths/signatures) is an explicit `wasm-tools print` capture step (Task 2 Step 3) feeding Task 3 — not a vague TODO.
- **Type consistency:** `gfx-info`{width,height,stride,format} and `gfx-event`{kind,p0,p1,p2} match `gui_core::abi` and the current 16-byte wire layout; the codec encodes the identical bytes today's `gfx_info`/`gfx_poll_event` produce, so rendering is byte-identical by construction (Task 4 verifies).
- **Risk:** the `poll-event` return-area `option<record>` encoding is the one nontrivial codec piece — Task 3 ties it to the `wasm-tools print` of how the guest reads the return area. The bulk-pixel `blit` stays a `(ptr,len)` read (no copy beyond what `mem::read` already does; small with dirty-rect).
