//! Global Descriptor Table + Task State Segment.
//!
//! A flat GDT with kernel + user code/data segments and a TSS descriptor.
//! The TSS reserves IST stack 0 for the Double Fault handler, so a `#DF`
//! triggered while the regular kernel stack is corrupted still has a clean
//! stack to land on (preventing an instant triple-fault).
//!
//! All three statics (DF stack, TSS, GDT+Selectors) are per-CPU arrays sized
//! `MAX_CPUS`. `init(cpu_id)` initialises the slot for one core; the BSP calls
//! `init(0)`. APs will call `init(n)` in a later SMP phase.

use crate::cpu::MAX_CPUS;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024; // 16 KiB per core

// One double-fault IST stack per core (~16 KiB each; MAX_CPUS=16 → 256 KiB BSS).
static mut DOUBLE_FAULT_STACK: [[u8; DOUBLE_FAULT_STACK_SIZE]; MAX_CPUS] =
    [[0; DOUBLE_FAULT_STACK_SIZE]; MAX_CPUS];

// TaskStateSegment derives Copy, so array-repeat works directly.
// Using a named const as the repeat expression is the most portable form.
const NEW_TSS: TaskStateSegment = TaskStateSegment::new();
static mut TSS: [TaskStateSegment; MAX_CPUS] = [NEW_TSS; MAX_CPUS];

// One GDT+Selectors per core, built lazily by init(cpu_id).
const ONCE: spin::Once<(GlobalDescriptorTable, Selectors)> = spin::Once::new();
static GDT: [spin::Once<(GlobalDescriptorTable, Selectors)>; MAX_CPUS] = [ONCE; MAX_CPUS];

#[derive(Copy, Clone)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_code:   SegmentSelector,
    pub user_data:   SegmentSelector,
    pub tss:         SegmentSelector,
}

pub fn init(cpu_id: usize) {
    use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
    use x86_64::instructions::tables::load_tss;

    // SAFETY: each core touches only its own slot during its own boot.
    unsafe {
        let stack_ptr   = core::ptr::addr_of!(DOUBLE_FAULT_STACK[cpu_id]);
        let stack_start = VirtAddr::from_ptr(stack_ptr);
        let stack_end   = stack_start + DOUBLE_FAULT_STACK_SIZE as u64;
        let tss_ptr     = core::ptr::addr_of_mut!(TSS[cpu_id]);
        (*tss_ptr).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_end;
    }

    let (gdt, sels) = GDT[cpu_id].call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_code   = gdt.append(Descriptor::user_code_segment());
        let user_data   = gdt.append(Descriptor::user_data_segment());
        // SAFETY: TSS[cpu_id] lives forever in BSS.
        let tss = gdt.append(unsafe {
            Descriptor::tss_segment(&*core::ptr::addr_of!(TSS[cpu_id]))
        });
        (gdt, Selectors { kernel_code, kernel_data, user_code, user_data, tss })
    });

    gdt.load();
    // SAFETY: selectors above match the GDT just loaded.
    unsafe {
        CS::set_reg(sels.kernel_code);
        DS::set_reg(sels.kernel_data);
        ES::set_reg(sels.kernel_data);
        FS::set_reg(sels.kernel_data);
        GS::set_reg(sels.kernel_data);
        SS::set_reg(sels.kernel_data);
        load_tss(sels.tss);
    }
}

/// Returns the GDT selectors for the BSP (slot 0).
///
/// All current callers run on the BSP during early boot. When AP support
/// arrives a `selectors_for(cpu_id)` variant can be added.
pub fn selectors() -> Selectors {
    GDT[0].get().expect("gdt::init() not called").1
}
