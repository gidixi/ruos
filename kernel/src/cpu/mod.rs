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

/// BSP per-CPU init. Call AFTER gdt::init (the GS segment-load done there zeroes
/// the GS base) and AFTER lapic::init (the APIC ID register must be mapped).
/// Sets PER_CPU[0] and the GS base so `this_cpu()` resolves via `gs:[0]`.
///
/// Returns `true` if the GS-base write took effect (verified by read-back),
/// `false` if the VMM/CPU silently ignored `wrmsr IA32_GS_BASE` — VirtualBox
/// does this. On `false`, `this_cpu()` falls back to the BSP slot, so boot
/// completes on a single CPU regardless. A future AP bring-up phase MUST
/// require a `true` return before relying on per-core gs-base.
pub fn init_bsp(kernel_stack_top: u64) -> bool {
    let lapic_id = crate::apic::lapic::apic_id();
    // SAFETY: single-threaded boot; no other accessor to PER_CPU yet.
    unsafe {
        let slot = core::ptr::addr_of_mut!(PER_CPU.0[0]);
        (*slot).cpu_id = 0;
        (*slot).lapic_id = lapic_id;
        (*slot).kernel_stack_top = kernel_stack_top;
        (*slot).self_ptr = slot as *const PerCpu;
        let want = slot as u64;
        GsBase::write(VirtAddr::new(want));
        // Verify the write took: some VMMs (VirtualBox) silently drop wrmsr to
        // IA32_GS_BASE. If the read-back doesn't match, gs:[0] is unusable.
        GsBase::read().as_u64() == want
    }
}

/// &PerCpu for the current core.
///
/// Reads the self-pointer at `gs:[0]`. If the GS base was never installed
/// (gs:[0] == 0 — e.g. a VMM that ignored the GS-base write, or a call before
/// `init_bsp`), falls back to the BSP slot `PER_CPU[0]`. This keeps boot alive
/// on a single CPU even when gs-base is unavailable; it is correct because slot
/// 0 IS the BSP and no AP is running. (AP bring-up will gate on init_bsp's bool.)
#[inline]
pub fn this_cpu() -> &'static PerCpu {
    // Check the GS base MSR first. If it is 0 (never installed, or a VMM that
    // ignored the write), reading `gs:[0]` would dereference linear address 0
    // and #PF — so we must NOT touch gs:[0] in that case. Read the MSR instead
    // and fall back to the BSP slot.
    if GsBase::read().as_u64() == 0 {
        // SAFETY: PER_CPU.0[0] is a valid 'static; slot 0 is always the BSP.
        return unsafe { &*core::ptr::addr_of!(PER_CPU.0[0]) };
    }
    let p: *const PerCpu;
    // SAFETY: GS base is non-zero, set by init_bsp to a valid PerCpu whose
    // offset 0 holds its own self-pointer.
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) p, options(nostack, preserves_flags));
        &*p
    }
}

#[inline]
pub fn cpu_id() -> u32 { this_cpu().cpu_id }
