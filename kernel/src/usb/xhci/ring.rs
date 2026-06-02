//! Raw TRB helpers for the command (producer) and event (consumer) rings.
//! A TRB = 4 LE u32 words (16 bytes). Word3 bit0 = Cycle, bits10..=15 = Type.
use super::Xhci;

pub const TRB_BYTES: usize = 16;
pub const RING_TRBS: usize = 256;
pub const LINK_IDX: usize = RING_TRBS - 1; // command ring Link TRB slot

pub const TRB_LINK: u32 = 6;
pub const TRB_NOOP_CMD: u32 = 23;
pub const TRB_CMD_COMPLETION: u32 = 33;

/// Timeout for command completion polling (ms).
const CMD_TIMEOUT_MS: u64 = 50;

#[inline]
fn trb_ptr(virt: x86_64::VirtAddr, idx: usize) -> *mut u32 {
    (virt.as_u64() as usize + idx * TRB_BYTES) as *mut u32
}

/// Write a 4-word TRB at `idx` (volatile).
pub fn write_trb(virt: x86_64::VirtAddr, idx: usize, w: [u32; 4]) {
    let p = trb_ptr(virt, idx);
    unsafe {
        for i in 0..4 {
            core::ptr::write_volatile(p.add(i), w[i]);
        }
    }
}

/// Read a 4-word TRB at `idx` (volatile).
pub fn read_trb(virt: x86_64::VirtAddr, idx: usize) -> [u32; 4] {
    let p = trb_ptr(virt, idx);
    let mut w = [0u32; 4];
    unsafe {
        for i in 0..4 {
            w[i] = core::ptr::read_volatile(p.add(i));
        }
    }
    w
}

/// TRB type accessor (word3 bits 10..=15).
pub fn trb_type(w: &[u32; 4]) -> u32 {
    (w[3] >> 10) & 0x3F
}

/// Completion code of an event TRB (word2 bits 24..=31).
pub fn completion_code(w: &[u32; 4]) -> u32 {
    (w[2] >> 24) & 0xFF
}

/// Install a Link TRB at the last slot of any 256-entry transfer ring.
/// Used for EP0 and other transfer rings (not the command ring — use init_cmd_link
/// for that). `phys` is the physical base of the ring, `cycle` is the initial DCS.
pub fn init_link(virt: x86_64::VirtAddr, phys: u64, cycle: bool) {
    let w = [
        (phys & 0xFFFF_FFFF) as u32,
        (phys >> 32) as u32,
        0,
        (TRB_LINK << 10) | (1 << 1) | (cycle as u32),
    ];
    write_trb(virt, LINK_IDX, w);
}

/// Poll for a Command Completion event TRB (type 33) up to `CMD_TIMEOUT_MS`.
/// Returns Some(words) on success, None on timeout.
///
/// ASSUMPTION (MVP): boot enumeration is strictly serial — one command in
/// flight at a time and no concurrent transfers — so any non-CommandCompletion
/// event seen here (a stray Port Status Change, etc.) is discarded and its slot
/// consumed. Safe today because nothing else drives the rings during bring-up;
/// a future concurrent design must route unexpected events instead of dropping.
pub fn wait_cmd(x: &mut Xhci) -> Option<[u32; 4]> {
    let deadline = crate::boot::clock::elapsed_ms() + CMD_TIMEOUT_MS;
    loop {
        if let Some(ev) = poll_event(x) {
            if trb_type(&ev) == TRB_CMD_COMPLETION {
                return Some(ev);
            }
        }
        if crate::boot::clock::elapsed_ms() >= deadline {
            return None;
        }
        core::hint::spin_loop();
    }
}

/// Install the Link TRB at LINK_IDX on the command ring (points to ring start,
/// Toggle Cycle set). Call once during init/first use.
pub fn init_cmd_link(x: &Xhci) {
    let base = x.cmd_ring.phys.as_u64();
    // Link: word0=ptr_lo, word1=ptr_hi, word2=0,
    // word3 = (TRB_LINK<<10) | TC(bit1) | cycle
    let cyc = x.cmd_cycle as u32;
    let w = [
        (base & 0xFFFF_FFFF) as u32,
        (base >> 32) as u32,
        0,
        (TRB_LINK << 10) | (1 << 1) | cyc, // Toggle Cycle bit set
    ];
    write_trb(x.cmd_ring.virt, LINK_IDX, w);
}

/// Enqueue one transfer TRB onto an EP transfer ring (NOT the command ring).
/// Applies the current producer `*cycle` into word3 bit0, writes the TRB at
/// `*enqueue`, advances the pointer, and handles Link-TRB wrap at LINK_IDX:
/// rewrites the Link TRB with the current cycle, wraps to 0, and toggles
/// `*cycle`. Does NOT ring any doorbell — caller does that after pushing all TRBs.
///
/// `words` should have word3 set with all bits EXCEPT cycle (bit0), which this
/// function fills in from `*cycle`.
pub fn enqueue_xfer(
    ring: &crate::memory::dma::DmaRegion,
    enqueue: &mut usize,
    cycle: &mut bool,
    mut words: [u32; 4],
) {
    // Bake current cycle bit into word3.
    words[3] = (words[3] & !1) | (*cycle as u32);
    write_trb(ring.virt, *enqueue, words);
    *enqueue += 1;
    if *enqueue == LINK_IDX {
        // Rewrite Link TRB with current cycle, then wrap and toggle.
        let phys = ring.phys.as_u64();
        let link = [
            (phys & 0xFFFF_FFFF) as u32,
            (phys >> 32) as u32,
            0,
            (TRB_LINK << 10) | (1 << 1) | (*cycle as u32),
        ];
        write_trb(ring.virt, LINK_IDX, link);
        *enqueue = 0;
        *cycle = !*cycle;
    }
}

/// Enqueue a command TRB (words 0..2 caller-provided; type+cycle applied here),
/// then ring the command doorbell. Handles Link-TRB wrap at LINK_IDX.
///
/// Doorbell API used: `regs.doorbell.update_volatile_at(0, |d| { d.set_doorbell_target(0); })`
pub fn enqueue_cmd(x: &mut Xhci, mut w: [u32; 4], trb_type_val: u32) {
    w[3] = (w[3] & !((0x3F << 10) | 1)) | (trb_type_val << 10) | (x.cmd_cycle as u32);
    write_trb(x.cmd_ring.virt, x.cmd_enqueue, w);
    x.cmd_enqueue += 1;
    if x.cmd_enqueue == LINK_IDX {
        // Rewrite the Link TRB cycle to current producer cycle, then wrap+toggle.
        let base = x.cmd_ring.phys.as_u64();
        let link = [
            (base & 0xFFFF_FFFF) as u32,
            (base >> 32) as u32,
            0,
            (TRB_LINK << 10) | (1 << 1) | (x.cmd_cycle as u32),
        ];
        write_trb(x.cmd_ring.virt, LINK_IDX, link);
        x.cmd_enqueue = 0;
        x.cmd_cycle = !x.cmd_cycle;
    }
    // Ring command doorbell (doorbell 0, target 0).
    // `update_volatile_at` confirmed on accessor::array::Generic<T,M,ReadWrite>.
    x.regs.doorbell.update_volatile_at(0, |d| {
        d.set_doorbell_target(0);
    });
}

/// Poll one event TRB. Returns Some(words) if a new event is present (cycle
/// matches consumer cycle), advancing the dequeue + updating ERDP. None if empty.
///
/// ERDP API used:
///   `set_event_ring_dequeue_pointer(phys_u64)` — sets bits 4..63, zeroes bits 0..3.
///   `clear_event_handler_busy()` — rw1c: writes 1 to bit 3 to clear EHB.
/// Both are called in the same `update_volatile` closure so they compose correctly.
pub fn poll_event(x: &mut Xhci) -> Option<[u32; 4]> {
    let w = read_trb(x.event_ring.virt, x.event_dequeue);
    let cyc = (w[3] & 1) != 0;
    if cyc != x.event_cycle {
        return None;
    }
    // Advance dequeue.
    x.event_dequeue += 1;
    if x.event_dequeue == RING_TRBS {
        x.event_dequeue = 0;
        x.event_cycle = !x.event_cycle;
    }
    // Update ERDP to the new dequeue position phys, with EHB (bit3) cleared.
    // `set_event_ring_dequeue_pointer` asserts 16-byte alignment (TRB_BYTES=16, satisfied).
    // `clear_event_handler_busy` writes 1 to bit3 (rw1c) to clear EHB.
    let deq_phys =
        x.event_ring.phys.as_u64() + (x.event_dequeue as u64) * TRB_BYTES as u64;
    x.regs
        .interrupter_register_set
        .interrupter_mut(0)
        .erdp
        .update_volatile(|r| {
            r.set_event_ring_dequeue_pointer(deq_phys);
            r.clear_event_handler_busy();
        });
    Some(w)
}
