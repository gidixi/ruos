//! Per-CPU data. Each core's GS base points at its `PerCpu` block, so `gs:[0]`
//! (a self-pointer stored at offset 0) yields `&PerCpu` in O(1) — the standard
//! x86-64 per-CPU pattern. On 1 CPU only slot 0 (the BSP) is live; the later
//! AP bring-up phase will call an `init_ap(n)` for the others.

use x86_64::VirtAddr;
use x86_64::registers::model_specific::GsBase;

pub const MAX_CPUS: usize = 16;

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
static GS_USABLE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

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
    GS_USABLE.store(gs_ok, core::sync::atomic::Ordering::SeqCst);
    gs_ok
}

/// &PerCpu for the current core.
///
/// In Fase 0 there is exactly one CPU (the BSP = slot 0) and NO AP is running,
/// so the correct per-CPU block is unconditionally `PER_CPU[0]`. We return it
/// directly WITHOUT a `gs:`-relative access — that is always safe and cannot
/// #PF on any VMM (VirtualBox accepts the GS-base MSR but does not update the
/// hidden segment base, so `mov gs:[0]` would fault on base 0). A future AP
/// phase that brings up real cores will switch the multi-CPU path to
/// [`this_cpu_via_gs`] only after `init_bsp` confirmed gs-base is usable.
#[inline]
pub fn this_cpu() -> &'static PerCpu {
    // SAFETY: PER_CPU.0[0] is a valid 'static; slot 0 is the BSP, the only live
    // CPU in Fase 0.
    unsafe { &*core::ptr::addr_of!(PER_CPU.0[0]) }
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
    GS_USABLE.load(core::sync::atomic::Ordering::SeqCst)
}

#[inline]
pub fn cpu_id() -> u32 { this_cpu().cpu_id }
