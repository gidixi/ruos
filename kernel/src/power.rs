//! Power management: reboot and poweroff.
//!
//! Reboot: pulse the keyboard controller (port 0x64, command 0xFE).
//! Universally supported on x86 — qemu, vbox, real hardware. Falls
//! through to triple-fault if the controller doesn't respond.
//!
//! Poweroff: try the well-known I/O debug-exit ports in sequence —
//!   QEMU isa-debug-exit at 0x604
//!   VirtualBox at 0x4004
//!   QEMU q35 ACPI shutdown at 0xB004
//! If none respond, halt forever. ACPI S5 sleep (proper poweroff via
//! FADT + DSDT _S5 SLP_TYPa) deferred — would need AML parser.

use x86_64::instructions::port::Port;
use x86_64::instructions::interrupts;

use crate::sync::IrqMutex;

/// Countdown di default per le richieste differite da host fn GUI (spec
/// kernel-event-bus v1).
pub const DEFAULT_COUNTDOWN_SEC: u32 = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PendingKind { Poweroff, Reboot }

#[derive(Clone, Copy)]
struct Pending {
    kind: PendingKind,
    deadline_tick: u64,
}

static PENDING: IrqMutex<Option<Pending>> = IrqMutex::new(None);

/// Richiede uno spegnimento differito annullabile: pubblica SHUTDOWN_PENDING
/// e spawna il task di enforcement. Richiesta duplicata mentre un PENDING è
/// attivo = no-op. Ritorna subito (NON è mai divergente).
pub fn request_poweroff(countdown_sec: u32) {
    request(PendingKind::Poweroff, countdown_sec);
}

/// Twin di `request_poweroff` per il riavvio (REBOOT_PENDING).
pub fn request_reboot(countdown_sec: u32) {
    request(PendingKind::Reboot, countdown_sec);
}

fn request(kind: PendingKind, countdown_sec: u32) {
    let deadline = crate::timer::ticks() + countdown_sec as u64 * 100;
    {
        let mut p = PENDING.lock();
        if p.is_some() {
            return; // già pendente: no-op
        }
        *p = Some(Pending { kind, deadline_tick: deadline });
    }
    let ev = match kind {
        PendingKind::Poweroff => crate::kevent::KIND_SHUTDOWN_PENDING,
        PendingKind::Reboot => crate::kevent::KIND_REBOOT_PENDING,
    };
    // reason 0 = richiesta utente (unico in v1).
    crate::kevent::publish(ev, crate::kevent::SEV_CRIT, [countdown_sec, 0, 0, 0]);
    crate::binfo!("power", "{:?} pending in {}s", kind, countdown_sec);
    // L'ENFORCEMENT è il task, non la UI: lo spegnimento avviene anche
    // headless o con compositor morto. Spawn sul BSP (core 0).
    if crate::executor::spawn_on(0, power_enforce_task(deadline)).is_err() {
        // Pool esaurito (2 cancel+re-request nello stesso countdown): rifiuta
        // la richiesta — la macchina resta accesa, l'utente ritenta.
        *PENDING.lock() = None;
        crate::kevent::publish(crate::kevent::KIND_POWER_CANCELLED,
                               crate::kevent::SEV_INFO, [0; 4]);
        crate::bwarn!("power", "enforce task pool full: request dropped");
    }
}

/// Annulla la richiesta pendente (se c'è) e pubblica POWER_CANCELLED. Il task
/// di enforcement in volo troverà PENDING == None e terminerà senza spegnere.
pub fn cancel() {
    let was = PENDING.lock().take();
    if was.is_some() {
        crate::kevent::publish(crate::kevent::KIND_POWER_CANCELLED,
                               crate::kevent::SEV_INFO, [0; 4]);
        crate::binfo!("power", "pending shutdown/reboot cancelled");
    }
}

/// Richiesta pendente: `(kind, tick rimanenti)`. Fonte di verità per il
/// countdown del modale (il compositor NON conta da solo).
pub fn pending() -> Option<(PendingKind, u64)> {
    let p = (*PENDING.lock())?;
    Some((p.kind, p.deadline_tick.saturating_sub(crate::timer::ticks())))
}

/// Task di enforcement: dorme fino alla deadline; al risveglio spegne SOLO se
/// PENDING è ancora attivo E la deadline è la SUA (un cancel + nuova richiesta
/// = un altro task con un'altra deadline). pool_size 2 copre il caso
/// cancel→re-request mentre il task vecchio sta ancora dormendo.
#[embassy_executor::task(pool_size = 2)]
async fn power_enforce_task(deadline: u64) {
    loop {
        let now = crate::timer::ticks();
        if now >= deadline {
            break;
        }
        crate::executor::delay::Delay::ticks(deadline - now).await;
    }
    let p = *PENDING.lock();
    if let Some(p) = p {
        if p.deadline_tick == deadline {
            match p.kind {
                PendingKind::Poweroff => poweroff(),
                PendingKind::Reboot => reboot(),
            }
        }
    }
    // PENDING annullato (o sostituito): termina senza spegnere.
}

/// Reboot the system. Never returns.
pub fn reboot() -> ! {
    interrupts::disable();
    let mut cmd: Port<u8> = Port::new(0x64);
    // Wait for keyboard input buffer to drain, then issue reset cmd 0xFE.
    for _ in 0..1024 {
        unsafe {
            if cmd.read() & 0x02 == 0 {
                cmd.write(0xFE);
            }
        }
        for _ in 0..10_000 { core::hint::spin_loop(); }
    }
    // Keyboard controller didn't reset — triple-fault by loading null IDT.
    unsafe {
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: x86_64::VirtAddr::new(0),
        };
        x86_64::instructions::tables::lidt(&null_idt);
        core::arch::asm!("int3");
    }
    loop { x86_64::instructions::hlt(); }
}

/// Power off the system. Never returns.
pub fn poweroff() -> ! {
    interrupts::disable();
    unsafe {
        // QEMU isa-debug-exit (works if -device isa-debug-exit set).
        let mut p604: Port<u16> = Port::new(0x604);
        p604.write(0x2000);
        // VirtualBox.
        let mut p4004: Port<u16> = Port::new(0x4004);
        p4004.write(0x3400);
        // QEMU q35 ACPI shutdown.
        let mut pb004: Port<u16> = Port::new(0xB004);
        pb004.write(0x2000);
    }
    // Nothing worked — halt.
    loop { x86_64::instructions::hlt(); }
}
