# 453 — wt WASI: clock REALTIME ancorato all'RTC (prerequisito TLS in-app)

**Data:** 2026-06-11

## Cosa

`clock_time_get` nello shim WASI delle finestre Wasmtime (`wt/wasi.rs`) ora
distingue il clock id: `0` (REALTIME) restituisce ns unix-epoch (epoch RTC
ancorato al primo uso, cache in `AtomicU64` — CMOS letto una volta sola, non a
ogni `SystemTime::now()`); gli altri id restano ns-since-boot dal timer 100 Hz.
Nota aggiunta in `docs/api/wasi.md`.

## Perché

Spike TLS app-side (opzione A1): `rustls 0.23` (default-features off, std+tls12)
+ provider puro-Rust `rustls-rustcrypto 0.0.2-alpha` + `webpki-roots 0.26`
**compila pulito su `wasm32-wasip1`** — 1.03 MiB di .wasm con tutto il root CA
bundle; import richiesti: `random_get`, `clock_time_get`, `environ_*`,
`fd_write`, `proc_exit`, tutti già forniti dallo shim wt. Unico blocco runtime:
prima di questo fix `SystemTime::now()` nel guest dava ~1970+uptime → rustls
avrebbe rifiutato ogni certificato come "not yet valid". Con il fix il path
HTTPS lato app non richiede altro dal kernel.

## File toccati

- kernel/src/wasm/wt/wasi.rs
- docs/api/wasi.md
