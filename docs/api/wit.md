# WIT — component-model ABI

Typed (WebAssembly Component Model) bridge between the kernel and component guests.
Source: `wit/ruos-gui.wit`, `wit/ruos-bringup.wit`. Kernel side:
`kernel/src/wasm/wt/component.rs` + the WIT-bound linkers.

This is the typed alternative to the raw [`wm`](wm.md)/[`term`](term.md) modules,
plus the raw framebuffer service `gfx`. Used by component-model guests (e.g. the
bring-up component); the egui window apps use the raw modules.

**Last reviewed:** 2026-06-09.

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
