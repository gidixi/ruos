# Demand paging della linear-memory Wasmtime — design

**Data:** 2026-06-09
**Stato:** implementato (CHANGELOG 364) — boot-checks: demand-paging + zero-init
self-test ok; due finestre egui live concorrenti senza OOM in `test-boot`.
**Area:** `kernel/src/wasm/wt/platform.rs`, `kernel/src/idt.rs`, `kernel/src/memory/`

> **CORREZIONE (2026-06-10, changelog 422):** la premessa "con
> `memory_reservation(0)` la linear memory passa da `wasmtime_mmap_new`" è
> **errata**: in wasmtime 45 con `signals_based_traps(false)` + reservation 0 +
> guard 0 + cow off la linear memory usa `MallocMemory` (heap talc), MAI il path
> mmap — il demand paging di questa spec copriva quindi solo codice AOT e altri
> usi di Mmap. Il whack-a-mole di HEAP_SIZE (16→128→256→384 MiB, changelog 366)
> era il sintomo. Fix: `memory_reservation(256 MiB)` (kernel + wt-precompile,
> valore hashato nel .cwasm) → MmapMemory → demand paging reale sui frame.

## Problema

Aprire la 3ª finestra del desktop egui fallisce con:

```
spawn: instantiate failed: failed to allocate 0x3000000 bytes
(0x3000000 minimum + 0x0 memory_reservation_for_growth)
```

`0x3000000` = 48 MiB = il **minimo dichiarato** della linear memory di ogni app
egui (`--initial-memory=50331648` in `ruos-desktop/.cargo/config.toml`).

### Modello di memoria reale

Due sistemi distinti:

| Sistema | Backa | Provenienza |
|---------|-------|-------------|
| talc heap (`HEAP_SIZE`, `heap.rs`) | Rust `alloc` kernel-side (byte `.cwasm`, store, dati host egui) | 1 regione USABLE |
| frame allocator (`frames.rs`) | `map_page` → **linear memory WASM + codice AOT** | tutta la RAM **meno** l'heap |

`frames.rs:187-193` marca la regione heap come *used*, quindi
`frame_disponibili = RAM_totale − HEAP_SIZE − kernel`.

La linear memory WASM viene da `allocate_frame()` (`platform.rs:178`), **non
dall'heap**. L'OOM è **esaurimento frame**, non heap. Corollario importante:
alzare `HEAP_SIZE` *riduce* i frame → da solo peggiora il numero di finestre; la
leva è la RAM (`-m`).

### Causa radice

`wasmtime_mmap_new` (`platform.rs:159-195`) **committa un frame fisico per ogni
pagina subito**, persino per il reserve `PROT_NONE` e persino per le pagine che
il guest non tocca mai. Wasmtime, con `memory_reservation(0)`, riserva esattamente
il minimo (48 MiB) come `PROT_NONE` e poi lo rende accessibile (`make_accessible`
→ `wasmtime_mprotect(.., RW)`). Risultato: **12288 frame committati per finestra,
~95% mai toccati**.

Il footprint reale di una finestra egui (font ~1.5 MiB + raster ~2-4 MiB + heap
guest) sta in pochi MiB. Stiamo sprecando ~10× RAM per finestra.

## Obiettivo

Rendere la linear memory (e il codice AOT) **demand-paged**: il minimo dichiarato
costa solo le pagine effettivamente toccate. Niente rebuild delle app, niente
cambio dell'hash AOT (resta `memory_reservation(0)` → `tools/wt-precompile` e
`kernel/src/wasm/wt/mod.rs:engine_config` restano allineati e immutati).

Risultato atteso: ~6-10× densità finestre nello stesso budget di RAM, e la classe
di OOM "minimo dichiarato non entra" sparisce (si paga il toccato, non il
dichiarato).

## Vincoli e fatti della piattaforma

1. **`MAPPER`** è `spin::Mutex` (`mapper.rs:16`), lock order **MAPPER → FRAMES**,
   mai invertito.
2. `map_page` su pagina **not-present non fa TLB shootdown** (`mapper.rs:99-104`):
   x86 non cachea entry negative. → commit-on-fault è O(1) locale, **nessun IPI**.
3. `set_flags`/`unmap_page` **fanno** shootdown: broadcast `VEC_TLB_SHOOTDOWN` +
   attesa ack **con IRQ abilitati** (vedi `tlb.rs`). Questo è il rischio deadlock
   da gestire nel #PF handler.
4. `signals_based_traps(false)` (`wt/mod.rs:267`) → Wasmtime emette **bounds-check
   inline**, NON usa guard-page trap. Il guest perciò **non fa mai fault** in una
   pagina `PROT_NONE`. → un #PF in una pagina WT `PROT_NONE` registrata = **bug
   reale** (o accesso kernel errato), non un lazy-commit: deve fare panic come oggi.
5. `pf_handler` (`idt.rs:78`) è `extern "x86-interrupt"` **senza `!`** → può già
   ritornare (#PF risolvibile). Oggi stampa e `halt()`.
6. La VA window Wasmtime è `[WT_VM_BASE = 0xFFFF_D000_0000_0000, NEXT)` con
   `NEXT` bump-allocator monotono (`platform.rs:137-138`). Distinta da
   `memory::exec`.
7. WASM su più core concorrenti (commenti `platform.rs:20`, `:46`) → registry e
   #PF handler devono essere SMP-safe.

## Design

### 1. Registry dei range WT

Struttura globale lock-protected che mappa ogni sotto-range della WT window al suo
`prot` corrente:

```rust
struct WtRange { base: u64, end: u64, prot: u32 }   // end esclusivo, prot bits READ/WRITE/EXEC
static WT_RANGES: IrqMutex<Vec<WtRange>> = ...;       // o array a capacità fissa
```

- **Lock leaf-level**: si prende SOLO per leggere/aggiornare il registry, MAI
  tenuto mentre si chiama `map_page`/`set_flags` (che prendono MAPPER). Niente
  inversione di lock order: registry-lock → release → MAPPER.
- Lookup nel #PF: O(n) su n = numero di mapping vivi (poche decine). Se diventa
  caldo, indicizzare per pagina; per ora lineare basta.

### 2. `wasmtime_mmap_new` → reserve, niente commit

Nuovo comportamento:
- `prot == PROT_NONE` (il reserve di Wasmtime): **assegna solo VA** (`NEXT.fetch_add`),
  registra `WtRange{base, end, prot:0}`, **committa zero frame**. Restituisce base.
- `prot` con R/W/X: registra il range con quel prot, **committa zero frame**
  (lazy anche qui — il #PF farà il commit al primo touch). Vedi nota EXEC sotto.

### 3. `wasmtime_mprotect` → aggiorna prot, lazy

- Aggiorna il `prot` del range (o spezza il range se la mprotect copre solo una
  parte — gestione split/merge dei `WtRange`).
- Per ogni pagina **già present** del range: `set_flags(prot)` (transizione W^X
  reale, con shootdown — corretto, è raro: load del codice, teardown).
- Per ogni pagina **not-present**: niente — verrà mappata col nuovo prot al fault.
- NON ritorna errore su pagine not-present (oggi `set_flags` → `NotMapped` →
  ritorno 1: va cambiato).

### 4. `pf_handler` → commit-on-fault

Flusso (solo se `cr2 ∈ WT window`):
1. registry-lock → trova il `WtRange` che copre `cr2` → copia `prot` → unlock.
2. Se nessun range, oppure `prot == 0` (PROT_NONE) → **path attuale** (stampa +
   panic/halt): è un bug reale, non un lazy-commit (vincolo #4).
3. Altrimenti: `allocate_frame()` → `map_page(page_base, frame, flags(prot)|PRESENT)`
   con la pagina azzerata (garanzia zero-init WASM, come fa già `mmap_new`).
   - **Race SMP**: se `map_page` ritorna `AlreadyMapped`, un altro core ha già
     committato la stessa pagina → libera il frame appena preso, considera
     risolto, ritorna (resume).
   - Niente shootdown (vincolo #2): not-present → present.
4. Ritorna dall'handler → la CPU riesegue l'istruzione faultante sulla pagina ora
   presente.

**Disciplina IRQ (deadlock, vincolo #3):** il #PF handler prende `MAPPER`. Se un
altro core tiene `MAPPER` per una `set_flags`/`unmap` (che fa broadcast IPI +
attende ack), il core faultante che spinna su `MAPPER` con IRQ disabilitati non
servirebbe mai lo shootdown IPI → deadlock. Mitigazione: **abilitare gli IRQ
(`sti`) prima di spinnare su `MAPPER`** nel #PF handler, così serve gli IPI
mentre aspetta. Sicuro perché:
- il commit-on-fault è lavoro indipendente (non tiene altri lock cross-core);
- nessun fault annidato su pagine WT: l'handler dopo `map_page` scrive la pagina
  ora *present* (lo zeroing) → no re-fault; tocca solo memoria kernel present.
- un timer IRQ durante l'attesa è benigno (ritorna).
Va verificato il gate del #PF nell'IDT (interrupt-gate → IF=0 all'ingresso):
documentare/forzare lo stato IRQ esplicitamente nell'handler.

### 5. `wasmtime_munmap` / `wasmtime_mmap_remap` / teardown

- `munmap(range)`: per ogni pagina, `unmap_page` **tollerando not-present**
  (pagine mai toccate non hanno frame → skip, niente errore). Libera solo i frame
  present. Rimuove il `WtRange` dal registry.
- `mmap_remap` (blank/reset, usato da grow/reset): stesso principio — libera i
  present, ri-registra il range lazy col nuovo prot, niente commit eager.
- Garanzia anti-leak: il conteggio frame liberati = solo i present → nessun
  doppio-free, nessun leak dei reserved-mai-toccati (non avevano frame).

### 6. Nota EXEC (codice AOT)

Il publish del codice di Wasmtime: `mmap_new(PROT_NONE)` reserve → `mprotect(RW)`
→ memcpy del codice macchina → `mprotect(RX)`.
- Al memcpy il `prot` registrato è RW → i write faultano e committano RW. ✓
- `mprotect(RX)` fa `set_flags` sulle pagine present (W^X) e aggiorna il prot. ✓
- Un eventuale instruction-fetch su pagina di codice not-present faulta con prot
  EXEC registrato → `map_page` con flag eseguibili. ✓

Quindi **un solo meccanismo** demand-paga dati e codice; il discriminante è il
`prot` registrato, non il tipo di range.

### 7. Self-test (`boot-checks`)

Adattare `zero_init_self_test` (`platform.rs:297`) al path lazy: il `read` dopo
`mmap_new(NONE)`+`mprotect(RW)` deve far scattare un #PF kernel-side che committa
una pagina **azzerata** → la lettura torna zero. Diventa un test end-to-end del
demand paging (registry + #PF + zero-init) oltre che del solo zero-init.
Aggiungere un test che verifica che le pagine **mai toccate non consumino frame**
(confronto `frame_counts()` prima/dopo un `mmap_new` grande non toccato).

## Ottimizzazioni (fuori dal primo taglio)

- **Batch fault**: mappare N pagine contigue per fault (es. 16) per ammortizzare
  il costo dei ~2300 fault one-time del codice AOT (~9 MiB). Misurare prima.
- **Registry indicizzato** per pagina se il lookup lineare diventa caldo.
- **Reclaim VA**: `NEXT` non riusa la VA dei range smontati. 48-bit di spazio →
  irrilevante a breve; eventuale free-list dopo.

## Rischi

| Rischio | Mitigazione |
|---------|-------------|
| Deadlock MAPPER vs shootdown nel #PF | IRQ abilitati durante lo spin su MAPPER (§4) |
| Fault annidato nel handler | handler tocca solo memoria present dopo il map (§4) |
| Race due core stessa pagina | `AlreadyMapped` → free del frame extra (§4) |
| `mprotect` su pagine not-present rompe il load codice | mprotect lazy + set_flags solo sui present (§3, §6) |
| Bug reali mascherati da lazy-commit | solo R/W/X committano; PROT_NONE → panic (§4, vincolo #4) |
| Leak/double-free al teardown | libera solo i present, tollera not-present (§5) |

## Fix correlato (separato da questa feature)

Indipendente dal demand paging: il commit #362 ha alzato `HEAP_SIZE` a 384 MiB,
ma poiché la linear memory = frame, alzare l'heap *riduce* i frame. Per
massimizzare le finestre conviene **heap basso + RAM alta** (es. heap 256 +
`-m 1024` → ~768 MiB di frame vs ~640 con heap 384). Da valutare a parte; il
commento storico in `heap.rs` che attribuiva i 48 MiB all'heap è fuorviante e va
corretto. Con il demand paging questa tensione si attenua molto (le finestre
costano il toccato, non il dichiarato).

## Piano di implementazione (abbozzo)

1. `WtRange` registry + helper lookup/insert/update/split/remove (+ test unit).
2. `mmap_new`/`mprotect`/`munmap`/`mmap_remap` → reserve+lazy, aggiornano il
   registry, niente commit eager.
3. `pf_handler` → commit-on-fault con disciplina IRQ; fallback panic invariato.
4. Adatta `zero_init_self_test` + nuovo test "untouched = 0 frame".
5. `make iso CARGO_FEATURES=boot-checks` + `make run`: apri 6+ finestre, verifica
   niente OOM e che `free`/`frame_counts` cresca col toccato, non col dichiarato.
6. Misura i fault di startup; se troppi, batch (§Ottimizzazioni).
