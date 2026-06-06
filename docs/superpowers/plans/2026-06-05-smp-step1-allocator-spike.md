# Step 1 — Per-core allocator SPIKE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decidere, con dati, l'architettura dell'allocatore per-core (Step 1 della
migrazione SMP shared-nothing) costruendo DUE prototipi sotto feature flag + uno
strumento di benchmark, misurando latenza alloc/free single-core e contesa multi-core,
e registrando la decisione. NON implementa l'allocatore di produzione finale (dipende
dai numeri).

**Architecture:** Tre alternative del `#[global_allocator]`, selezionate da feature
cargo mutuamente esclusive (default = talc globale di oggi, invariato):
- `alloc-magazine` → **Prototipo A**: cache per-core per size-class (magazine) davanti a
  UN talc globale. Small alloc/free colpiscono la cache locale; miss/overflow → talc
  globale. Free cross-core banale (qualsiasi blocco torna al talc globale che possiede
  tutto lo heap). Niente split, niente remote-free queue.
- `alloc-percore-talc` → **Prototipo B**: N istanze `Talc` su sub-span disgiunti dello
  heap, indicizzate da `cpu_id()`. Free cross-core instradato all'owner per range +
  remote-free queue per-core. Big alloc → fallback talc globale.
Uno strumento di benchmark (boot-check) misura entrambi in modo identico: latenza TSC
per alloc/free small+large single-core, e un job pool multi-core che stressa la contesa.

**Tech Stack:** Rust `no_std`, `talc` 4, `spin`/`IrqMutex`, `smp::pool` (job
`fn(&[u8])->u64`), TSC (`boot::clock::read_tsc`), boot-check markers (`binfo!`),
`make test-boot` + `make run-smp-test`.

**Decisione baked-in (brainstorm 2026-06-05):** l'utente ha scelto "prototipa entrambe
+ misura". Questo piano termina a un **gate di decisione**; l'allocatore finale è il
piano successivo, scritto sui dati raccolti qui.

**Nota sullo spec:** la spec macro §6 descriveva l'opzione B (arena per-core +
remote-free queue) come design. Questo spike mette B a confronto con A (magazine). Se
A vince, §6 va aggiornato (niente remote-free queue). La Task 6 registra l'esito.

---

## File Structure

- `kernel/Cargo.toml` — aggiunge le feature `alloc-magazine`, `alloc-percore-talc`
  (mutuamente esclusive; default nessuna = talc globale invariato).
- `kernel/src/memory/heap.rs` — definizioni `#[global_allocator]` cfg-gated (default /
  A / B). Espone `heap_span() -> (u64 base, usize size)` e `init_heap()` invariato nel
  comportamento default.
- `kernel/src/memory/alloc_magazine.rs` — **nuovo**, Prototipo A (solo sotto
  `alloc-magazine`). `MagazineAlloc` (`GlobalAlloc`) + size-class table + magazine
  per-core.
- `kernel/src/memory/alloc_percore_talc.rs` — **nuovo**, Prototipo B (solo sotto
  `alloc-percore-talc`). `PerCoreTalc` (`GlobalAlloc`) + array `Talc` + range-routing +
  remote-free queue.
- `kernel/src/memory/allocbench.rs` — **nuovo**, lo strumento di benchmark (solo sotto
  `boot-checks`). Latenza single-core + job multi-core + marker.
- `kernel/src/boot/phases/mem.rs` — chiama `allocbench::run_single_core()` (sotto
  `boot-checks`) dopo `init_heap`.
- `kernel/src/boot/phases/interrupts.rs` — chiama `allocbench::run_multicore()` (sotto
  `boot-checks`) DOPO `smp::bringup()` (gli AP devono essere online).
- `kernel/src/memory/mod.rs` — re-export dei moduli nuovi (cfg-gated).
- `CHANGELOG/297-…`, `CHANGELOG/298-…`, … una entry per modifica (vedi Task).
- `docs/superpowers/decisions/2026-06-…-allocator-architecture.md` — **nuovo**, record
  di decisione (Task 6).

**Confini:** ogni prototipo è in un file dedicato dietro la sua feature → il default
build resta byte-identico a oggi (zero regressione finché non si attiva una feature).
Lo strumento di benchmark è separato dai prototipi (misura tutti e tre identicamente).

---

## Task 1: Baseline globali (Step 7 accorpato, doc-only)

Formalizza l'audit dei globali (è `CHANGELOG/186` promosso a baseline della migrazione)
e documenta l'ordine lock. Zero codice funzionale → zero rischio, sblocca i target di
contesa che il benchmark misurerà.

**Files:**
- Modify: `kernel/src/memory/heap.rs` (commento doc su `ALLOCATOR`)
- Modify: `kernel/src/memory/mapper.rs:13` (commento ordine lock)
- Modify: `kernel/src/memory/frames.rs:144` (commento ordine lock)
- Create: `CHANGELOG/297-26-06-05-smp-step7-baseline-globali.md`

- [ ] **Step 1: Aggiungi il commento d'invariante su `ALLOCATOR`**

In `heap.rs`, sopra `pub static ALLOCATOR` (riga 18), aggiungi:

```rust
// SMP baseline (migrazione shared-nothing, spec 2026-06-05): questo è un VERO
// spinlock SMP (spin 0.9.8, CAS cross-core), non uno stub single-core. È preso su
// OGNI alloc/free di OGNI core → è il collo di contesa #1 quando arriveranno gli
// executor per-core (Step 3). Step 1 lo affianca con arene per-core (vedi
// memory/alloc_magazine.rs / alloc_percore_talc.rs). NON è un problema di safety
// (audit CHANGELOG/186: 0 must-fix), è un problema di CONTESA.
```

- [ ] **Step 2: Documenta l'ordine lock in `mapper.rs` e `frames.rs`**

In `mapper.rs` sopra `static MAPPER` (riga 13):
```rust
// ORDINE LOCK (invariante SMP): MAPPER.lock() PRIMA di FRAMES.lock(), mai invertito.
// map_page acquisisce MAPPER poi (via il frame allocator) FRAMES. Non tenere nessuno
// dei due attraverso un await o un send/wait di messaggio cross-core.
```
In `frames.rs` sopra `static FRAMES` (riga 144):
```rust
// ORDINE LOCK (invariante SMP): chi serve sia MAPPER che FRAMES prende MAPPER PRIMA.
// FRAMES è preso da solo (allocate_frame/free_frame) o dopo MAPPER, mai prima.
```

- [ ] **Step 3: Scrivi il changelog 297**

Crea `CHANGELOG/297-26-06-05-smp-step7-baseline-globali.md` con sezioni
`# 297 — …`, `**Data:** 2026-06-05`, `## Cosa`, `## Perché`, `## File toccati`
(elenca i 3 file modificati + se stesso). Contenuto: Step 7 della migrazione SMP —
baseline documentale, commenti d'invariante (ordine lock, spinlock-già-SMP, contesa
non-safety). Nessun cambiamento funzionale.

- [ ] **Step 4: Verifica build default invariato**

Run (da WSL):
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot'
```
Expected: `TEST_BOOT_PASS` (i commenti non cambiano il comportamento; il boot deve
passare identico a prima).

- [ ] **Step 5: Commit**

```bash
git add kernel/src/memory/heap.rs kernel/src/memory/mapper.rs kernel/src/memory/frames.rs CHANGELOG/297-26-06-05-smp-step7-baseline-globali.md
git commit -m "docs(smp): step 7 baseline — invariant comments (lock order, spinlock-already-SMP)"
```

---

## Task 2: Strumento di benchmark single-core (boot-check)

Costruisci PRIMA lo strumento di misura, così i due prototipi sono misurati in modo
identico. Misura latenza TSC media di alloc+free per blocchi small (64 B) e large
(1 MiB), single-core (BSP), ed emette marker greppabili.

**Files:**
- Create: `kernel/src/memory/allocbench.rs`
- Modify: `kernel/src/memory/mod.rs` (re-export `allocbench` sotto `boot-checks`)
- Modify: `kernel/src/boot/phases/mem.rs` (chiama `run_single_core()` sotto `boot-checks`)
- Create: `CHANGELOG/298-26-06-05-smp-step1-allocbench.md`

- [ ] **Step 1: Scrivi il marker atteso come "test" (boot-check)**

Il "test" di questo kernel è un marker su seriale. Definisci il contratto: dopo
`init_heap`, il boot deve stampare una riga
`allocbench single small_ns=<N> large_ns=<N> iters=<N>`. La Task lo rende vero.

- [ ] **Step 2: Implementa `allocbench::run_single_core`**

Crea `kernel/src/memory/allocbench.rs`:

```rust
//! Allocator micro-benchmark (solo sotto `boot-checks`). Misura la latenza media
//! di alloc+free in cicli TSC, convertita in ns via TSC_PER_MS. Usato per
//! confrontare i prototipi di allocatore (default talc / magazine / per-core talc).

use alloc::boxed::Box;
use alloc::vec::Vec;
use crate::boot::clock::read_tsc;

/// Cicli TSC → nanosecondi. TSC_PER_MS è la calibrazione del clock di boot.
fn cyc_to_ns(cyc: u64, iters: u64) -> u64 {
    let per_ms = crate::boot::clock::tsc_per_ms().max(1);
    // ns = cyc / (per_ms / 1_000_000) / iters  →  cyc * 1_000_000 / per_ms / iters
    cyc.saturating_mul(1_000_000) / per_ms / iters.max(1)
}

const SMALL_ITERS: u64 = 100_000;
const LARGE_ITERS: u64 = 256;

/// Single-core alloc/free latency su BSP. Stampa il marker greppabile dal test.
pub fn run_single_core() {
    // Small: 64 byte Box, alloc+drop, accumulando l'indirizzo per impedire al
    // compilatore di ottimizzare via l'allocazione.
    let mut acc: u64 = 0;
    let t0 = read_tsc();
    for _ in 0..SMALL_ITERS {
        let b = Box::new(0xA5u64);
        acc = acc.wrapping_add(&*b as *const u64 as u64);
        core::hint::black_box(&b);
        drop(b);
    }
    let small_cyc = read_tsc().saturating_sub(t0);

    // Large: 1 MiB Vec, alloc+drop.
    let t1 = read_tsc();
    for _ in 0..LARGE_ITERS {
        let mut v: Vec<u8> = Vec::with_capacity(1024 * 1024);
        v.push((acc & 0xFF) as u8);
        core::hint::black_box(&v);
        drop(v);
    }
    let large_cyc = read_tsc().saturating_sub(t1);

    crate::binfo!(
        "allocbench",
        "single small_ns={} large_ns={} iters={} acc=0x{:X}",
        cyc_to_ns(small_cyc, SMALL_ITERS),
        cyc_to_ns(large_cyc, LARGE_ITERS),
        SMALL_ITERS, acc
    );
}
```

> Se `crate::boot::clock::tsc_per_ms()` non esiste con questo nome, verifica in
> `kernel/src/boot/clock.rs` il nome reale della costante di calibrazione
> (`TSC_PER_MS` citata nello spec §2.2) ed esponi un accessor `pub fn tsc_per_ms()`.

- [ ] **Step 3: Re-export e chiamata dal boot**

In `kernel/src/memory/mod.rs` aggiungi:
```rust
#[cfg(feature = "boot-checks")]
pub mod allocbench;
```
In `kernel/src/boot/phases/mem.rs`, dentro il blocco `#[cfg(feature = "boot-checks")]`
esistente subito dopo `init_heap` (righe 16-23), aggiungi alla fine del blocco:
```rust
crate::memory::allocbench::run_single_core();
```

- [ ] **Step 4: Build + run, verifica il marker**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot'
```
Expected: log contiene `allocbench single small_ns=` (un numero plausibile, es.
small_ns nell'ordine 10–100, large_ns più alto) e `TEST_BOOT_PASS`. Registra i numeri
del **default talc** come baseline.

- [ ] **Step 5: Changelog 298 + commit**

Crea `CHANGELOG/298-26-06-05-smp-step1-allocbench.md` (Cosa: strumento benchmark
allocatore single-core sotto boot-checks; Perché: misurare i prototipi in modo
identico). Poi:
```bash
git add kernel/src/memory/allocbench.rs kernel/src/memory/mod.rs kernel/src/boot/phases/mem.rs CHANGELOG/298-26-06-05-smp-step1-allocbench.md
git commit -m "feat(smp): single-core allocator micro-benchmark (boot-checks)"
```

---

## Task 3: Benchmark di contesa multi-core (boot-check, via smp::pool)

Gli AP oggi eseguono solo job `fn(&[u8])->u64` (puri per disciplina, ma POSSONO
allocare). Sfruttiamolo: un job che alloca+libera in loop, sottomesso a tutti gli AP,
misura la contesa sul lock allocatore. Va eseguito DOPO `smp::bringup()`.

**Files:**
- Modify: `kernel/src/memory/allocbench.rs` (aggiunge `run_multicore`)
- Modify: `kernel/src/boot/phases/interrupts.rs` (chiama dopo `bringup()`, sotto boot-checks)
- Create: `CHANGELOG/299-26-06-05-smp-step1-allocbench-mc.md`

- [ ] **Step 1: Contratto marker (test)**

Dopo `bringup`, il boot deve stampare
`allocbench multi cores=<N> total_ns=<N> per_job=<N>`. La Task lo rende vero.

- [ ] **Step 2: Implementa il job e `run_multicore`**

Aggiungi a `allocbench.rs`:

```rust
use core::sync::atomic::{AtomicU64, Ordering};

/// Contatore globale per dare lavoro osservabile (impedisce DCE) ai job.
static BENCH_SINK: AtomicU64 = AtomicU64::new(0);

/// Job di contesa: alloca+libera molti blocchi piccoli, ritorna un checksum.
/// `fn(&[u8])->u64` (firma JobFn del pool). `input` ignorato.
fn alloc_churn_job(_input: &[u8]) -> u64 {
    let mut acc: u64 = 0;
    for i in 0..50_000u64 {
        let b = Box::new(i);
        acc = acc.wrapping_add(&*b as *const u64 as u64);
        core::hint::black_box(&b);
        drop(b);
    }
    BENCH_SINK.fetch_add(acc, Ordering::Relaxed);
    acc
}

/// Sottomette N job di churn al pool (uno per core online), li attende, misura il
/// wall-time TSC e i core che li hanno eseguiti.
pub fn run_multicore() {
    static EMPTY_INPUT: [u8; 0] = [];
    let n = crate::cpu::cpus_online().max(1);
    // Sottometti fino a un job per core online (cap a MAX_JOBS del pool).
    let want = (n as usize).min(crate::smp::pool::MAX_JOBS);
    let mut ids: Vec<usize> = Vec::with_capacity(want);
    let t0 = read_tsc();
    for _ in 0..want {
        match crate::smp::pool::submit(alloc_churn_job, &EMPTY_INPUT) {
            Some(id) => ids.push(id),
            None => break,
        }
    }
    // Drena come fa il BSP fallback (cos', se nessun AP è libero, il BSP esegue),
    // poi raccogli i risultati.
    let mut cores_mask: u32 = 0;
    let mut done = 0usize;
    while done < ids.len() {
        // Aiuta come worker: esegui un job inline se disponibile (BSP fallback).
        if let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
        }
        for &id in &ids {
            if let Some((_r, cpu)) = crate::smp::pool::poll_done(id) {
                cores_mask |= 1u32 << (cpu & 31);
                done += 1;
            }
        }
    }
    let total_cyc = read_tsc().saturating_sub(t0);
    let cores = cores_mask.count_ones();
    crate::binfo!(
        "allocbench",
        "multi cores={} total_ns={} per_job={} jobs={} sink=0x{:X}",
        cores,
        cyc_to_ns(total_cyc, 1),
        cyc_to_ns(total_cyc, ids.len().max(1) as u64),
        ids.len(),
        BENCH_SINK.load(Ordering::Relaxed)
    );
}
```

> Nota: `poll_done` libera lo slot al primo `Some`; il doppio-conteggio è evitato
> perché ogni `id` diventa `Some` una sola volta (poi lo slot è `EMPTY`). Tenere il
> `for` su `ids` e incrementare `done` una volta per id.
> ATTENZIONE bug sottile: il `for &id in &ids` ri-pollerà id già completati (ora
> `EMPTY` → `None`), quindi `done` va incrementato solo sulla transizione. Usa invece
> un `Vec<bool> seen` parallelo a `ids` e conta solo i `!seen[k]` che diventano `Some`.
> Riscrivi il loop di raccolta con `seen` per evitare il doppio-decremento.

- [ ] **Step 3: Correggi il loop di raccolta con `seen`**

Sostituisci il blocco di raccolta con:
```rust
    let mut seen = alloc::vec![false; ids.len()];
    let mut cores_mask: u32 = 0;
    let mut done = 0usize;
    while done < ids.len() {
        if let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
        }
        for (k, &id) in ids.iter().enumerate() {
            if !seen[k] {
                if let Some((_r, cpu)) = crate::smp::pool::poll_done(id) {
                    seen[k] = true;
                    cores_mask |= 1u32 << (cpu & 31);
                    done += 1;
                }
            }
        }
    }
```

- [ ] **Step 4: Chiamata dal boot dopo bringup**

In `kernel/src/boot/phases/interrupts.rs`, DOPO `crate::smp::bringup()` (riga ~52),
aggiungi:
```rust
    #[cfg(feature = "boot-checks")]
    crate::memory::allocbench::run_multicore();
```

- [ ] **Step 5: Build + run multi-core**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot'
```
Expected: `allocbench multi cores=N total_ns=… per_job=…` con `cores >= 2` (il pool
usa gli AP — come il marker `composite cores=` esistente). Se `cores=1`, il
`-cpu max` di QEMU potrebbe esporre 1 core: per la contesa reale usa
`make run-smp-test` (vedi tests/smp-test.sh) o aggiungi `-smp 4` (vedi Task 6).
Registra i numeri del **default talc** come baseline di contesa.

- [ ] **Step 6: Changelog 299 + commit**

Crea `CHANGELOG/299-…-allocbench-mc.md`. Poi:
```bash
git add kernel/src/memory/allocbench.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/299-26-06-05-smp-step1-allocbench-mc.md
git commit -m "feat(smp): multi-core allocator contention benchmark (boot-checks, via smp::pool)"
```

---

## Task 4: Prototipo A — magazine per-core davanti a talc globale (feature `alloc-magazine`)

**Files:**
- Modify: `kernel/Cargo.toml` (feature `alloc-magazine`)
- Create: `kernel/src/memory/alloc_magazine.rs`
- Modify: `kernel/src/memory/heap.rs` (global_allocator cfg-gated)
- Modify: `kernel/src/memory/mod.rs` (re-export sotto feature)
- Create: `CHANGELOG/300-26-06-05-smp-step1-proto-magazine.md`

- [ ] **Step 1: Aggiungi la feature**

In `kernel/Cargo.toml`, sezione `[features]`:
```toml
alloc-magazine = []
alloc-percore-talc = []
```

- [ ] **Step 2: Implementa `MagazineAlloc`**

Crea `kernel/src/memory/alloc_magazine.rs`. Algoritmo: per `cpu_id()`, una cache di
free-list per size-class (potenze di 2 da 16 B a 2 KiB). Alloc: se size-class piccola e
la cache locale ha un nodo → pop (con IF disabilitato per evitare reentrancy ISR
sullo stesso core); altrimenti `inner_talc.alloc`. Free: se size-class piccola → push
sulla cache del core CHE LIBERA (qualsiasi core; il blocco appartiene al talc globale
che possiede tutto lo heap, quindi può essere riciclato da qualunque cache o restituito
al talc globale); overflow oltre `CACHE_DEPTH` → `inner_talc.dealloc`.

```rust
//! Prototipo A: magazine per-core davanti a UN talc globale (feature alloc-magazine).
//! THROWAWAY spike — confronto con alloc_percore_talc. NON di produzione.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use talc::{ErrOnOom, Span, Talc, Talck};
use crate::cpu::{cpu_id, MAX_CPUS};

const NUM_CLASSES: usize = 8;          // 16,32,64,128,256,512,1024,2048 B
const MAX_SMALL: usize = 2048;
const CACHE_DEPTH: usize = 64;          // nodi liberi per (core, classe)

#[inline]
fn size_class(layout: Layout) -> Option<usize> {
    let need = layout.size().max(layout.align());
    if need == 0 || need > MAX_SMALL { return None; }
    // più piccola potenza di 2 >= max(16, need)
    let mut sz = 16usize;
    let mut idx = 0usize;
    while sz < need { sz <<= 1; idx += 1; }
    Some(idx)
}

/// free-list intrusiva: il primo usize di ogni blocco libero è il "next".
struct Magazine {
    heads: [*mut u8; NUM_CLASSES],
    depth: [u16; NUM_CLASSES],
}
impl Magazine {
    const fn new() -> Self { Self { heads: [ptr::null_mut(); NUM_CLASSES], depth: [0; NUM_CLASSES] } }
}

struct PerCpuMag(core::cell::UnsafeCell<[Magazine; MAX_CPUS]>);
unsafe impl Sync for PerCpuMag {}   // partizionato per cpu_id, IF-mask sul push/pop

pub struct MagazineAlloc {
    inner: Talck<spin::Mutex<()>, ErrOnOom>,
    mags: PerCpuMag,
}

impl MagazineAlloc {
    pub const fn new() -> Self {
        const M: Magazine = Magazine::new();
        Self {
            inner: Talc::new(ErrOnOom).lock(),
            mags: PerCpuMag(core::cell::UnsafeCell::new([M; MAX_CPUS])),
        }
    }
    /// Claim dello span heap (chiamato da init_heap).
    pub unsafe fn claim(&self, base: *mut u8, size: usize) -> Result<(), ()> {
        self.inner.lock().claim(Span::from_base_size(base, size)).map(|_| ()).map_err(|_| ())
    }
}

unsafe impl GlobalAlloc for MagazineAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if let Some(cls) = size_class(layout) {
            let saved = x86_64::instructions::interrupts::are_enabled();
            x86_64::instructions::interrupts::disable();
            let mags = &mut (*self.mags.0.get())[cpu_id() as usize];
            let head = mags.heads[cls];
            if !head.is_null() {
                let next = *(head as *const *mut u8);
                mags.heads[cls] = next;
                mags.depth[cls] -= 1;
                if saved { x86_64::instructions::interrupts::enable(); }
                return head;
            }
            if saved { x86_64::instructions::interrupts::enable(); }
        }
        // miss o blocco grande → talc globale
        self.inner.alloc(layout)
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if let Some(cls) = size_class(layout) {
            let saved = x86_64::instructions::interrupts::are_enabled();
            x86_64::instructions::interrupts::disable();
            let mags = &mut (*self.mags.0.get())[cpu_id() as usize];
            if (mags.depth[cls] as usize) < CACHE_DEPTH {
                *(p as *mut *mut u8) = mags.heads[cls];   // next = vecchia testa
                mags.heads[cls] = p;
                mags.depth[cls] += 1;
                if saved { x86_64::instructions::interrupts::enable(); }
                return;
            }
            if saved { x86_64::instructions::interrupts::enable(); }
        }
        self.inner.dealloc(p, layout);
    }
}
```

> **Invariante di correttezza:** un blocco di classe `c` cachato è grande almeno
> `16<<c` byte → contiene il puntatore `next` (8 byte). I blocchi cachati NON tornano
> al talc finché non c'è overflow: va bene, è memoria dello heap globale, ricicabile da
> qualunque core (il talc possiede l'intero span). Free cross-core = push sulla cache
> del core che libera; nessuna remote-free queue.

- [ ] **Step 3: global_allocator cfg-gated in `heap.rs`**

Sostituisci la riga 18-19 (`#[global_allocator] pub static ALLOCATOR …`) con tre
alternative cfg-gated:

```rust
#[cfg(not(any(feature = "alloc-magazine", feature = "alloc-percore-talc")))]
#[global_allocator]
pub static ALLOCATOR: Talck<spin::Mutex<()>, ErrOnOom> = Talc::new(ErrOnOom).lock();

#[cfg(feature = "alloc-magazine")]
#[global_allocator]
pub static ALLOCATOR: crate::memory::alloc_magazine::MagazineAlloc =
    crate::memory::alloc_magazine::MagazineAlloc::new();
```
E adatta il `claim` in `init_heap` (riga 80-85) a essere cfg-aware:
```rust
    #[cfg(not(feature = "alloc-magazine"))]
    unsafe { ALLOCATOR.lock().claim(Span::from_base_size(virt_base as *mut u8, HEAP_SIZE)).map_err(|_| HeapInitError::ClaimFailed)?; }
    #[cfg(feature = "alloc-magazine")]
    unsafe { ALLOCATOR.claim(virt_base as *mut u8, HEAP_SIZE).map_err(|_| HeapInitError::ClaimFailed)?; }
```
(Il ramo `alloc-percore-talc` viene aggiunto nella Task 5.)

In `memory/mod.rs`:
```rust
#[cfg(feature = "alloc-magazine")]
pub mod alloc_magazine;
```

- [ ] **Step 4: Build con la feature + benchmark**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot CARGO_FEATURES="boot-checks alloc-magazine"'
```
Expected: `TEST_BOOT_PASS` + righe `allocbench single …` e `allocbench multi …` con la
magazine attiva. **Confronta small_ns/per_job vs il default talc** (Task 2/3 baseline):
ci si attende small_ns più basso (cache hit) e per_job multi-core più basso (meno
contesa). Registra i numeri.

- [ ] **Step 5: Changelog 300 + commit**

```bash
git add kernel/Cargo.toml kernel/src/memory/alloc_magazine.rs kernel/src/memory/heap.rs kernel/src/memory/mod.rs CHANGELOG/300-26-06-05-smp-step1-proto-magazine.md
git commit -m "feat(smp): prototype A — per-core magazine over global talc (alloc-magazine)"
```

---

## Task 5: Prototipo B — talc per-core + remote-free (feature `alloc-percore-talc`)

**Files:**
- Create: `kernel/src/memory/alloc_percore_talc.rs`
- Modify: `kernel/src/memory/heap.rs` (ramo global_allocator B + claim B)
- Modify: `kernel/src/memory/mod.rs` (re-export sotto feature)
- Create: `CHANGELOG/301-26-06-05-smp-step1-proto-percore-talc.md`

- [ ] **Step 1: Implementa `PerCoreTalc`**

Crea `kernel/src/memory/alloc_percore_talc.rs`. Algoritmo: dividi lo heap in
`MAX_CPUS` sub-span uguali + 1 span "fallback" globale per i blocchi grandi e per i
core con sub-arena esaurita. Alloc: `cpu_id()` → prova il talc per-core; su `null`
(arena locale piena) o blocco grande (≥ soglia) → talc fallback (lockato). Free:
determina l'owner per range (`ptr` in quale sub-span) → se è il core corrente, free
locale; se è un altro core, push sulla remote-free queue dell'owner; se è il fallback,
free fallback. Drena la propria remote-free queue all'inizio di ogni alloc.

```rust
//! Prototipo B: talc per-core su sub-span disgiunti + remote-free queue
//! (feature alloc-percore-talc). THROWAWAY spike. NON di produzione.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use talc::{ErrOnOom, Span, Talc, Talck};
use alloc::collections::VecDeque;
use crate::cpu::{cpu_id, MAX_CPUS};
use crate::sync::IrqMutex;

const LARGE_THRESHOLD: usize = 64 * 1024;   // ≥64 KiB → fallback globale

struct Arena {
    talc: Talck<spin::Mutex<()>, ErrOnOom>,
    base: usize,
    end: usize,     // [base, end)
}
impl Arena {
    const fn new() -> Self {
        Self { talc: Talc::new(ErrOnOom).lock(), base: 0, end: 0 }
    }
}

pub struct PerCoreTalc {
    arenas: UnsafeCell<[Arena; MAX_CPUS]>,
    fallback: Talck<spin::Mutex<()>, ErrOnOom>,
    remote: [IrqMutex<VecDeque<(usize, usize)>>; MAX_CPUS],  // (ptr, size) per owner
    fb_base: UnsafeCell<usize>,
    fb_end: UnsafeCell<usize>,
}
unsafe impl Sync for PerCoreTalc {}

impl PerCoreTalc {
    pub const fn new() -> Self {
        const A: Arena = Arena::new();
        const Q: IrqMutex<VecDeque<(usize, usize)>> = IrqMutex::new(VecDeque::new());
        Self {
            arenas: UnsafeCell::new([A; MAX_CPUS]),
            fallback: Talc::new(ErrOnOom).lock(),
            remote: [Q; MAX_CPUS],
            fb_base: UnsafeCell::new(0),
            fb_end: UnsafeCell::new(0),
        }
    }

    /// Dividi lo heap: metà ai sub-arena per-core (MAX_CPUS fette), metà al fallback
    /// (per le big alloc, es. gui.cwasm ~10 MiB, che NON entrano in una fetta).
    pub unsafe fn claim(&self, base: *mut u8, size: usize) -> Result<(), ()> {
        let per_core_total = size / 2;
        let slice = per_core_total / MAX_CPUS;
        let arenas = &mut *self.arenas.get();
        for (i, a) in arenas.iter_mut().enumerate() {
            let b = base as usize + i * slice;
            a.base = b;
            a.end = b + slice;
            a.talc.lock().claim(Span::from_base_size(b as *mut u8, slice)).map_err(|_| ())?;
        }
        let fb = base as usize + per_core_total;
        let fb_size = size - per_core_total;
        *self.fb_base.get() = fb;
        *self.fb_end.get() = fb + fb_size;
        self.fallback.lock().claim(Span::from_base_size(fb as *mut u8, fb_size)).map_err(|_| ())?;
        Ok(())
    }

    /// Quale arena possiede `ptr`? Some(i) per-core, None = fallback.
    unsafe fn owner_of(&self, ptr: usize) -> Option<usize> {
        let arenas = &*self.arenas.get();
        for (i, a) in arenas.iter().enumerate() {
            if ptr >= a.base && ptr < a.end { return Some(i); }
        }
        None
    }

    /// Drena i blocchi liberati da altri core che appartengono alla MIA arena.
    unsafe fn drain_remote(&self, me: usize) {
        while let Some((ptr, size)) = self.remote[me].lock().pop_front() {
            // size encode la Layout? Serve la Layout per dealloc. Vedi nota sotto.
            let layout = Layout::from_size_align_unchecked(size, 16);
            (*self.arenas.get())[me].talc.dealloc(ptr as *mut u8, layout);
        }
    }
}

unsafe impl GlobalAlloc for PerCoreTalc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let me = cpu_id() as usize;
        self.drain_remote(me);
        if layout.size() < LARGE_THRESHOLD {
            let p = (*self.arenas.get())[me].talc.alloc(layout);
            if !p.is_null() { return p; }
        }
        // grande, o arena locale piena → fallback
        self.fallback.alloc(layout)
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let addr = p as usize;
        match self.owner_of(addr) {
            Some(owner) => {
                let me = cpu_id() as usize;
                if owner == me {
                    (*self.arenas.get())[me].talc.dealloc(p, layout);
                } else {
                    // free cross-core → coda dell'owner (drenata dal suo alloc)
                    self.remote[owner].lock().push_back((addr, layout.size()));
                }
            }
            None => self.fallback.dealloc(p, layout),
        }
    }
}
```

> **Nota di correttezza (limite noto dello spike):** `talc::dealloc` richiede la
> `Layout` ESATTA usata in alloc. La remote-free queue salva solo `size`; per ricostruire
> la `Layout` lo spike assume `align=16` (vero per la maggior parte delle alloc Rust;
> alloc con align>16 attraverso i core sono rare). Se nel benchmark emergono fault,
> salva l'`align` reale nella coda (`(usize, usize, usize)`). Questo è ACCETTABILE in
> uno spike di misura; il valore dello spike è il dato di contesa, non la robustezza.

- [ ] **Step 2: rami global_allocator B + claim B in `heap.rs`**

Aggiungi:
```rust
#[cfg(feature = "alloc-percore-talc")]
#[global_allocator]
pub static ALLOCATOR: crate::memory::alloc_percore_talc::PerCoreTalc =
    crate::memory::alloc_percore_talc::PerCoreTalc::new();
```
E il ramo claim in `init_heap`:
```rust
    #[cfg(feature = "alloc-percore-talc")]
    unsafe { ALLOCATOR.claim(virt_base as *mut u8, HEAP_SIZE).map_err(|_| HeapInitError::ClaimFailed)?; }
```
In `memory/mod.rs`:
```rust
#[cfg(feature = "alloc-percore-talc")]
pub mod alloc_percore_talc;
```

- [ ] **Step 3: Build con la feature + benchmark**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot CARGO_FEATURES="boot-checks alloc-percore-talc"'
```
Expected: `TEST_BOOT_PASS` + righe `allocbench …`. Verifica in particolare che la **big
alloc** (large_ns) sia simile al default (va al fallback, come previsto) e che
small/multi mostrino il comportamento per-core. Registra i numeri. **Se il boot fa
fault** sulla big alloc GUI (`gui.cwasm` ~10 MiB), conferma che il fallback span
(64 MiB) è abbastanza grande; in caso, riduci la quota per-core (`size/4`) e aumenta il
fallback.

- [ ] **Step 4: Changelog 301 + commit**

```bash
git add kernel/src/memory/alloc_percore_talc.rs kernel/src/memory/heap.rs kernel/src/memory/mod.rs CHANGELOG/301-26-06-05-smp-step1-proto-percore-talc.md
git commit -m "feat(smp): prototype B — per-core talc + remote-free (alloc-percore-talc)"
```

---

## Task 6: Gate di decisione (misura comparata + record)

Esegui i tre build (default / A / B) con `-smp 4` reale, raccogli i marker, confronta
contro criteri espliciti, registra la decisione. Questo è un passo di DECISIONE, non
di codice.

**Files:**
- Create: `docs/superpowers/decisions/2026-06-05-allocator-architecture.md`
- Create: `CHANGELOG/302-26-06-05-smp-step1-allocator-decision.md`

- [ ] **Step 1: Esegui i tre build con SMP a 4 core**

Per ottenere contesa reale serve `-smp 4`. `make test-boot` usa `-cpu max` senza
`-smp`; aggiungi un override locale. Esegui (per ciascun set di feature):
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && \
  make iso CARGO_FEATURES="boot-checks" && \
  timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio \
    -device qemu-xhci -cdrom build/os.iso 2>&1 | tee build/bench-default.log | grep allocbench'
```
Ripeti con `CARGO_FEATURES="boot-checks alloc-magazine"` → `build/bench-magazine.log` e
`CARGO_FEATURES="boot-checks alloc-percore-talc"` → `build/bench-percore.log`.
Expected: ogni log ha `allocbench single …` e `allocbench multi cores=4 …`.

- [ ] **Step 2: Compila la tabella comparativa**

Estrai `small_ns`, `large_ns`, `multi per_job`, `multi cores` dai tre log. Tabella:

| Allocatore | small_ns | large_ns | multi per_job (4 core) | note |
|---|---|---|---|---|
| default talc | … | … | … | baseline |
| A magazine | … | … | … | |
| B percore-talc | … | … | … | |

- [ ] **Step 3: Applica i criteri di decisione**

Criteri (in ordine di peso):
1. **Correttezza:** il build boota fino a `TEST_BOOT_PASS`-equivalente (shell prompt) +
   GUI big alloc non fa fault. Un prototipo che non boota è squalificato.
2. **Contesa multi-core:** `multi per_job` più basso vince (è il bersaglio reale di
   Step 1: ridurre la contesa quando arriveranno gli executor per-core).
3. **Regressione single-core:** `small_ns`/`large_ns` NON peggiori del default oltre il
   ~10% (Step 1 non deve rallentare il caso comune).
4. **Semplicità/rischio:** a parità, vince quello con meno codice/`unsafe` e senza i
   limiti noti (B ha il problema Layout-su-remote-free; A no).

- [ ] **Step 4: Scrivi il record di decisione**

Crea `docs/superpowers/decisions/2026-06-05-allocator-architecture.md`: tabella numeri,
criteri, **decisione** (A o B o "serve iterare"), e le implicazioni sullo spec macro
(§6): se vince A → aggiorna §6 (magazine, niente remote-free queue); se vince B →
§6 confermato. Linka i tre log.

- [ ] **Step 5: Changelog 302 + commit**

```bash
git add docs/superpowers/decisions/2026-06-05-allocator-architecture.md CHANGELOG/302-26-06-05-smp-step1-allocator-decision.md
git commit -m "docs(smp): step 1 allocator architecture decision (data-driven)"
```

- [ ] **Step 6: (condizionale) aggiorna lo spec macro §6**

Se la decisione cambia §6 (es. vince A), modifica
`docs/superpowers/specs/2026-06-05-smp-shared-nothing-migration-design.md` §6 per
riflettere l'architettura scelta, e committa separatamente.

---

## Self-Review

**Spec coverage:** Lo spike copre l'obiettivo dell'utente ("prototipa entrambe +
misura"): Task 4 = prototipo A (magazine, l'alternativa scoperta in planning), Task 5 =
prototipo B (arena per-core + remote-free dello spec §6), Task 2/3 = strumento di
misura identico, Task 6 = decisione. Task 1 accorpa Step 7 (doc baseline). NON
implementa l'allocatore finale — intenzionale (dipende dai dati); è il piano successivo.

**Placeholder scan:** nessun "TBD/TODO". Le note "limite noto dello spike" (Layout su
remote-free; cpu_id cost) sono limiti deliberati di uno spike di misura, documentati,
non placeholder. I numeri di benchmark sono `…` perché sono OUTPUT da raccogliere, non
input da scrivere (Task 6 li riempie).

**Type consistency:** `MagazineAlloc::new()`/`claim`, `PerCoreTalc::new()`/`claim` usati
in heap.rs coerenti con le definizioni. `JobFn = fn(&[u8])->u64` coerente con
`alloc_churn_job`. `smp::pool::{submit, take, run_slot, poll_done, MAX_JOBS}` coerenti
con pool.rs. `cpu::{cpu_id, cpus_online, MAX_CPUS}` coerenti con cpu/mod.rs.
`binfo!`/`boot-checks` coerenti col pattern di mem.rs.

**Rischi residui da verificare in esecuzione:** (a) nome reale della costante di
calibrazione TSC (`tsc_per_ms` vs `TSC_PER_MS`) — Step 2.2 della Task 2 lo segnala;
(b) `-cpu max` senza `-smp` potrebbe dare 1 core → Task 6 usa `-smp 4`; (c) fault sulla
big alloc GUI nel prototipo B → Task 5.3 dà la mitigazione (ridurre quota per-core).
