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
/// Sets PER_CPU[0] and the GS base so `this_cpu()` works thereafter.
pub fn init_bsp(kernel_stack_top: u64) {
    let lapic_id = crate::apic::lapic::apic_id();
    // SAFETY: single-threaded boot; no other accessor to PER_CPU yet.
    unsafe {
        let slot = core::ptr::addr_of_mut!(PER_CPU.0[0]);
        (*slot).cpu_id = 0;
        (*slot).lapic_id = lapic_id;
        (*slot).kernel_stack_top = kernel_stack_top;
        (*slot).self_ptr = slot as *const PerCpu;
        GsBase::write(VirtAddr::new(slot as u64));
    }
}

/// &PerCpu for the current core, via gs:[0] (the self-pointer at offset 0).
#[inline]
pub fn this_cpu() -> &'static PerCpu {
    let p: *const PerCpu;
    // SAFETY: init_bsp set the GS base to a valid PerCpu whose offset 0 holds
    // its own self_ptr.
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) p, options(nostack, preserves_flags));
        &*p
    }
}

#[inline]
pub fn cpu_id() -> u32 { this_cpu().cpu_id }
