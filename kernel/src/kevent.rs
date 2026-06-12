//! Kernel event bus — ring broadcast pub/sub kernel→compositor.
//!
//! Spec: docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md
//!
//! Publish = scrivere uno slot del ring + incrementare il seq monotonico:
//! IRQ-safe (IrqMutex), zero alloc, mai bloccante. Ogni lettore tiene il
//! proprio cursore `last_seq` e rileva da solo i gap (eventi sovrascritti);
//! il bus non registra subscriber. In v1 l'unico lettore è il compositor.

use core::sync::atomic::{AtomicU64, Ordering};

pub const RING_LEN: usize = 64;

// Severity.
pub const SEV_INFO: u8 = 0;
pub const SEV_WARN: u8 = 1;
pub const SEV_CRIT: u8 = 2;

// Catalogo kind v1 — byte alto = categoria (0x00 meta-bus, 0x01 power,
// 0x02 app/risorse; 0x03 storage e 0x04 hotplug/net riservati fase 2).
// MAI riusare/ridefinire il payload di un kind esistente: kind nuovi = ID nuovi.
/// Sintetizzato LOCALMENTE dal lettore su gap (mai scritto nel ring).
pub const KIND_SUBSCRIBER_OVERFLOW: u16 = 0x0001;
/// Evento di prova (self-test boot-checks + builtin debug `kev-test`).
pub const KIND_TEST: u16 = 0x0002;
pub const KIND_SHUTDOWN_PENDING: u16 = 0x0101; // payload [countdown_sec, reason, 0, 0]
pub const KIND_REBOOT_PENDING: u16 = 0x0102;   // payload [countdown_sec, reason, 0, 0]
pub const KIND_POWER_CANCELLED: u16 = 0x0103;  // payload [0; 4]
pub const KIND_APP_CRASHED: u16 = 0x0201;      // payload [win_id, causa, 0, 0] + nome
pub const KIND_APP_FUEL_EXHAUSTED: u16 = 0x0202; // payload [pid, 0, 0, 0] + nome
pub const KIND_MEM_LOW: u16 = 0x0203;          // payload [frame_liberi, frame_totali, 0, 0]

// APP_CRASHED.causa
pub const CRASH_TRAP: u32 = 0;        // trap WASM (o proc_exit del guest)
pub const CRASH_WATCHDOG: u32 = 1;    // epoch watchdog deadline
pub const CRASH_SPAWN_FAILED: u32 = 2; // instantiate/_initialize falliti

/// Evento del bus. Struct fissa `repr(C)`, versionata implicitamente dal `kind`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KEvent {
    pub seq: u64,          // monotonico globale, parte da 1 (0 = slot vuoto)
    pub kind: u16,
    pub severity: u8,
    pub _pad: u8,
    pub ts_ticks: u32,     // tick timer 100 Hz al momento del publish
    pub payload: [u32; 4], // semantica per-kind (vedi catalogo)
}

impl KEvent {
    pub const ZERO: KEvent =
        KEvent { seq: 0, kind: 0, severity: 0, _pad: 0, ts_ticks: 0, payload: [0; 4] };
}

/// Ring + side-table nomi sotto UN lock (consistenza slot↔nome). I nomi app
/// non entrano nel payload fisso: copia troncata, stesso indice dello slot.
struct Bus {
    ring: [KEvent; RING_LEN],
    names: [heapless::String<32>; RING_LEN],
}

const EMPTY_NAME: heapless::String<32> = heapless::String::new();

static BUS: crate::sync::IrqMutex<Bus> = crate::sync::IrqMutex::new(Bus {
    ring: [KEvent::ZERO; RING_LEN],
    names: [EMPTY_NAME; RING_LEN],
});
/// Seq dell'ULTIMO evento pubblicato (0 = nessuno). Incrementato SOTTO il lock
/// BUS (l'ordine dei seq = l'ordine di scrittura degli slot); la load lock-free
/// serve solo a `current_seq()`.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Pubblica un evento. IRQ-safe, zero alloc, mai blocca (il critical section è
/// una copy di 32 byte). Slot di scrittura = `seq % RING_LEN` (circolare).
pub fn publish(kind: u16, severity: u8, payload: [u32; 4]) {
    publish_inner(kind, severity, payload, None);
}

/// Come `publish`, con nome associato (copiato TRONCATO a 32 byte nella
/// side-table — mai allocato).
pub fn publish_named(kind: u16, severity: u8, payload: [u32; 4], name: &str) {
    publish_inner(kind, severity, payload, Some(name));
}

fn publish_inner(kind: u16, severity: u8, payload: [u32; 4], name: Option<&str>) {
    let mut bus = BUS.lock();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1; // primo evento = seq 1
    let slot = (seq % RING_LEN as u64) as usize;
    bus.ring[slot] = KEvent {
        seq,
        kind,
        severity,
        _pad: 0,
        ts_ticks: crate::timer::ticks() as u32,
        payload,
    };
    bus.names[slot].clear();
    if let Some(n) = name {
        // Troncamento UTF-8-safe: push char-per-char finché c'è spazio.
        for ch in n.chars() {
            if bus.names[slot].push(ch).is_err() {
                break;
            }
        }
    }
}

/// Lettura da cursore: copia in `out` gli eventi con `seq > last_seq` in
/// ordine, ritorna `(n_copiati, lost)`. `lost > 0` se il ring ha sovrascritto
/// eventi mai letti (gap = seq_globale − last_seq − RING_LEN, se positivo);
/// in quel caso il lettore sintetizza localmente SUBSCRIBER_OVERFLOW{lost}.
/// Se gli eventi pendenti superano `out.len()` si richiama con il cursore
/// avanzato (l'ultimo `seq` copiato).
pub fn read_since(last_seq: u64, out: &mut [KEvent]) -> (usize, u64) {
    if out.is_empty() {
        return (0, 0);
    }
    let bus = BUS.lock();
    let cur = SEQ.load(Ordering::Relaxed);
    if cur <= last_seq {
        return (0, 0);
    }
    // Seq più vecchio ancora presente nel ring.
    let oldest = cur.saturating_sub(RING_LEN as u64 - 1).max(1);
    let lost = if last_seq + 1 < oldest { oldest - last_seq - 1 } else { 0 };
    let mut n = 0;
    let mut s = core::cmp::max(last_seq + 1, oldest);
    while s <= cur && n < out.len() {
        out[n] = bus.ring[(s % RING_LEN as u64) as usize];
        n += 1;
        s += 1;
    }
    (n, lost)
}

/// Nome associato all'evento `seq` (side-table). `None` se lo slot è stato
/// sovrascritto da un evento più recente o se l'evento non aveva nome.
pub fn name_of(seq: u64) -> Option<heapless::String<32>> {
    if seq == 0 {
        return None;
    }
    let bus = BUS.lock();
    let slot = (seq % RING_LEN as u64) as usize;
    if bus.ring[slot].seq != seq || bus.names[slot].is_empty() {
        return None;
    }
    Some(bus.names[slot].clone())
}

/// Seq corrente (ultimo pubblicato; 0 = nessun evento). Un lettore nuovo parte
/// da qui per NON rivedere il backlog (es. gli eventi del self-test in-boot).
pub fn current_seq() -> u64 {
    SEQ.load(Ordering::Relaxed)
}

/// Self-test in-boot (CARGO_FEATURES=boot-checks): pubblica RING_LEN+6 eventi,
/// verifica ordine seq e che `read_since` da cursore vecchio riporti lost == 6.
/// Stampa `KEVENT_TEST: OK` / `KEVENT_TEST: FAIL ...` (pattern engine_test).
#[cfg(feature = "boot-checks")]
pub fn self_test() {
    let base = current_seq();
    for i in 0..(RING_LEN as u32 + 6) {
        publish(KIND_TEST, SEV_INFO, [i, 0, 0, 0]);
    }
    let mut out = [KEvent::ZERO; RING_LEN];
    let (n, lost) = read_since(base, &mut out);
    let mut ok = n == RING_LEN && lost == 6 && out[0].seq == base + 7;
    for w in 0..n.saturating_sub(1) {
        if out[w + 1].seq != out[w].seq + 1 {
            ok = false;
        }
    }
    if ok {
        crate::kprintln!("KEVENT_TEST: OK");
    } else {
        crate::kprintln!(
            "KEVENT_TEST: FAIL n={} lost={} first_seq={} base={}",
            n, lost, out[0].seq, base
        );
    }
}
