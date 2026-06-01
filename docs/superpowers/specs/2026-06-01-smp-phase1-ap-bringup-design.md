# SMP Fase 1 — AP bring-up → idle: Design Spec

**Data:** 2026-06-01
**Branch:** `feature/smp-phase1-ap-bringup`

## Contesto

Fase 0 (merged `a40a1f6`) ha reso ruos strutturalmente SMP-ready su 1 CPU:
per-CPU data scaffolding (GDT/TSS/IST per-core, `PER_CPU[MAX_CPUS=16]`),
enumerazione ACPI dei core (parcheggiati, NON avviati), audit lock (zero
must-fix). Verificato su QEMU + VirtualBox reale.

Fase 1 **avvia davvero** gli Application Processor enumerati e li parcheggia in
un idle loop. NIENTE scheduler, NIENTE lavoro sui core (Fase 2). L'obiettivo è
provare che il bring-up multi-core funziona, con il rischio confinato.

### Fatto chiave: Limine avvia gli AP per noi

Il crate `limine` 0.6.3 espone `MpRequest`/`MpResponse` (`src/mp/x86_64.rs`).
Limine porta ogni AP **già in long-mode** e lo consegna a una
`unsafe extern "C" fn(&MpInfo) -> !` via `MpInfo::bootstrap(fn, extra_arg)`.
**Niente trampoline asm 16→64bit fatto a mano** — il pezzo più rischioso
dell'AP bring-up classico è eliminato. API confermata contro il crate:

```rust
pub struct MpInfo { pub processor_id: u32, pub lapic_id: u32, /* ... */ }
impl MpInfo { pub fn bootstrap(&self, address: MpGotoFunction, extra_arg: u64); }
pub type MpGotoFunction = unsafe extern "C" fn(&MpInfo) -> !;
pub struct MpRespData { pub flags: u32, pub bsp_lapic_id: u32, /* cpu_count, cpus */ }
impl MpRespData { pub const fn cpus(&self) -> &[&MpInfo]; }
pub type MpRequest = Request<MpRespData, u64>;
```

### Vincolo VirtualBox (da Fase 0)

VBox accetta `wrmsr IA32_GS_BASE` nel MSR ma NON aggiorna la base nascosta del
segmento GS, quindi `mov gs:[0]` faulta. Fase 0 lo aggira facendo sì che
`this_cpu()` non tocchi mai `gs:[0]`. Fase 1 **non usa gs-base** per il core-id:
usa un lookup APIC-id → cpu_id che funziona su ogni VMM/HW.

## Obiettivo

- Avviare gli AP via Limine `MpRequest`; ogni AP raggiunge un PerCpu valido
  (GDT/TSS/IDT/cpu-id propri) e si registra "online".
- AP parcheggiati in `hlt` loop. NESSUN STI (no IRQ routing in Fase 1).
- BSP attende che tutti gli AP siano online (timeout), poi prosegue il boot
  normale fino alla shell.
- core-id VMM-independent via lookup LAPIC (no gs-base).
- Boot BSP invariato; run-test/ssh/pipe/fuel verdi; verificato su VBox.

## Non-goal (Fase 2, esplicitamente FUORI)

- Scheduler / executor SMP, qualsiasi lavoro utile sugli AP (restano idle).
- IRQ routing agli AP, per-CPU LAPIC timer attivo sugli AP.
- TLB shootdown via IPI, load balancing, work stealing.
- Migrazione dell'executor cooperativo a multi-core (resta single-core, solo il
  BSP chiama `run()`).

Gli AP in Fase 1 fanno SOLO: setup per-CPU minimo + `hlt` loop.

## Honest ceiling

Fase 1 NON dà ancora parallelismo utile: gli AP sono vivi ma idle. Prova che il
bring-up funziona e stabilisce il modello per-core runtime. Il workload
(.wasm/SSH/GUI) non è CPU-bound, quindi il beneficio reale arriva solo con Fase
2 (scheduler) E un carico parallelo CPU-bound. Fase 1 è il prerequisito.

## Componenti

### 1. `kernel/src/main.rs` — MpRequest

Aggiungere `pub static MP_REQUEST: MpRequest = MpRequest::new();` tra i limine
requests (dentro lo start/end marker). Nessun flag (x2APIC opzionale, lasciamo
xAPIC per coerenza con `lapic::apic_id()` che legge reg 0x20).

### 2. `kernel/src/cpu/mod.rs` — core-id via LAPIC + online tracking

- `static LAPIC_TO_CPU: [AtomicU8; 256]` — mappa `lapic_id → cpu_id`,
  inizializzata a un sentinel (es. 0xFF) e popolata a bring-up. (256 = spazio
  xAPIC ID a 8 bit.)
- `pub fn set_cpu_mapping(lapic_id: u32, cpu_id: u8)` — registra la mappa +
  popola `PER_CPU[cpu_id]` (cpu_id, lapic_id).
- `pub fn cpu_id() -> u32` — **cambia**: legge l'APIC ID via
  `crate::apic::lapic::apic_id()` (registro LAPIC, funziona su ogni core/VMM,
  NIENTE gs:[0]) e indicizza `LAPIC_TO_CPU`. Se sentinel (non mappato) →
  ritorna 0 (BSP fallback, sicuro). Questo è il path multi-CPU corretto.
- `pub fn this_cpu() -> &'static PerCpu` — **cambia**: `&PER_CPU[cpu_id() as
  usize]`. Niente gs-base. (init_bsp/gs_usable/this_cpu_via_gs restano per uso
  futuro ma non sono il path attivo.)
- `static CPUS_ONLINE: AtomicU32` + `pub fn mark_online()` (incrementa) +
  `pub fn cpus_online() -> u32`.

NOTA: `cpu_id()` letto via LAPIC ad ogni chiamata costa una lettura MMIO. È
accettabile in Fase 1 (uso raro); Fase 2 può cachare via gs-base sui core dove
funziona. Documentare il trade-off.

### 3. `kernel/src/cpu/ap.rs` (nuovo) — AP entry point

```rust
/// AP entry. Limine hands the AP here already in 64-bit long mode with a valid
/// (Limine-owned) stack. cpu_id is passed as the bootstrap extra_arg.
pub unsafe extern "C" fn ap_entry(info: &limine::mp::MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize; // the id we assigned
    // Per-core CPU structures.
    crate::gdt::init(cpu_id);          // load this core's GDT/TSS (slot cpu_id)
    crate::idt::load();                // load the shared IDT
    // PER_CPU[cpu_id] was filled by the BSP via set_cpu_mapping before bootstrap.
    crate::cpu::mark_online();
    // Fase 1: no work, no IRQs. Park.
    loop { x86_64::instructions::hlt(); }
}
```
NO `interrupts::enable()` (gli AP non ricevono IRQ in Fase 1). Lo stack è quello
fornito da Limine (valido per un idle loop). `AP_STACKS` per-core NON serve in
Fase 1 (verrà introdotto in Fase 2 quando gli AP eseguiranno lavoro reale).

### 4. `kernel/src/smp.rs` (nuovo) — bring-up coordinator

```rust
/// Start all enumerated APs and wait until they're online (with a timeout).
pub fn bringup() {
    let Some(resp) = crate::MP_REQUEST.get_response() else {
        crate::binfo!("smp", "no MP response — single CPU");
        return;
    };
    let bsp = resp.bsp_lapic_id;
    let mut next_id: u8 = 1; // BSP = 0, assigned in init_bsp
    let mut started = 0u32;
    for cpu in resp.cpus() {
        if cpu.lapic_id == bsp { continue; }              // skip BSP
        if (next_id as usize) >= crate::cpu::MAX_CPUS {     // cap
            crate::bwarn!("smp", "more CPUs than MAX_CPUS; rest parked");
            break;
        }
        let id = next_id; next_id += 1;
        crate::cpu::set_cpu_mapping(cpu.lapic_id, id);     // fill PER_CPU[id] + map
        // SAFETY: ap_entry is a valid MpGotoFunction.
        cpu.bootstrap(crate::cpu::ap::ap_entry, id as u64);
        started += 1;
    }
    // Wait for all started APs to report online (bounded spin).
    let mut spins = 0u64;
    while crate::cpu::cpus_online() < started && spins < 100_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }
    let online = crate::cpu::cpus_online();
    if online == started {
        crate::binfo!("smp", "{}/{} APs online", online, started);
    } else {
        crate::bwarn!("smp", "{}/{} APs online (timeout)", online, started);
    }
}
```
Also map the BSP into `LAPIC_TO_CPU` (id 0) at init so `cpu_id()` resolves on
the BSP too — done in `init_bsp`/`set_cpu_mapping(bsp_lapic, 0)`.

### 5. `kernel/src/idt.rs` — `load()` per gli AP

Split: `init()` (build via `Once` + `load`, BSP) resta; aggiungere
`pub fn load() { IDT.get().expect("idt not init").load(); }` che gli AP chiamano
per caricare l'IDT già costruito dal BSP (condiviso, read-only).

### 6. Boot wiring

In `boot/phases/`, dopo `interrupts::init` (che fa init_bsp + ACPI enum), una
nuova chiamata `crate::smp::bringup()` (nuova mini-fase o in coda a interrupts).
Mappare il BSP (id 0) PRIMA del bringup così `cpu_id()` sul BSP funziona.

## Gestione errori

- `MpResponse` assente → skip, "single CPU".
- AP non online entro il timeout → warning, boot continua (quell'AP resta giù;
  non blocca).
- `cpu_count > MAX_CPUS` → avvia i primi MAX_CPUS-1, log.
- `LAPIC_TO_CPU` sentinel su `cpu_id()` → 0 (BSP), sicuro.

## Strategia di test

- **QEMU `-smp 4`**: serial `smp: 3/3 APs online`, boot fino a shell.
- **QEMU `-smp 1`**: `single CPU` (o `0/0`), boot invariato.
- **VirtualBox (4 CPU)**: `smp: 3/3 APs online` + shell, ZERO #PF (core-id via
  LAPIC aggira gs-base). Verifica via VBoxManage headless + serial→file.
- run-test / run-ssh-test / run-pipe-test / run-fuel-test verdi.
- Nuovo target `make run-smp-test` (QEMU `-smp 4`, assert `APs online`).

## Done criteria

- Gli AP enumerati partono via Limine MpRequest e raggiungono `ap_entry`.
- Ogni AP carica GDT/TSS (slot proprio) + IDT condiviso, si registra online.
- BSP logga `N/N APs online`; boot prosegue normale fino alla shell.
- `cpu_id()`/`this_cpu()` risolvono via LAPIC (no gs:[0]); funziona su VBox.
- Tutti i test esistenti verdi + run-smp-test verde.
- AP in idle (`hlt`), nessun STI, nessun lavoro (Fase 2).
- Verificato su VirtualBox reale (no #PF).

## Piano implementativo (sintesi — dettaglio in writing-plans)

1. MpRequest in main.rs.
2. `idt::load()` per AP.
3. cpu/mod.rs: LAPIC_TO_CPU + cpu_id()/this_cpu() via LAPIC + online tracking +
   set_cpu_mapping; mappa BSP id 0.
4. cpu/ap.rs: ap_entry.
5. smp.rs: bringup() + boot wiring.
6. run-smp-test (QEMU -smp 4) + VBox verify + roadmap nota.
