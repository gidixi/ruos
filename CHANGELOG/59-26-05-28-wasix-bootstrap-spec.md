# 59 — Spec design: WASIX bootstrap (Step 10)

**Data:** 2026-05-28

## Cosa

Scritta spec Step 10 in
`docs/superpowers/specs/2026-05-28-rust-wasix-bootstrap-design.md`.

Pivot dalla roadmap: WASI Preview 1 → **WASIX** (Wasmer Inc, superset
di WASI P1). Motivazione: ecosystem WASIX pre-built (bash, python, vim,
curl) sblocca userland reale a Step 13 senza scrivere shell custom.

Decisioni strategiche (brainstorm):

- Runtime: `wasmi` 0.36 `default-features=false` (pure Rust, no_std,
  interprete).
- WASM loading: Limine modules (Opt 3) — N moduli dichiarati in
  `limine.conf`, kernel li monta in tmpfs.
- Sockets: smoltcp loopback (Opt C) — TCP funzionale su 127.0.0.1, no
  NIC reale fino a Step 14.
- Scope D3 monolitico: ~25 host fns, demo = `init.wasm` (welcome) +
  `server.wasm`/`client.wasm` (ping/pong TCP).
- Wasm task lifecycle: ogni `.wasm` in embassy task suo; host fns
  sync con `embassy_futures::block_on` interno.
- CSPRNG: weak xorshift seedato da TICKS; RDRAND a Step 14.

Decomposizione in 6 task implementativi:

1. Limine modules → VFS mount.
2. wasmi up + lifecycle host fns + `fd_write` a console.
3. VFS-backed `fd_*` + `path_*` host fns.
4. clock + random + remaining stdio.
5. smoltcp + Loopback + `net_poll_task`.
6. `sock_*` host fns + demo ping/pong.

Smoke test HELLO finale: `ruos: client.wasm: rx='pong'`.

Out of scope: `proc_fork`/`proc_exec`, `thread_spawn`, signals, TTY
ioctl, DNS, IPv6.

## Perché

Step 10 della roadmap WASM-first, scope esteso a "WASIX bootstrap
completo" per scelta utente (Opt B monolitico nel brainstorm). Fine
Step 10 = ruos esegue programmi reali `.wasm` con file I/O + TCP
loopback.

## File toccati

- docs/superpowers/specs/2026-05-28-rust-wasix-bootstrap-design.md (nuovo)
- CHANGELOG/59-26-05-28-wasix-bootstrap-spec.md (nuovo)
