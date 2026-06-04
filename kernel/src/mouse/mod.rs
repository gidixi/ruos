//! PS/2 mouse driver (IRQ12). Standard 3-byte packet protocol.
//!
//! Mirrors `crate::keyboard`: a pure `decode_packet` turns a raw 3-byte packet
//! into a `MouseEvent`; the ISR assembles packets and pushes events into an
//! IRQ-safe ring drained by higher layers (later: the GUI gfx service).

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use alloc::collections::VecDeque;
use x86_64::instructions::port::Port;
use x86_64::structures::idt::InterruptStackFrame;
use crate::apic;
use crate::acpi_init::IrqOverride;

/// One decoded mouse report. Movement is relative; Y is already flipped so
/// positive `dy` means "cursor moves down" (PS/2 reports up as positive).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MouseEvent {
    pub dx: i16,
    pub dy: i16,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

/// Decode a raw 3-byte PS/2 mouse packet.
///
/// byte0: bit0 L, bit1 R, bit2 M, bit3 always-1, bit4 X-sign, bit5 Y-sign,
///        bit6 X-overflow, bit7 Y-overflow.
/// byte1: X movement (9-bit two's complement with byte0 sign bit).
/// byte2: Y movement (likewise). Y is negated so down is positive.
pub fn decode_packet(b: [u8; 3]) -> MouseEvent {
    let flags = b[0];
    let left = flags & 0x01 != 0;
    let right = flags & 0x02 != 0;
    let middle = flags & 0x04 != 0;

    let dx = if flags & 0x10 != 0 {
        (b[1] as i16) - 0x100
    } else {
        b[1] as i16
    };
    let dy_raw = if flags & 0x20 != 0 {
        (b[2] as i16) - 0x100
    } else {
        b[2] as i16
    };

    MouseEvent { dx, dy: -dy_raw, left, right, middle }
}

// --- Event queue ----------------------------------------------------------

/// Bounded event queue. Oldest events are dropped when full so a fast-moving
/// mouse never blocks the ISR or grows memory without bound.
const QUEUE_CAP: usize = 256;
static QUEUE: crate::sync::IrqMutex<VecDeque<MouseEvent>> =
    crate::sync::IrqMutex::new(VecDeque::new());

/// Total events enqueued since boot (diagnostic; lets a smoke test confirm a
/// real IRQ12 round-trip happened).
static EVENT_COUNT: AtomicU32 = AtomicU32::new(0);

fn push_event(ev: MouseEvent) {
    let mut q = QUEUE.lock();
    if q.len() >= QUEUE_CAP {
        q.pop_front();
    }
    q.push_back(ev);
    EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Drain one event, if any. Called by higher layers (GUI gfx service).
pub fn pop_event() -> Option<MouseEvent> {
    QUEUE.lock().pop_front()
}

/// Number of mouse events seen since boot.
pub fn event_count() -> u32 {
    EVENT_COUNT.load(Ordering::Relaxed)
}

// --- ISR ------------------------------------------------------------------

/// Index of the next byte within the current 3-byte packet (0,1,2).
static PKT_IDX: AtomicU8 = AtomicU8::new(0);
/// The two already-received bytes packed as (byte0 << 8) | byte1.
static PKT_BUF: AtomicU32 = AtomicU32::new(0);

/// IRQ12 handler. Reads one byte from the PS/2 data port, assembles a 3-byte
/// packet, and on completion decodes + enqueues a `MouseEvent`.
pub extern "x86-interrupt" fn mouse_handler(_frame: InterruptStackFrame) {
    let mut data: Port<u8> = Port::new(0x60);
    // SAFETY: 0x60 is the PS/2 controller data port.
    let byte = unsafe { data.read() };

    match PKT_IDX.load(Ordering::SeqCst) {
        0 => {
            // Sync guard: byte0 must have bit3 set. If not, drop and resync.
            if byte & 0x08 == 0 {
                apic::lapic::eoi();
                return;
            }
            PKT_BUF.store((byte as u32) << 8, Ordering::SeqCst);
            PKT_IDX.store(1, Ordering::SeqCst);
        }
        1 => {
            let prev = PKT_BUF.load(Ordering::SeqCst);
            PKT_BUF.store(prev | (byte as u32), Ordering::SeqCst);
            PKT_IDX.store(2, Ordering::SeqCst);
        }
        _ => {
            let packed = PKT_BUF.load(Ordering::SeqCst);
            let b0 = (packed >> 8) as u8;
            let b1 = packed as u8;
            push_event(decode_packet([b0, b1, byte]));
            PKT_IDX.store(0, Ordering::SeqCst);
        }
    }

    apic::lapic::eoi();
}

// --- Controller init ------------------------------------------------------

const PS2_DATA: u16 = 0x60;
const PS2_CMD: u16 = 0x64;

/// Spin until the controller's input buffer is empty (safe to write).
fn wait_write() {
    let mut status: Port<u8> = Port::new(PS2_CMD);
    for _ in 0..100_000 {
        // SAFETY: reading the PS/2 status port has no side effects.
        if unsafe { status.read() } & 0x02 == 0 {
            return;
        }
    }
}

/// Spin until the controller's output buffer is full (data available to read).
fn wait_read() {
    let mut status: Port<u8> = Port::new(PS2_CMD);
    for _ in 0..100_000 {
        // SAFETY: reading the PS/2 status port has no side effects.
        if unsafe { status.read() } & 0x01 != 0 {
            return;
        }
    }
}

fn cmd(b: u8) {
    let mut p: Port<u8> = Port::new(PS2_CMD);
    wait_write();
    // SAFETY: 0x64 is the PS/2 command port.
    unsafe { p.write(b) };
}

fn write_data(b: u8) {
    let mut p: Port<u8> = Port::new(PS2_DATA);
    wait_write();
    // SAFETY: 0x60 is the PS/2 data port.
    unsafe { p.write(b) };
}

fn read_data() -> u8 {
    let mut p: Port<u8> = Port::new(PS2_DATA);
    wait_read();
    // SAFETY: 0x60 is the PS/2 data port.
    unsafe { p.read() }
}

/// Send a command to the mouse (auxiliary device) and return its ACK byte.
/// The 0xD4 prefix routes the next data byte to the aux device.
fn mouse_cmd(b: u8) -> u8 {
    cmd(0xD4);
    write_data(b);
    read_data()
}

/// Initialise the PS/2 mouse and wire IRQ12 through the IOAPIC.
pub fn init(overrides: &[IrqOverride]) {
    // 1. Enable the auxiliary (mouse) PS/2 device.
    cmd(0xA8);

    // 2. Read config byte, enable aux IRQ (bit1), enable aux clock (clear bit5).
    cmd(0x20);
    let mut config = read_data();
    config |= 0x02;
    config &= !0x20;
    cmd(0x60);
    write_data(config);

    // 3. Set defaults (0xF6) and 4. enable data reporting (0xF4). 0xFA = ACK.
    let ack_def = mouse_cmd(0xF6);
    let ack_en = mouse_cmd(0xF4);
    crate::binfo!(
        "mouse", "init defaults_ack=0x{:02X} enable_ack=0x{:02X}",
        ack_def, ack_en
    );

    // 5. Route IRQ12 → VEC_MOUSE (handles ACPI interrupt overrides).
    apic::ioapic::redirect(12, crate::idt::VEC_MOUSE, overrides);
}

/// Boot-check self-test: deterministic packets exercise sign-extension, Y-flip,
/// button bits, and the event queue. Returns true on success.
#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    if decode_packet([0x08, 0x00, 0x00])
        != (MouseEvent { dx: 0, dy: 0, left: false, right: false, middle: false })
    {
        return false;
    }
    if decode_packet([0x09, 0x05, 0x03])
        != (MouseEvent { dx: 5, dy: -3, left: true, right: false, middle: false })
    {
        return false;
    }
    // 0x3C = always-1(0x08) | middle(0x04) | X-sign(0x10) | Y-sign(0x20).
    if decode_packet([0x3C, 0xFE, 0xFF])
        != (MouseEvent { dx: -2, dy: 1, left: false, right: false, middle: true })
    {
        return false;
    }
    // Queue FIFO + drain.
    push_event(MouseEvent { dx: 1, dy: 2, left: true, right: false, middle: false });
    push_event(MouseEvent { dx: 3, dy: 4, left: false, right: true, middle: false });
    let a = pop_event();
    let b = pop_event();
    let c = pop_event();
    a == Some(MouseEvent { dx: 1, dy: 2, left: true, right: false, middle: false })
        && b == Some(MouseEvent { dx: 3, dy: 4, left: false, right: true, middle: false })
        && c.is_none()
}
