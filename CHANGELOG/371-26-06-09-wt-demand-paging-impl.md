# 371 — demand paging della linear-memory Wasmtime (impl)

**Data:** 2026-06-09

## Cosa

Implementato il demand paging della VA window Wasmtime (linear memory + codice
AOT), come da spec
`docs/superpowers/specs/2026-06-09-wt-linear-mem-demand-paging-design.md`.

- **`kernel/src/wasm/wt/demand.rs`** (nuovo): registry dei range WT
  (`WtRange{base,end,prot}`, `IrqMutex<Vec>` leaf-lock) + bump allocator della VA
  window (`WT_VM_BASE`, spostato qui da `platform.rs`). API: `reserve`,
  `set_prot` (con split), `remove`, `in_window`, `commit_fault`. `self_test`
  (boot-checks): verifica che un range riservato-ma-non-toccato costi 0 frame e
  che un touch committi lazy una pagina azzerata.
- **`kernel/src/wasm/wt/platform.rs`**: `wasmtime_mmap_new` ora SOLO riserva VA +
  registra il range (zero frame committati); `wasmtime_mprotect` registra il
  nuovo prot e flippa i flag solo delle pagine già present (W^X), tollerando le
  not-present; `wasmtime_munmap`/`wasmtime_mmap_remap` liberano solo i frame
  present e aggiornano il registry. Rimossi il commit eager e lo
  zeroing-per-pagina (ora avviene on-fault via HHDM alias).
- **`kernel/src/idt.rs`**: `pf_handler` chiama `demand::commit_fault` sui fault
  not-present dentro la WT window (resume); i PROTECTION_VIOLATION e i fault
  fuori window cadono nel path panic invariato. Passa l'IF del contesto faultante
  per riabilitare gli IRQ durante lo spin su MAPPER (evita deadlock vs TLB
  shootdown).
- **`kernel/src/boot/phases/interrupts.rs`**: aggiunto il `demand::self_test` ai
  boot-checks.

## Perché

L'OOM all'apertura della 3ª finestra desktop era esaurimento frame: ogni minimo
dichiarato di linear memory (48 MiB/finestra) committava tutti i frame
all'instantiate, ~95% mai toccati. Ora il minimo costa solo le pagine toccate
(~pochi MiB/finestra) → molte più finestre nello stesso budget RAM, senza rebuild
delle app né cambio dell'hash AOT (`memory_reservation(0)` invariato).

## Verifica

`make test-boot` (boot-checks) → `TEST_BOOT_PASS`. Dal log seriale:
`exec W^X self-test ok`, `linear-mem zero-init self-test ok`,
`linear-mem demand-paging self-test ok`, `wasmtime AOT hello ok`,
`wasip1 probe spawn ok`, `egui-demo spawn ok` con **due finestre egui live
concorrenti** (`live=2`) e `spc flags=0b11` — lo scenario che prima andava OOM —
senza alcun #PF/panic.

NB ambiente: serviva `wasm-tools` (mancante) per generare i `.cwasm` component
dei boot-checks; installato via `cargo install wasm-tools`.

## File toccati
- kernel/src/wasm/wt/demand.rs (nuovo)
- kernel/src/wasm/wt/platform.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/idt.rs
- kernel/src/boot/phases/interrupts.rs
- docs/superpowers/specs/2026-06-09-wt-linear-mem-demand-paging-design.md
