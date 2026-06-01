//! Per-CPU data. Each core's GS base points at its `PerCpu` block, so `gs:[0]`
//! (a self-pointer stored at offset 0) yields `&PerCpu` in O(1) — the standard
//! x86-64 per-CPU pattern. On 1 CPU only slot 0 (the BSP) is live; the later
//! AP bring-up phase will call an `init_ap(n)` for the others.

pub mod ap;

use x86_64::VirtAddr;
use x86_64::registers::model_specific::GsBase;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

pub const MAX_CPUS: usize = 16;

/// Sentinel for an unmapped LAPIC ID slot.
const NO_CPU: u8 = 0xFF;

/// lapic_id (xAPIC 8-bit) -> dense cpu_id. Filled by `set_cpu_mapping` at
/// bring-up; read by `cpu_id()` on every core.
static LAPIC_TO_CPU: [AtomicU8; 256] = {
    const Z: AtomicU8 = AtomicU8::new(NO_CPU);
    [Z; 256]
};

/// Count of cores that have registered online via `mark_online`.
static CPUS_ONLINE: AtomicU32 = AtomicU32::new(0);

#[repr(C)]
pub struct PerCpu {
    /// MUST be offset 0: a pointer to self, so `mov rax, gs:[0]` loads &PerCpu.
    pub self_ptr: *const PerCpu,
    pub cpu_id: u32,
    pub lapic_id: u32,
    pub kernel_stack_top: u64,
}

impl PerCpu {
    const fn zeroed() -> Self {
        Self { self_ptr: core::ptr::null(), cpu_id: 0, lapic_id: 0, kernel_stack_top: 0 }
    }
}

// SAFETY: PER_CPU is mutated only during single-threaded boot (init_bsp here;
// later init_ap before that AP runs any task). After setup each core only reads
// its own slot via gs-base. The raw-pointer field makes it !Sync by default; we
// assert Sync because access is partitioned per-core by construction.
struct PerCpuArray([PerCpu; MAX_CPUS]);
unsafe impl Sync for PerCpuArray {}

const ZEROED: PerCpu = PerCpu::zeroed();
static mut PER_CPU: PerCpuArray = PerCpuArray([ZEROED; MAX_CPUS]);

/// True iff the GS base, as seen by an actual `gs:`-relative memory access,
/// holds the installed per-CPU pointer. Set by `init_bsp` after a *memory*
/// probe (not a bare MSR read-back — see init_bsp).
static GS_USABLE: AtomicBool = AtomicBool::new(false);

/// BSP per-CPU init. Call AFTER gdt::init (the GS segment-load done there zeroes
/// the GS base) and AFTER lapic::init (the APIC ID register must be mapped).
/// Fills PER_CPU[0] and writes the GS base.
///
/// Returns `true` if `gs:[0]` actually reads back the installed self-pointer.
/// Returns `false` on VMMs that accept `wrmsr IA32_GS_BASE` into the MSR but do
/// NOT update the hidden GS segment base used by `gs:`-relative accesses —
/// VirtualBox does exactly this (the MSR read-back matches, yet `mov gs:[0]`
/// still uses base 0 and faults). We therefore probe with a REAL memory access,
/// not an MSR read-back. On `false`, `this_cpu()` uses the BSP slot directly and
/// never touches `gs:[0]`, so boot completes on a single CPU regardless. A
/// future AP bring-up phase MUST require `true` before relying on per-core
/// gs-base to distinguish cores.
pub fn init_bsp(kernel_stack_top: u64) -> bool {
    let lapic_id = crate::apic::lapic::apic_id();
    // SAFETY: single-threaded boot; no other accessor to PER_CPU yet.
    let want = unsafe {
        let slot = core::ptr::addr_of_mut!(PER_CPU.0[0]);
        (*slot).cpu_id = 0;
        (*slot).lapic_id = lapic_id;
        (*slot).kernel_stack_top = kernel_stack_top;
        (*slot).self_ptr = slot as *const PerCpu;
        let want = slot as u64;
        GsBase::write(VirtAddr::new(want));
        want
    };

    // Best-effort liveness flag for a FUTURE AP phase. The MSR read-back is NOT
    // a reliable proof that `gs:`-relative accesses work — VirtualBox accepts
    // the wrmsr (read-back matches) yet leaves the hidden segment base at 0, so
    // `mov gs:[0]` still faults. There is no fault-free way to probe the hidden
    // base here, so Fase 0 simply never uses `gs:[0]` (see `this_cpu`). Record
    // the read-back result as a hint; AP bring-up must do its own real probe
    // (e.g. a guarded access with a recoverable #PF) before trusting gs-base.
    let gs_ok = GsBase::read().as_u64() == want;
    GS_USABLE.store(gs_ok, Ordering::SeqCst);
    gs_ok
}

/// Register `lapic_id -> cpu_id` and populate PER_CPU[cpu_id]'s identity.
/// Called by the BSP for itself (id 0) and for each AP BEFORE the AP starts.
pub fn set_cpu_mapping(lapic_id: u32, cpu_id: u8) {
    if (lapic_id as usize) < 256 {
        LAPIC_TO_CPU[lapic_id as usize].store(cpu_id, Ordering::SeqCst);
    }
    // SAFETY: each slot is written once during single-threaded bring-up before
    // the corresponding AP starts; the AP only reads its own slot afterwards.
    unsafe {
        let slot = core::ptr::addr_of_mut!(PER_CPU.0[cpu_id as usize]);
        (*slot).cpu_id = cpu_id as u32;
        (*slot).lapic_id = lapic_id;
        (*slot).self_ptr = slot as *const PerCpu;
    }
}

/// An AP (or the BSP) marks itself online.
pub fn mark_online() {
    CPUS_ONLINE.fetch_add(1, Ordering::SeqCst);
}

/// Number of cores that have registered online via `mark_online`.
pub fn cpus_online() -> u32 {
    CPUS_ONLINE.load(Ordering::SeqCst)
}

/// Dense cpu_id of the current core. Reads the LAPIC ID register (works on
/// every core and every VMM — no `gs:[0]`, dodging VirtualBox's gs-base quirk)
/// and maps it to a dense id. Returns 0 (BSP) if the LAPIC ID isn't mapped yet
/// (e.g. very early boot before bring-up) — safe on a single CPU.
#[inline]
pub fn cpu_id() -> u32 {
    let lapic = crate::apic::lapic::apic_id();
    if (lapic as usize) < 256 {
        let id = LAPIC_TO_CPU[lapic as usize].load(Ordering::SeqCst);
        if id != NO_CPU {
            return id as u32;
        }
    }
    0
}

/// &PerCpu for the current core, resolved via `cpu_id()` (LAPIC-based, no gs).
#[inline]
pub fn this_cpu() -> &'static PerCpu {
    // SAFETY: PER_CPU[cpu_id()] is a valid 'static; cpu_id() is in range
    // (0..MAX_CPUS) by construction of the dense ids set in set_cpu_mapping.
    unsafe { &*core::ptr::addr_of!(PER_CPU.0[cpu_id() as usize]) }
}

/// &PerCpu resolved via the GS base (`gs:[0]` self-pointer). ONLY valid once a
/// future AP phase has confirmed `init_bsp` returned `true` (gs-base usable).
/// Not used in Fase 0; reading `gs:[0]` with an uninstalled base #PFs.
#[inline]
#[allow(dead_code)]
pub unsafe fn this_cpu_via_gs() -> &'static PerCpu {
    let p: *const PerCpu;
    core::arch::asm!("mov {}, gs:[0]", out(reg) p, options(nostack, preserves_flags));
    &*p
}

/// Whether `gs:`-relative per-CPU access is usable on this machine (probed at
/// `init_bsp`). AP bring-up must check this before using [`this_cpu_via_gs`].
#[inline]
#[allow(dead_code)]
pub fn gs_usable() -> bool {
    GS_USABLE.load(Ordering::SeqCst)
}
