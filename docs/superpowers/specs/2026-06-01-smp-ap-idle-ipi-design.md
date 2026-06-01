# SMP — AP idle via hlt + IPI-wake (Fase 2 fix): Design Spec

**Data:** 2026-06-01
**Branch:** `feature/smp-phase2-executor` (same branch — fixes a defect introduced by Fase 2 before merge)

## Contesto / problema

Fase 2 (compute offload pool) ha trasformato gli AP da `hlt` idle a un
`ap_worker_loop` che **busy-spinna al 100%**: ogni iterazione fa
`pool::take()` → `QUEUE.lock()` anche a coda vuota, senza mai cedere il core.
Su VBox/QEMU i 3-5 vCPU degli AP saturano la CPU host e competono col vCPU del
BSP → la **shell (che gira sul BSP) diventa lentissima**.

Era un compromesso accettato nello spec Fase 2 ("AP spin brucia CPU quando idle
→ idle vero con IPI = follow-up"). È il follow-up: gli AP dormono (`hlt`,
0% CPU) quando non c'è lavoro, e il BSP li sveglia con un IPI dopo `submit`.

### Vincolo: il timer non sveglia gli AP

Il LAPIC timer è armato solo sul BSP (`set_timer_periodic` in `timer::init`,
una volta). `timer_handler` fa lavoro BSP-specifico (`executor::delay::tick`,
`fb::tick_cursor`). Quindi un AP che fa `hlt` con STI NON riceve tick → senza
IPI resterebbe addormentato per sempre. **L'IPI è il meccanismo di wake.**

## Obiettivo

- AP idle = `hlt` (0% CPU), non busy-spin.
- BSP sveglia gli AP con un IPI broadcast ("all excluding self") dopo aver
  sottomesso un job; gli AP escono da `hlt`, prendono il lavoro, e ri-dormono.
- Shell di nuovo reattiva; `smptest` continua a dare speedup.
- Nessun #PF/#GP dagli AP sotto STI (verificato su VBox reale).

## Non-goal

- IPI per-target (per-lapic-id): basta il broadcast all-but-self. Chi ha lavoro
  lo prende, gli altri ri-dormono.
- Instradare timer/IRQ agli AP: gli AP ricevono SOLO il vettore wake via IPI.
- Scheduler / preemption (resta fuori; gli AP fanno solo pool jobs + hlt).

## Componenti

### 1. `kernel/src/apic/lapic.rs` — send IPI + AP LAPIC init

- `pub fn send_ipi_all_but_self(vector: u8)`: scrive l'ICR (Interrupt Command
  Register). ICR low = reg `0x300`, high = `0x310`. Per "all excluding self":
  destination shorthand bits [19:18] = `0b11`, delivery mode = Fixed (000),
  level = assert (bit 14 = 1), trigger = edge. Valore low =
  `(0b11 << 18) | (1 << 14) | (vector as u32)`. Scrivere prima `0x310` (high =
  0 per shorthand) poi `0x300` (low) — la scrittura su low triggera l'IPI.
  Riusa `reg()` + `write_volatile`.
- `pub fn init_ap()`: minimo LAPIC setup per un AP — abilita il LAPIC (SVR bit 8
  + spurious vector) e MASCHERA il timer LVT, SENZA ri-mappare LAPIC_VIRT (già
  fatto dal BSP). Necessario perché ogni core ha il proprio LAPIC; senza enable
  SVR, gli interrupt (incluso il wake IPI) potrebbero non essere consegnati.
  Estrarre la parte "enable SVR + mask timer" da `init()` in un helper condiviso
  o duplicare le 2 scritture. NON ricalibra il timer (BSP-only).

### 2. `kernel/src/idt.rs` — vettore wake

- `pub const VEC_WAKE: u8 = 0x40;`
- `wake_handler`: handler `extern "x86-interrupt"` che fa SOLO `lapic::eoi()` e
  ritorna (no-op — serve solo a far uscire l'AP da `hlt`). Registrato
  nell'IDT condiviso (`idt[VEC_WAKE].set_handler_fn(wake_handler)`), così ogni
  core (BSP + AP) lo ha quando carica l'IDT.

### 3. `kernel/src/cpu/ap.rs` — worker loop con hlt + STI

```rust
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize;
    crate::gdt::init(cpu_id);
    crate::idt::load();
    crate::apic::lapic::init_ap();      // enable this core's LAPIC, mask timer
    crate::cpu::mark_online();
    ap_worker_loop()
}

fn ap_worker_loop() -> ! {
    let me = crate::cpu::cpu_id();
    loop {
        // Run all available jobs.
        while let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, me);
        }
        // No work: sleep until a wake IPI. Anti-missed-wake: disable IRQs,
        // re-check the queue; if still empty, `sti; hlt` atomically (the wake
        // IPI cannot fire between sti and hlt — sti has a 1-instruction shadow).
        x86_64::instructions::interrupts::disable();
        if crate::smp::pool::is_empty() {
            x86_64::instructions::interrupts::enable_and_hlt();
        } else {
            x86_64::instructions::interrupts::enable();
        }
    }
}
```
Needs a `pool::is_empty()` (cheap queue-len check under the IrqMutex).
The `sti; hlt` pattern mirrors the BSP executor's `enable_and_hlt` to avoid the
missed-wake window.

### 4. `kernel/src/smp/pool.rs` — wake on submit

In `submit`, after pushing the id to the queue, send the wake IPI so any
sleeping AP wakes to take it:
```rust
    QUEUE.lock().push_back(id);
    crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_WAKE);
    return Some(id);
```
(Submit is called from the BSP — `send_ipi_all_but_self` wakes all APs; the one
that grabs the job runs it, the rest find the queue empty and re-`hlt`.)

## Data flow

AP idle → `hlt` (0% CPU). BSP `submit(job)` → push + `send_ipi_all_but_self(WAKE)`
→ APs exit `hlt` (wake_handler EOIs) → loop re-checks → `take()` → one runs the
job, others see empty → `sti; hlt`. Idle at rest = 0% CPU → shell responsive.

## Gestione errori

- IPI with no AP online (1 CPU) → ICR write is harmless (no targets).
- Spurious wake (IPI but queue drained) → `take()` None → re-`hlt`. Correct.
- Missed-wake race (job submitted between take→None and hlt) → the
  disable-IRQ + re-check-queue + `sti;hlt` pattern closes the window: if a job
  arrived, `is_empty()` is false → we `sti` and loop instead of sleeping.

## STI sugli AP — il rischio (VBox)

Gli AP ora abilitano gli interrupt. Rischi:
- LAPIC dell'AP non inizializzato → STI su uno stato strano. Mitigato da
  `init_ap()` (enable SVR + mask timer).
- Gli AP devono ricevere SOLO il wake: il timer LVT è masked (init_ap), la
  tastiera (IRQ1 via IOAPIC) è instradata al solo BSP. Nessun altro IPI.
- VBox ha già scoperto il quirk gs-base (Fase 0). **Verifica obbligatoria su
  VBox reale**: boot pulito (no #PF/#GP dagli AP), shell reattiva, smptest ok.

## Strategia di test

- **Interattività**: dopo il fix, la shell deve rispondere subito (no lag da
  AP busy-spin). Confronto qualitativo + i comandi (`ls`, `lscpu`) rispondono
  senza ritardo percepibile.
- `smptest` deve ancora mostrare speedup ≥1.5× su -smp 4 (gli AP si svegliano
  via IPI, prendono i job) — `make run-smp2-test` resta verde.
- run-test / ssh / pipe / fuel / smp tutti verdi.
- **VBox reale (6 vCPU)**: boot pulito, N/N APs online, NO #PF/#GP, smptest ok,
  shell reattiva. Questo è il gate (STI sugli AP è CPU-sensibile).

## Done criteria

- AP idle = `hlt`, 0% CPU a riposo (non busy-spin).
- BSP `submit` sveglia gli AP via IPI broadcast; i job vengono raccolti.
- Shell reattiva; nessun lag percepibile.
- `smptest` speedup ≥1.5× ancora valido; tutti i test verdi.
- VBox: boot pulito, no #PF/#GP dagli AP sotto STI, shell reattiva.

## Piano implementativo (sintesi — dettaglio in writing-plans)

1. `lapic.rs`: `send_ipi_all_but_self(vector)` + `init_ap()` (enable SVR + mask
   timer, no remap/recalibrate).
2. `idt.rs`: `VEC_WAKE` + `wake_handler` (EOI-only) registered in the shared IDT.
3. `pool.rs`: `is_empty()` + `send_ipi_all_but_self(VEC_WAKE)` in `submit`.
4. `cpu/ap.rs`: `init_ap()` in entry; worker loop `hlt`s when idle (anti-missed
   -wake `sti;hlt`), wakes on IPI.
5. Test: interattività + smptest speedup + full regression + VBox verify;
   CHANGELOG + (no roadmap change — Fase 2 already marked done).
