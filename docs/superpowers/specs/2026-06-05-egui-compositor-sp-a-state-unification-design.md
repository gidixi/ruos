# egui apps in the compositor — SP-A: state unification + WASI on the compositor linker (design)

**Date:** 2026-06-05
**Status:** approved (brainstorm), pending spec review → writing-plans
**Branch:** `feat/egui-compositor-sp-a`

## Context — the 3-sub-project arc

North-star feature: **run real egui apps as windows of the kernel-side multi-window
compositor** (which today only hosts `wasm32-unknown-unknown` "reactor" guests that
fill a solid colour). A parallel codebase exploration (5 readers) established this is
**feasible with no wasmtime/AOT barrier** — the egui→pixels pipeline already exists
(`ruos-desktop/gui-core`, retargetable to an arbitrary `w×h` RGBA buffer, with
`Platform::present` as its only IO seam) and the compositor already exists
(`kernel/src/wasm/wt/wm.rs`). The work is **joining them**, decomposed into three
sub-projects, each spec → plan → build:

- **SP-A (this doc) — state unification + WASI on the compositor linker.** The crux,
  the hardest and most load-bearing integration point, **independently verifiable**
  with no egui involved.
- **SP-B — egui-reactor harness.** A `Platform` impl over `wm` + a `frame()`-exporting
  wrapper around `gui-core`; a minimal egui app (button + label) in a window.
- **SP-C — system-info app.** The egui UI (CPU/mem/uptime/process table) + a host
  data fn (kernel snapshot → guest). The first real app.

SP-A delivers **no egui** — it proves a `wasm32-wasip1` (std) guest can live as a
compositor window. SP-B/C build on it.

## The crux SP-A resolves

A wasmtime `Linker<T>` is monomorphised over exactly **one** store-data type `T`.
Today the two host-fn worlds are written against two different concrete types:

- **WASI Preview 1** (`kernel/src/wasm/wt/wasi.rs:19` `add_to_linker`, ~17 functions:
  `proc_exit`, `fd_write/read/seek/close`, `fd_fdstat_get`, `fd_filestat_get`,
  `fd_prestat_get`, `fd_prestat_dir_name`, `path_open`, `args_*`, `environ_*`,
  `clock_time_get`, `random_get`, `sched_yield`) is typed `Linker<WtState>`, where
  `WtState` (`kernel/src/wasm/wt/state.rs:14`) holds `{ args, exit, fds, stdout_pty }`.
- **Compositor surface** (`kernel/src/wasm/wt/wm.rs` `add_to_linker`, 5 functions:
  `wm.commit/app_id/tick/poll_event/close`) is typed `Linker<WmState>`, where
  `WmState` holds `{ id, win_w, win_h, pixels, tick, events, close_requested }`.

An egui window guest is a `wasm32-wasip1` (std) binary, so it imports **both** worlds
at once: WASI (so std's libc shim links + the runtime starts) **and** the `wm`
surface/event protocol (so it composites). You cannot register `wasi.rs::add_to_linker`
(typed `<WtState>`) onto the compositor's `Linker<WmState>`, nor vice versa. SP-A
makes both worlds register onto one linker **without breaking the existing
command-app path** (`run_cwasm`, which legitimately uses `Linker<WtState>`).

## Goal (SP-A only)

A `wasm32-wasip1` reactor guest — exports `frame()`, imports WASI + `wm`, does a real
`std` allocation, fills its surface, calls `wm.commit` — **spawns from the launcher
and composites as a window**, proving WASI + `wm` coexist on one linker and that a
std/wasip1 guest runs inside the compositor's instance/frame model. No egui yet.

## Architecture — accessor-trait-generic host functions (non-breaking)

Instead of a destructive retype of every host closure onto one fat struct, make the
two host-fn registrations **generic over accessor traits**, and define one merged
state that implements both. The existing command-app path keeps using `WtState`
unchanged; only the compositor gets the merged state.

**No struct renames** — keep `WtState` (the WASI state) and `WmState` (the window
state) exactly as they are today; add the two accessor traits, impl them, and define
`AppState` that **embeds both**:

```rust
// New: the two capability accessors. The "self-as-capability" blanket impls let the
// existing concrete states satisfy them with zero field churn.
pub trait HasWasi   { fn wasi(&mut self) -> &mut WtState; fn wasi_ref(&self) -> &WtState; }
pub trait HasWindow { fn win(&mut self)  -> &mut WmState; fn win_ref(&self)  -> &WmState; }

// Command apps (run_cwasm): store-data type stays `WtState`, now also a HasWasi holder.
impl HasWasi for WtState { fn wasi(&mut self) -> &mut WtState { self } fn wasi_ref(&self) -> &WtState { self } }

// Compositor windows: BOTH capabilities, by embedding the existing structs.
pub struct AppState { pub wasi: WtState, pub win: WmState }
impl HasWasi   for AppState { fn wasi(&mut self) -> &mut WtState { &mut self.wasi } fn wasi_ref(&self) -> &WtState { &self.wasi } }
impl HasWindow for AppState { fn win(&mut self)  -> &mut WmState { &mut self.win }  fn win_ref(&self)  -> &WmState { &self.win } }
```

`WtState` and `WmState` keep their field names, so existing field access inside the
closures only gains an accessor hop (`caller.data_mut().wasi().fds` /
`caller.data_mut().win().pixels`) — no struct surgery, no rename ripple across the
codebase.

Then:

- `wasi::add_to_linker<T: HasWasi + 'static>(linker: &mut Linker<T>)` — generic. Each
  closure changes `caller.data().<field>` → `caller.data().wasi_ref().<field>` (read)
  / `caller.data_mut().wasi().<field>` (write). The audited memory path
  (`kernel/src/wasm/wt/mem.rs`) becomes generic over `T` too (it already fetches the
  `memory` export via `Caller`, so the change is a signature `<T>`, not logic).
- `wm::add_to_linker<T: HasWindow + 'static>(linker: &mut Linker<T>)` — generic. The
  SP2 `read_guest`/`write_guest` helpers (already in `wm.rs`) become generic over `T`
  the same way. Closures use `.win()`.
- The compositor builds **one `Linker<AppState>`**, calls **both** generic
  `add_to_linker`s on it, and instantiates every window's `Store<AppState>` against it.
- `run_cwasm` (command apps, `kernel/src/wasm/wt/mod.rs`) keeps `Linker<WtState>` +
  `wasi::add_to_linker(&mut linker)` — it still type-checks because `WtState: HasWasi`.
  **Zero change to the shell / command-app behaviour.**

### Why generic-over-trait, not one fat struct

A single `AppState` that every WASI consumer must adopt would force the command-app
path onto `AppState` too (carrying unused window fields) or duplicate `wasi.rs`. The
accessor-trait approach keeps `wasi.rs` as the single source of truth, lets `WtState`
(command) and `AppState` (window) both satisfy it, and bounds the diff to "retype
signatures + route field access through an accessor".

## Components / files

| File | Change |
|---|---|
| `kernel/src/wasm/wt/state.rs` | Add `HasWasi` trait + `impl HasWasi for WtState` (self-as-capability). `WtState` fields unchanged. |
| `kernel/src/wasm/wt/wm.rs` | Add `HasWindow` trait + `impl HasWindow for WmState`. Define `AppState { wasi: WtState, win: WmState }` (impl both traits). `add_to_linker<T: HasWindow>`; closures via `.win()`. `Compositor` uses `Store<AppState>` / `Linker<AppState>`. Build the compositor linker by calling BOTH generic `add_to_linker`s (`wasi::add_to_linker` + `wm::add_to_linker`). `WmState` fields unchanged. |
| `kernel/src/wasm/wt/wasi.rs` | `add_to_linker<T: HasWasi>`. Closures via `.wasi()`/`.wasi_ref()`. |
| `kernel/src/wasm/wt/mem.rs` | `read`/`write` generic over `T` (signature only — logic unchanged). |
| `kernel/src/wasm/wt/mod.rs` | `run_cwasm` unchanged in behaviour (`WtState: HasWasi`). Confirm it still compiles. |
| `tools/wt-wasip1-probe/{Cargo.toml,src/lib.rs}` | **New.** `wasm32-wasip1` reactor: `#[no_mangle] pub extern "C" fn frame()` that does a `std` alloc (e.g. build a `Vec`/`String`), fills a 320×240 RGBA buffer with a colour, `wm.commit`. NOT a command (`_start`-less reactor); imports WASI (pulled in by std) + `wm`. |
| `Makefile` | Build rule: `wt-wasip1-probe` → `wasm32-wasip1` → `wt-precompile` → `kernel/src/wasm/wt/probe.cwasm`; ship + `include_bytes!`. Add to `APPS`. |
| `kernel/src/wasm/wt/wm.rs` (APPS) | Add `AppEntry { name: "wasip1-probe", cwasm: PROBE_CWASM }`. |

## Data flow (SP-A)

Identical to a reactor today, but the guest is std/wasip1 and instantiated against
`Linker<AppState>`:

```
launcher click "wasip1-probe"
  → Compositor::spawn_app: Store::new(engine, AppState{ wasi: WasiState::default(), win: WindowState::new(id) })
  → linker.instantiate(&store, &module)   // module = wt-precompile(probe.wasip1)
  → frame_all(): probe.frame()            // std alloc + fill + wm.commit(buf,len,w,h)
  → wm.commit closure: caller.data_mut().win().pixels = bytes
  → compose_window + present() (UNCHANGED) → window on screen
```

WASI calls the probe makes at runtime (std startup + the alloc) are serviced by the
WASI closures now present on the compositor linker, reading/writing
`caller.data_mut().wasi()`.

## Error handling

- `proc_exit` / guest trap in a persistent reactor: **out of scope for SP-A.** The
  probe never calls `proc_exit` and is written not to panic. The hazard (a wasip1
  guest's `proc_exit` traps-to-unwind and poisons the persistent instance; `frame_all`
  swallows the `Err` so the window just stops drawing) is documented here and handled
  in **SP-B** (map `proc_exit`/trap → `close_requested` so a crashed window auto-reaps).
- Instantiate failure (missing import, bad module) already returns `None` from
  `spawn_app` and frees the window-id — unchanged.

## Testing / verification

1. **Build** — kernel compiles on all three profiles (`default`, `boot-checks`,
   `serial-composite`) → `Finished`. The command-app path must still build (proves the
   `WtState: HasWasi` retype didn't break `run_cwasm`).
2. **Boot-check (headless, deterministic)** — instantiate the probe against the
   `Linker<AppState>` in a headless self-test, call `frame()` once, assert the surface
   was committed (`win.pixels` non-empty, expected first byte). Marker
   `wasip1 probe spawn ok pixels=<n>`. Proves a std/wasip1 guest instantiates +
   runs against WASI+wm on one linker.
3. **Visual (QEMU+KVM QMP, then VBox)** — boot the compositor ISO, launch the probe
   from the taskbar, screendump → a colour-filled window **rendered by a std/wasip1
   guest** (not `unknown-unknown`). Confirms the full spawn→frame→commit→compose path
   end-to-end. VBox sanity per the project rule (VM `ruos`, see
   `[[vbox-test-harness]]`) — though SP-A is not SMP-sensitive.
4. **Regression** — the shell + an existing command tool (e.g. `ls`/`rtop`) still run
   via `run_cwasm` (the `WtState` path), proving no break.

## Risks

- **Churn:** ~17 WASI + 5 `wm` closures change `Caller<'_, ConcreteState>` →
  `Caller<'_, T>` + accessor field access. A borrow-checker slip (e.g. holding
  `data_mut()` across a `mem::read`) can introduce a panic/aliasing bug under a
  specific call order. Mitigation: introduce the accessors first, retype
  mechanically, compile-check incrementally, keep the existing `read_guest`/`write_guest`
  borrow discipline.
- **Probe cargo shape:** a `wasm32-wasip1` **reactor** (exports `frame()`, has no
  `_start` main) needs the right crate-type / linker args (`cdylib`, reactor ABI) so
  wasm-ld emits a reactor, not a command. Mirror how `gui.cwasm` (wasip1) is built but
  with a `frame` export instead of `_start`. Confirm `wt-precompile` accepts it.
- **`mem.rs` genericisation:** if any caller relied on the concrete `WtState` type
  beyond the `memory` export, the `<T>` change surfaces it — expected to be none.
- **Module-cache identity:** the probe `.cwasm` must be `include_bytes!`'d (stable
  `&'static` ptr) to fit `MODULE_CACHE` keying — same as the existing reactors.

## Out of scope (SP-A)

- egui, `gui-core`, `Platform` impl, `compose_window`/`present`/input changes.
- `proc_exit`/panic → reap mapping (SP-B).
- Surface resize, scroll, clipboard, WIT-ification (later).
- Per-frame cost / repaint-policy tuning (SP-B/C, once egui's real cost is visible).

## Provides (for SP-B)

- `Linker<AppState>` with WASI + `wm` registered, and `Store<AppState>` per window —
  SP-B's egui-reactor instantiates against exactly this.
- The proven wasip1-reactor cargo + `wt-precompile` shape — SP-B's egui app reuses it.
- `AppState` as the home for any future per-window capability (e.g. SP-C's sysinfo
  data channel) without re-touching the linker generics.
