# TLB shootdown batch + manifest cache launcher — fix freeze compositor multi-core

**Data:** 2026-06-10
**Stato:** approvato (root-cause confermata da indagine multi-agente + repro)

## Problema

Su macchine con molti core (HW reale 16 core, VBox 16 vCPU) il lancio del
compositor (e di ogni app `.cwasm`) blocca il GUI core per minuti/ore
(supervisor: `mute cores=1 alive=15/16`), poi parte. Con pochi core tutto ok.

### Causa radice (confermata)

Tempesta di TLB shootdown per-pagina durante publish/teardown dei moduli AOT:

1. `Module::deserialize` copia il `.cwasm` in pagine RW (demand-paging, 0 IPI).
2. Il publish Wasmtime fa `make_readonly(intera immagine)` + `make_executable(.text)`.
3. `wasmtime_mprotect` (`platform.rs:216`) itera **per pagina** su
   `memory::set_flags`; ogni `set_flags` (`mapper.rs:149`) fa **un broadcast
   IPI `VEC_TLB_SHOOTDOWN` a tutti gli altri core e spin-wait di N-1 ack**,
   tenendo il lock MAPPER. Idem `wasmtime_munmap` (`platform.rs:194`) via
   `unmap_page` al teardown.
4. Il launcher (`wm.rs scan_apps`/`module_at_stem`) deserializza+istanzia+droppa
   **ogni** `.cwasm` di `/bin` (~54MB) per leggerne il manifest, senza cache →
   publish+munmap storm ripetuto.

Numeri: shell.cwasm 9.6MB ≈ 3.4k shootdown; primo scan launcher ≈ 33-36k;
totale bring-up ≈ **38-40k broadcast sincroni**. Su VBox-16 ogni rendezvous
costa ~ms-150ms (scheduling host di 15 vCPU) → ore. Il timeout per-shootdown
(2e9 spin) non scatta mai → zero warn nel log. `shootdown()` è ~no-op con <2
core → "con pochi core va veloce".

Refutate (indagine): busy-spin AP idle (hlt corretto, zero task), timestamp TSC
gonfiati (ricalibrazione PM-timer pre-SMP, T+ wall-true), costo intrinseco
demand-paging (~3-6µs/fault, no IPI), contesa heap (AP idle non allocano,
inflate allocation-free). Repro QEMU TCG -smp 1/4/16: unpack piatto → la
lentezza generale pre-GUI vista su VBox-16 è oversubscription dell'host VBox,
non bug ruos.

## Fix

### 1. `tlb.rs` — shootdown a range

- `shootdown_range(virt: u64, pages: usize)`: pubblica `(addr, len)` (atomics,
  un solo shootdown in flight — serializzato dal lock MAPPER come oggi), un
  broadcast `send_ipi_all_but_self`, spin-wait ack come oggi (stesso bound +
  warn TIMEOUT).
- Handler `on_ipi()`: se `len <= FLUSH_THRESHOLD` (32 pagine) → loop `invlpg`;
  altrimenti **full flush** (reload CR3). Vincolo: verificare che le pagine WT
  non abbiano il flag GLOBAL (il reload CR3 non flusha le global) — se lo
  avessero, fallback invlpg-loop o clear del flag.
- `shootdown(virt)` resta = `shootdown_range(virt, 1)` (compat per i caller
  esistenti single-page).
- Telemetria: contatori atomici `SHOOTDOWNS`, `FULL_FLUSHES`, esposti via
  `tlb::stats()`; il wm li logga (binfo) a compositor pronto.

### 2. `mapper.rs` — API range

- `set_flags_range(base: VirtAddr, pages: usize, flags)`: UNA acquisizione
  MAPPER, aggiorna gli N PTE (skippa le not-mapped, come fa oggi il loop del
  platform), **un solo** `shootdown_range` finale e SOLO se almeno una pagina
  era present.
- `unmap_range(base: VirtAddr, pages: usize) -> usize`: unmappa gli N PTE
  (skip not-mapped), libera i frame (stessa semantica di `unmap_page` +
  `free_frame`), un solo `shootdown_range` finale se serve. Ritorna il numero
  di pagine unmappate.
- `set_flags`/`unmap_page` single-page restano (wrapper o invariati).

### 3. `platform.rs` — usa le API range

`wasmtime_mprotect`, `wasmtime_munmap`, `wasmtime_mmap_remap`: sostituire i
loop per-pagina con UNA chiamata range. Semantica identica (skip not-mapped,
stessi flag, stessa gestione errori).

### 4. `wm.rs` — cache manifest del launcher

- Cache statica `stem → (file_size, manifest)` (IrqMutex<BTreeMap>): lo scan
  ri-proba un `.cwasm` (deserialize+instantiate+drop) SOLO se nuovo o di size
  diversa; altrimenti riusa il manifest cached. File rimossi → entry rimossa.
- Mantiene l'hot-plug della drop-folder (`/mnt/apps`): la LISTA file si
  rilegge a ogni refresh (readdir economico), solo il PROBE è cached.

## Invarianti preservate

- Un solo shootdown in flight (MAPPER held) — gli handler leggono atomics
  stabili.
- `commit_fault` re-enable IRQ mentre spinna su MAPPER (deadlock-avoidance) —
  invariato.
- Shootdown no-op con `cpus_online() < 2` — preservato nel range.
- Nessun cambiamento al fault-path demand paging (già IPI-free).

## Test

- Boot-check (feature `boot-checks`): il test no-fault remap esistente
  (interrupts.rs) resta verde; nuovo check range: mappa N pagine, le tocca su
  un altro core, `set_flags_range`+`unmap_range`, verifica traduzioni e che
  `stats()` conti 1 shootdown per l'intero range.
- `make run-test` → TEST_PASS invariato.
- Misura: log `tlb stats` a compositor su; su VBox-16 atteso crollo da ~40k a
  ~decine di shootdown e lancio in secondi. Verifica finale su HW reale.

## File toccati (previsione)

- kernel/src/memory/tlb.rs
- kernel/src/memory/mapper.rs
- kernel/src/wasm/wt/platform.rs
- kernel/src/wasm/wt/wm.rs
- kernel/src/boot/phases/interrupts.rs (solo boot-check nuovo, gated)
