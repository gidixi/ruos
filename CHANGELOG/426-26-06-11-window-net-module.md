# 426 — `net` host module for GUI windows (TCP + DNS, poll-based)

**Data:** 2026-06-11

Phase 4 of the Blitz viewer needs network from a window app, but the Wasmtime
window path is fully synchronous (no fiber/epoch) — the wasmi `ruos.tcp_dial`
suspend-based design cannot be reused, and a blocking host fn would freeze the
desktop.

- `wt/net.rs` (new): module `net` on every window linker —
  `resolve_start/resolve_poll` (DNS via `net::dns::resolve` spawned as an
  embassy task onto the BSP with `spawn_on(0, …)`, result parked in a slot
  table), `dial` (alloc Ethernet TCP + `connect_start`, returns immediately),
  `state` (0 connecting / 1 established / 2 closed), `read`/`write`
  (non-blocking, -1 = would-block), `close`. All fns return in O(1); apps poll
  from their frame loop.
- `net/sockets.rs`: the sync first half of `connect` extracted as
  `connect_start` (the async `connect` now calls it); `state_of` coarse state
  query for poll-based callers.
- Registered at all four `Linker<AppState>` build sites (catalog probe, reactor
  spike, `Compositor::new`, `new_empty`).
- `docs/api/net.md` (new page) + index updates.

Same smoltcp pool as the CLI path; `net_poll_task` (BSP) keeps driving the
stack, so the compositor core never waits on the network.
