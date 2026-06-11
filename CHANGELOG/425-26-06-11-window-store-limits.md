# 425 — StoreLimits on live window stores

**Data:** 2026-06-11

The Wasmtime-AOT window path had NO ResourceLimiter: any GUI app could
`memory.grow` until kernel RAM ran out and take the whole desktop down (the
48 MiB in the SDK's `.cargo/config.toml` is a link-time *initial* size, not a
host cap).

- `wt/wm.rs`: `AppState.limits: wasmtime::StoreLimits` + `WINDOW_MEM_CAP`
  (128 MiB). `spawn_named` — the one live-window instantiation path — installs
  `store.limiter(|s| &mut s.limits)` with `memory_size(WINDOW_MEM_CAP)`; a grow
  past the cap fails inside the guest (its allocator OOMs, the app aborts), the
  kernel and other windows are untouched.
- Throwaway stores (manifest probe, reactor spike) keep the unlimited default.
- Sizing: heaviest measured guest (Blitz viewer, 6.4k-node page) peaks ~16 MiB
  heap on a 48 MiB initial memory — 128 MiB is generous headroom.
