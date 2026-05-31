# 178 — WASM per-task resource limits: fd cap, linear-mem limiter, socket cap

**Data:** 2026-05-31

## Cosa

Tre meccanismi distinti che impediscono a un singolo modulo `.wasm` di esaurire
l'heap del kernel condiviso:

### 1. Costanti limite in `kernel/src/wasm/state.rs`

```rust
pub const MAX_FDS:        usize = 128;
pub const MAX_SOCKETS:    usize = 16;
pub const MAX_LINEAR_MEM: usize = 64 * 1024 * 1024;
```

### 2. `wasmi::ResourceLimiter` implementato su `RuntimeState`

Signatures copiate verbatim da `wasmi_core-1.0.9/src/limiter.rs` (il trait è
definito lì e re-esportato come `wasmi::ResourceLimiter`):

```rust
fn memory_growing(&mut self, _current: usize, desired: usize, maximum: Option<usize>)
    -> Result<bool, wasmi_core::LimiterError>

fn table_growing(&mut self, _current: usize, desired: usize, maximum: Option<usize>)
    -> Result<bool, wasmi_core::LimiterError>

fn instances(&self) -> usize  // 10_000 (wasmi DEFAULT)
fn tables(&self)    -> usize  // 10_000
fn memories(&self)  -> usize  // 10_000
```

`memory_growing` rifiuta crescite oltre `MAX_LINEAR_MEM` (64 MiB), rispettando
anche il `maximum` dichiarato dal modulo. `table_growing` usa cap 4096 elementi
se il modulo non ne specifica uno.

`wasmi_core = "1.0.9"` aggiunto come dipendenza diretta in `kernel/Cargo.toml`
per rendere `wasmi_core::LimiterError` raggiungibile (non è re-esportato
pubblicamente da `wasmi`).

### 3. Limiter agganciato allo `Store` in `Fiber::new`

```rust
store.limiter(|state| state as &mut dyn wasmi::ResourceLimiter);
```

Firma di `Store::limiter` da `wasmi-1.0.9/src/store/mod.rs`:
```
pub fn limiter(&mut self, limiter: impl (FnMut(&mut T) -> &mut dyn ResourceLimiter) + Send + Sync + 'static)
```

### 4. Fd table cappata ai siti push in `fiber.rs`

Entrambi i siti guest-driven (arms `PathOpen` e `OpenDir` nel dispatch loop)
ora usano `match` con guard `fds.len() < MAX_FDS` anziché `unwrap_or_else`:
- Slot `None` libero trovato → riusa (invariato).
- Nessuno slot + `len < MAX_FDS` → push (invariato).
- Nessuno slot + `len >= MAX_FDS` → `return 24` (EMFILE).

I tre slot 0/1/2 (stdio) sono allocati in `RuntimeState::new()` — non passano
per questi siti, non sono capped.

### 5. Socket cap in `ruos_tcp_dial` (host/proc.rs)

Prima di allocare il socket kernel, conta le voci `FdEntry::Socket` nella fd
table; se `>= MAX_SOCKETS` restituisce `Ok(24)` (EMFILE).
Il sito di push fd per i socket è anch'esso bounded con `MAX_FDS` (guard
`fds.len() < MAX_FDS`, altrimenti `Ok(24)`).

## Perché

Tutto gira in ring-0 su un heap condiviso. Un modulo che chiama `path_open` in
loop, cresce la memoria lineare senza limite, o apre socket in loop esaurisce
l'heap globale → il sistema intero crasha, non solo il task offensivo. Questi
cap fanno sì che la risorsa esaurita ritorni un errno al modulo offensivo invece
di OOM-killare il kernel.

## File toccati

- kernel/Cargo.toml
- kernel/src/wasm/state.rs
- kernel/src/wasm/fiber.rs
- kernel/src/wasm/host/proc.rs
