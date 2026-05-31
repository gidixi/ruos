# SMP Fase 0 — Fondamenta per-CPU (su 1 CPU, zero AP): Design Spec

**Data:** 2026-05-31
**Branch:** `feature/smp-phase0-percpu`

## Contesto

ruos gira tutto in ring 0 su **una sola CPU**, con concorrenza **cooperativa**
(executor async embassy, niente thread preemptive, niente SMP — drop espliciti
del pivot 2026). CLAUDE.md: *"Niente preemptive thread scheduler. Concurrency =
async cooperative, single-CPU. SMP dopo, se serve."*

Il kernel ha ~106 assunzioni single-CPU oggi corrette che diventerebbero bug
silenziosi sotto SMP: 42 `without_interrupts`, 64 `spin::Mutex`, 43 `static
mut`, un executor `unsafe impl Sync` documentato *"single-CPU; no concurrent
access is possible"*, GDT/TSS/IST singoli, nessun supporto MSR/gs-base, ACPI
parsato ma CPU non enumerate.

SMP completo è multi-mese ad alto rischio (race silenziose) e in tensione col
pivot. Questa è **Fase 0**: rendere il kernel *strutturalmente* SMP-ready
SENZA avviare gli Application Processor. Tutto si esercita e testa su 1 CPU
(solo lo slot BSP attivo). È il prerequisito obbligato; le decisioni "accendere
gli AP" (Fase 1) e "executor SMP/scheduler" (Fase 2) restano separate e
informate.

## Obiettivo

- Infrastruttura **per-CPU data** via GS-base MSR (idiomatico x86-64).
- **GDT/TSS/IST/stack per-core** (array dimensionato `MAX_CPUS=16`), slot 0 = BSP.
- **Enumerare le CPU** da ACPI (informativo; nessun AP avviato).
- **Audit + fix dei lock**: classificare ogni `without_interrupts`/`Mutex`/
  `static mut` in già-SMP-safe vs da-fixare; introdurre un `IrqMutex<T>`
  riusabile e sostituire i siti pericolosi.
- Boot + `make run-test` + tutti i test esistenti (ssh/pipe/fuel) restano verdi.

## Non-goal (Fase 1+, esplicitamente FUORI)

- **AP bring-up** (sequenza INIT-SIPI-SIPI via LAPIC). Fase 0 enumera ma non avvia.
- **Per-CPU LAPIC timer** su ogni core.
- **Executor SMP / scheduler** — il modello concorrenza resta cooperativo
  single-CPU. L'executor `unsafe impl Sync` resta intatto, solo meglio
  documentato. Sostituirlo = Fase 2, decisione che riscrive il modello del pivot.
- **TLB shootdown via IPI**, per-CPU run-queue, work stealing.

Fase 0 ABILITA tutto questo (fondamenta) ma non lo implementa. Niente codice di
boot AP in questo branch.

## Honest ceiling

Fase 0 NON dà parallelismo: il kernel continua a girare su 1 core. Rende solo le
fondamenta corrette e gli accessi ai dati condivisi SMP-safe, così che accendere
gli AP (Fase 1) non parta da un kernel pieno di assunzioni single-CPU. Il
beneficio immediato è **rigore** (l'audit lock migliora la correttezza anche su
1 CPU); il parallelismo arriva solo con le fasi successive.

## Componenti

### 1. `kernel/src/cpu/mod.rs` (nuovo) — per-CPU core + MSR

```rust
pub const MAX_CPUS: usize = 16;

#[repr(C)]
pub struct PerCpu {
    /// self-pointer at offset 0 so `gs:[0]` yields &PerCpu (Linux pattern).
    pub self_ptr: *const PerCpu,
    pub cpu_id: u32,          // dense index 0..N (BSP = 0)
    pub lapic_id: u32,        // hardware APIC ID (may be sparse)
    pub kernel_stack_top: u64,
    // future: cur_task, preempt_count, etc. (Fase 2)
}

static mut PER_CPU: [PerCpu; MAX_CPUS] = /* zeroed init */;
```

- **MSR helpers** (nuovi — non esistono oggi): `rdmsr(msr: u32) -> u64`,
  `wrmsr(msr: u32, val: u64)` via `rdmsr`/`wrmsr` inline asm (or
  `x86_64::registers::model_specific::Msr` if it exposes IA32_GS_BASE = 0xC000_0101).
- **`init_bsp()`**: legge l'APIC ID (LAPIC reg 0x20 >> 24), imposta
  `PER_CPU[0]` (self_ptr, cpu_id=0, lapic_id, kernel_stack_top), `wrmsr(GS_BASE,
  &PER_CPU[0])`. Chiamato a boot dopo LAPIC init.
- **`this_cpu() -> &'static PerCpu`**: legge `gs:[0]` (il self_ptr) → `&PerCpu`.
  Safe perché ogni core punta al proprio blocco. (Fase 1: ogni AP chiamerà
  l'equivalente `init_ap(n)`.)
- **`cpu_id() -> u32`**: `this_cpu().cpu_id`.

### 2. `kernel/src/gdt.rs` — per-CPU GDT/TSS/IST

Oggi: `static mut TSS`, `static GDT: Once<…>`, `static mut DOUBLE_FAULT_STACK`
singoli. Diventano array `[…; MAX_CPUS]`:

```rust
static mut TSS:  [TaskStateSegment; MAX_CPUS] = [TaskStateSegment::new(); MAX_CPUS];
static mut DOUBLE_FAULT_STACK: [[u8; STACK_SIZE]; MAX_CPUS] = ...;
// GDT per-core (each loads its own TSS descriptor)
```

- `gdt::init(cpu_id: usize)`: costruisce GDT[cpu_id] con TSS[cpu_id] descriptor,
  IST0 = DOUBLE_FAULT_STACK[cpu_id], `lgdt` + carica TR. BSP chiama `init(0)`.
- Conserva la logica IST esistente (IST0 per #DF). Solo moltiplicata per-core.

### 3. `kernel/src/acpi_init.rs` — enumerare le CPU

- Estrarre `processor_info` da `PlatformInfo` del crate `acpi` (già usato per
  l'InterruptModel). Per ogni processore: `(processor_uid, local_apic_id,
  is_application_processor, state)`.
- Popolare `PER_CPU[i].lapic_id` per i core trovati (BSP a slot 0). Contare le
  CPU → log `acpi: N CPU(s) found (1 active, N-1 parked)`.
- ACPI senza processor_info → assumere 1 CPU (BSP), warning, non fatale.
- HW con > MAX_CPUS → log + ignorare gli extra (non avviati comunque).

### 4. Lock audit + `IrqMutex<T>` — il cuore

**Audit (documentato in un file `docs/...` o nel CHANGELOG):** classificare
ogni sito in:
- **(a) già SMP-safe**: `without_interrupts(|| SOME_MUTEX.lock()…)` — lo
  `spin::Mutex` protegge cross-core; il WI evita solo il deadlock IRQ-vs-task
  sullo stesso core. Restano com'è; documentati come safe.
- **(b) da fixare**: `static mut` toccato senza lock, o stato condiviso protetto
  SOLO da `without_interrupts` (IF-based — non protegge cross-core).

**`IrqMutex<T>` (nuovo, in `kernel/src/sync/mod.rs` o `cpu/`):** un wrapper =
`spin::Mutex<T>` che su `lock()` salva+disabilita IF e su `Drop` lo ripristina.
Un solo primitive invece del pattern `without_interrupts(|| m.lock())` ripetuto.
Sostituire i siti (b) con `IrqMutex`. NON riscrivere in massa i siti (a)
già-safe (YAGNI — funzionano); opzionalmente migrarli a `IrqMutex` solo se
chiarisce.

**Executor:** `unsafe impl Sync for ExecCell` resta. Aggiornare il commento per
legare esplicitamente la sicurezza all'invariante *"esattamente 1 core chiama
`run()`; Fase 2 rivede questo per SMP"*. Nessun cambiamento funzionale.

## Gestione errori

- ACPI processor info assente → 1 CPU (BSP). Non fatale.
- APIC ID non leggibile → assumere 0. Log warning.
- `> MAX_CPUS` core → ignorare gli extra.
- `wrmsr` GS_BASE deve avvenire PRIMA di ogni uso di `this_cpu()` — l'ordine di
  boot lo garantisce (init_bsp early). Se `this_cpu()` chiamato prima del setup
  → UB; mitigare con un flag debug `GS_READY` controllato in debug build.

## Strategia di test

Nessun test multi-core (non ci sono AP — è il punto). Verifica su 1 CPU:
- `make run-test` → TEST_PASS (boot invariato, slot BSP).
- `make run-ssh-test`, `make run-pipe-test`, `make run-fuel-test` → tutti verdi
  (l'audit lock non deve regredire nulla).
- Smoke nuovo a boot (serial): `cpu0 apic_id=N gs_base=0x…`, `acpi: N CPU(s)
  found`, e un'asserzione `this_cpu().cpu_id == 0` + `this_cpu().self_ptr` valido.
- Se i `boot-checks` esistono (feature), aggiungere un check per-CPU lì.

## Done criteria

- `cpu::this_cpu()` funziona sul BSP (gs-base impostato), ritorna cpu_id=0.
- GDT/TSS/IST per-CPU: BSP carica slot 0; #DF usa ancora l'IST0 del BSP
  (verificabile: il test #DF esistente, se c'è, passa ancora).
- ACPI enumera ≥1 CPU e logga il conteggio.
- Audit completo: ogni `without_interrupts`/`Mutex`/`static mut` classificato;
  i siti (b) pericolosi convertiti a `IrqMutex` o veri lock.
- Tutti i test esistenti verdi. Boot invariato a 1 CPU.
- Nessun codice di AP bring-up (resta Fase 1).

## Piano implementativo (sintesi — dettaglio in writing-plans)

1. MSR helpers (`rdmsr`/`wrmsr`) + smoke.
2. `cpu/mod.rs`: PerCpu + PER_CPU array + init_bsp + this_cpu (gs-base).
3. GDT/TSS/IST → per-CPU array; `gdt::init(cpu_id)`; BSP usa slot 0.
4. ACPI: estrarre processor_info, popolare lapic_id, contare CPU.
5. `IrqMutex<T>` + audit: classificare i siti, convertire i (b) pericolosi.
6. Boot smoke (cpu0 log + assert) + CHANGELOG + roadmap nota (SMP Fase 0 done).
