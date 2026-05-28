//! Global Descriptor Table + Task State Segment.
//!
//! A flat GDT with kernel + user code/data segments and a TSS descriptor.
//! The TSS reserves IST stack 0 for the Double Fault handler, so a `#DF`
//! triggered while the regular kernel stack is corrupted still has a clean
//! stack to land on (preventing an instant triple-fault).

use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024; // 16 KiB

static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

static mut TSS: TaskStateSegment = TaskStateSegment::new();

static GDT: spin::Once<(GlobalDescriptorTable, Selectors)> = spin::Once::new();

#[derive(Copy, Clone)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_code:   SegmentSelector,
    pub user_data:   SegmentSelector,
    pub tss:         SegmentSelector,
}

pub fn init() {
    use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
    use x86_64::instructions::tables::load_tss;

    // SAFETY: single-threaded boot, no other accessors to TSS/stack yet.
    unsafe {
        let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
        let stack_end   = stack_start + DOUBLE_FAULT_STACK_SIZE as u64;
        TSS.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_end;
    }

    let (gdt, sels) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_code   = gdt.append(Descriptor::user_code_segment());
        let user_data   = gdt.append(Descriptor::user_data_segment());
        // SAFETY: TSS lives forever in BSS.
        let tss = gdt.append(unsafe { Descriptor::tss_segment(&*core::ptr::addr_of!(TSS)) });
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

pub fn selectors() -> Selectors {
    GDT.get().expect("gdt::init() not called").1
}
