# WIT — component-model ABI

Typed (WebAssembly Component Model) bridge between the kernel and component guests.
Source: `wit/ruos-gui.wit`, `wit/ruos-bringup.wit`, `wit/ruos-tui.wit`. Kernel
side: `kernel/src/wasm/wt/component.rs` + the WIT-bound linkers.

This is the typed alternative to the raw [`wm`](wm.md)/[`term`](term.md) modules,
plus the raw framebuffer service `gfx`. Used by component-model guests (e.g. the
bring-up component and the `ruos:tui` TUI apps); the egui window apps use the
raw modules.

**Last reviewed:** 2026-06-11.

---

## `ruos:gui` world  (`wit/ruos-gui.wit`)

### interface `gfx`
```wit
record gfx-info  { width: u32, height: u32, stride: u32, format: u32 }
record gfx-event { kind: u32, p0: u32, p1: u32, p2: u32 }

get-info:     func() -> gfx-info
blit:         func(pixels: list<u8>, x: u32, y: u32, w: u32, h: u32)
poll-event:   func() -> option<gfx-event>
pending:      func() -> u32
wall-seconds: func() -> f64
debug-log:    func(msg: string)
```
Raw framebuffer service: query the surface, blit RGBA, poll input
(`kind`: 0=key,1=mouse-move,2=mouse-button,3=resize,4=quit — see [wm](wm.md)),
monotonic time, log.

### interface `power`
```wit
poweroff: func()
reboot:   func()
```

### interface `term`
```wit
open:   func() -> s32
read:   func(handle: s32, cap: u32) -> list<u8>
write:  func(handle: s32, bytes: list<u8>)
resize: func(handle: s32, cols: u32, rows: u32)
close:  func(handle: s32)
```
Same PTY/shell bridge as the raw [`term`](term.md) module, typed.

---

## `ruos:bringup` world  (`wit/ruos-bringup.wit`)

### interface `system`
```wit
log:      func(msg: string)   // append to the kernel log
poweroff: func()              // never returns
```
Guest exports `run: func() -> s32` — the kernel calls it after instantiation
(bring-up test of the zero-arg / no-return component shape).

---

## `ruos:tui` package  (`wit/ruos-tui.wit`)

Component TUI apps (e.g. `rtop.cwasm`, world `tui-app`). At exec time the
kernel detects the component artifact (`Engine::detect_precompiled`),
instantiates the **shared provider** `/bin/tui.cwasm` (world `tui-provider`,
built from `tools/wt-tui` — ratatui + ANSI diff renderer) in the same store,
and links the app's `canvas` imports to the provider's exports with func
shims. `host` is implemented by the kernel
(`kernel/src/wasm/wt/component.rs::run_tui_component`).

Placement: interactive apps run on a **ComputeApp core** (poll-key spin-waits
there). On a single-core boot poll-key returns EOF immediately — the app
renders one frame and exits (only `--once` mode is fully useful there).

### interface `canvas`  (provider export, app import)
```wit
record rect  { x: u16, y: u16, w: u16, h: u16 }
record color { r: u8, g: u8, b: u8 }

init:       func(width: u16, height: u16)      // (re)size the back buffer
clear:      func()                             // empty the back buffer
draw-text:  func(text: string, area: rect, fg: color)
draw-bar:   func(pct: u8, area: rect, fg: color)   // "[###---] NN%"
draw-table: func(headers: list<string>, rows: list<list<string>>,
                 widths: list<u16>, area: rect)    // reversed-video header
flush:      func() -> string
```
Widgets accumulate into one back buffer; `flush` diffs against the last
flushed frame and returns the ANSI escape string (full repaint on the first
frame / after `init`). The app writes it to the tty via `host.write-tty`.

### interface `host`  (kernel import)
```wit
cpustat:   func() -> list<u8>   // u32 ncores, u64 tsc-per-ms, per core busy/idle u64
proc-stat: func() -> list<u8>   // u32 count, then pid/start/cpu-tsc/mem/name records
meminfo:   func() -> list<u8>   // 4×u64: heap-total, heap-used, frames-total, frames-used
uptime:    func() -> u64        // centiseconds since boot
poll-key:  func(timeout-ticks: s64) -> s32  // -1 EOF/no-tty, 0 timeout, 1..=256 byte+1
set-raw:   func(raw: bool)      // clear/restore ICANON|ECHO|ISIG on the app pty
write-tty: func(s: string)      // write to the app's pty (console fallback)
```
Blob layouts match the wasmi [`ruos`](ruos.md) host fns (`cpustat`,
`proc_stat`, `meminfo`), so parsers are shared between tool flavors.

### worlds
- `tui-provider` — exports `canvas` (implemented by `tools/wt-tui` → `tui.cwasm`).
- `tui-app` — imports `canvas` + `host`, exports `run: func(once: bool) -> s32`.
  `once=true` (shell arg `--once`) → single plain-text snapshot, no loop.
