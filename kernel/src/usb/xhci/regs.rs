use core::num::NonZeroUsize;
use xhci::accessor::Mapper;
use x86_64::PhysAddr;

#[derive(Clone)]
pub struct HhdmMapper;

impl Mapper for HhdmMapper {
    unsafe fn map(&mut self, phys_start: usize, bytes: usize) -> NonZeroUsize {
        let virt = crate::memory::mapper::map_io_range(
            PhysAddr::new(phys_start as u64), bytes)
            .expect("xhci: mmio map failed");
        NonZeroUsize::new(virt.as_u64() as usize).expect("xhci: null mmio virt")
    }
    fn unmap(&mut self, _virt_start: usize, _bytes: usize) {}
}
