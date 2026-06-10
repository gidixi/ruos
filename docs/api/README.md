# ruos app API manual

The reference for every **host function a WASM app/tool can import** — one page per
import module, like a crate's docs. This is the living manual: it grows as the OS
exposes new host functions (see the maintenance rule below).

## Start here

To write a GUI app, read **[`ruos-window.md`](ruos-window.md)** — the safe app-author
API (`frame_once`, `WindowState`, `declare_manifest!`, helpers). The pages below are
the raw host ABI it wraps; reach for them only for advanced needs.

## Runtimes & where each module applies

| Runtime | App kind | Pages |
|---------|----------|-------|
| **Wasmtime AOT** (`.cwasm`) | GUI window apps (the SDK) | **[ruos-window](ruos-window.md)** (start here) → raw [`wm`](wm.md) · [`sys`](sys.md) · [`term`](term.md) · [`gfx`](gfx.md) |
| **wasmi** (`.wasm`) | CLI tools | [`ruos`](ruos.md) · [`wasi`](wasi.md) |
| **Component model** (WIT) | typed bridge | [`wit`](wit.md) |

> **Note on Runtimes vs Interfaces:** 
> - **Wasmtime** and **wasmi** are execution engines (the "how"). Wasmtime compiles apps Ahead-of-Time for maximum GUI performance, while wasmi interprets CLI tools.
> - **WASI Preview 1** and **WIT** are API standards (the "what"). They define the signatures of functions (like `fd_read` or `clock_time_get`) that an app expects the OS to provide. In short, Wasmtime *runs* your app, but WASI is the *vocabulary* your app uses to talk to RuOS.

## Conventions

- **Signatures** are shown as the guest-side Rust `extern "C"` declaration. Pointers
  are `*const u8` / `*mut u8` into the guest's linear memory; lengths are `u32`.
- **errno**: most `ruos`/`sys`/WASI fns return `i32` = `0` on success, a positive
  errno on failure. Common: `8` ENOBUFS/ERANGE, `21` EFAULT, `28` EINVAL, `44`
  ENOENT, `54` ENOTDIR, `76` ENOTCAPABLE. Page tables note per-fn deviations.
- **"traps"**: the host suspends the fiber and resumes it when an async op (VFS,
  socket, sleep) completes — transparent to the guest (the call just blocks).
- **packed returns**: `i64` returns often pack two `u32` as `(hi << 32) | lo`.
- **Source** links point at the kernel registration (`func_wrap(...)`) so the doc
  can be checked against truth.

## Index

- **GUI app author API** — [ruos-window.md](ruos-window.md) ← start here
- **GUI raw host ABI** — [wm.md](wm.md), [sys.md](sys.md), [term.md](term.md), [gfx.md](gfx.md)
- **CLI tools** — [ruos.md](ruos.md), [wasi.md](wasi.md)
- **Component model** — [wit.md](wit.md)

---

## Maintenance rule (mirrored in `/CLAUDE.md`)

This manual MUST stay complete and precise. **Whenever you add, remove, or change an
app-facing host function** — any `func_wrap("wm"|"sys"|"term"|"ruos", …)` in
`kernel/src/wasm/wt/*` or `kernel/src/wasm/host/*`, or a `wit/*.wit` interface —
**update the matching page in the SAME change**:

1. Add/edit the function's entry (signature + params + returns + semantics).
2. For GUI modules, also update the `extern "C"` block in
   `ruos-desktop/crates/ruos-window/src/lib.rs`.
3. Bump the page's **Last reviewed** date.
4. Add a `CHANGELOG/NN-…md` entry.

`demo-apps-sdk` copies this whole `docs/api/` folder into each scaffolded project
(as `api/`) so app authors have the manual offline.
