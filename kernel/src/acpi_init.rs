//! ACPI bring-up: take Limine's RSDP, parse it with the `acpi` crate, and
//! extract LAPIC base + IOAPIC base + IRQ source overrides from the MADT.

use acpi::{AcpiHandler, AcpiTables, InterruptModel, PhysicalMapping};
use alloc::vec::Vec;
use core::ptr::NonNull;

/// `AcpiHandler` impl that maps a physical address to the HHDM image Limine
/// already established. No real (un)mapping happens — HHDM mappings are
/// permanent for the lifetime of the kernel.
#[derive(Clone)]
struct HhdmHandler {
    hhdm_offset: u64,
}

impl AcpiHandler for HhdmHandler {
    unsafe fn map_physical_region<T>(&self, phys: usize, size: usize) -> PhysicalMapping<Self, T> {
        let virt = phys as u64 + self.hhdm_offset;
        // SAFETY: HHDM covers all physical memory; the address is mapped and
        // alignment requirements for ACPI table headers are met by Limine's
        // mapping (the `acpi` crate handles further internal alignment).
        unsafe {
            PhysicalMapping::new(
                phys,
                NonNull::new(virt as *mut T).expect("acpi: null virt after HHDM"),
                size,
                size,
                self.clone(),
            )
        }
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {
        // HHDM mappings are permanent — nothing to unmap.
    }
}

#[derive(Debug, Copy, Clone)]
pub struct IrqOverride {
    pub source: u8,
    pub global_system_interrupt: u32,
    pub active_low: bool,
    pub level_triggered: bool,
}

#[derive(Clone)]
pub struct AcpiInfo {
    pub lapic_base:  u64,
    pub ioapic_base: u64,
    pub overrides:   Vec<IrqOverride>,
    pub hhdm_offset: u64,
}

#[derive(Debug)]
pub enum AcpiInitError {
    NoRsdp,
    NoHhdm,
    RsdpBelowHhdm,
    Parse,
    NoLapic,
    NoIoapic,
}

impl core::fmt::Display for AcpiInitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AcpiInitError::NoRsdp        => f.write_str("no rsdp"),
            AcpiInitError::NoHhdm        => f.write_str("no hhdm"),
            AcpiInitError::RsdpBelowHhdm => f.write_str("rsdp below hhdm"),
            AcpiInitError::Parse         => f.write_str("parse"),
            AcpiInitError::NoLapic       => f.write_str("no lapic"),
            AcpiInitError::NoIoapic      => f.write_str("no ioapic"),
        }
    }
}

pub fn parse() -> Result<AcpiInfo, AcpiInitError> {
    let rsdp_resp = crate::RSDP_REQUEST.response().ok_or(AcpiInitError::NoRsdp)?;
    let hhdm_resp = crate::HHDM_REQUEST.response().ok_or(AcpiInitError::NoHhdm)?;

    let hhdm_offset = hhdm_resp.offset;
    let handler = HhdmHandler { hhdm_offset };

    // Limine base revisions other than 3 (we use MAX_SUPPORTED = 6) give a
    // virtual address (HHDM-mapped) in the RSDP response. The `acpi` crate's
    // `from_rsdp` expects a physical address that it then feeds back to our
    // handler, which re-adds the HHDM offset. Subtract it here so the round
    // trip lands on the same RSDP bytes.
    let rsdp_virt = rsdp_resp.address as u64;
    // checked_sub turns a contract violation (Limine returning a non-HHDM addr)
    // into a named error rather than silently zeroing the pointer.
    let rsdp_phys = rsdp_virt
        .checked_sub(hhdm_offset)
        .ok_or(AcpiInitError::RsdpBelowHhdm)? as usize;

    // SAFETY: rsdp_phys derives from a virtual address Limine guarantees points
    // at a valid RSDP structure, mapped read/write via the HHDM.
    let tables = unsafe {
        AcpiTables::from_rsdp(handler, rsdp_phys).map_err(|_| AcpiInitError::Parse)?
    };

    let platform = tables.platform_info().map_err(|_| AcpiInitError::Parse)?;
    let apic = match platform.interrupt_model {
        InterruptModel::Apic(a) => a,
        _ => return Err(AcpiInitError::NoLapic),
    };

    let lapic_base  = apic.local_apic_address;
    let ioapic_base = apic.io_apics.first().ok_or(AcpiInitError::NoIoapic)?.address as u64;

    let mut overrides: Vec<IrqOverride> = Vec::new();
    for iso in apic.interrupt_source_overrides.iter() {
        overrides.push(IrqOverride {
            source: iso.isa_source,
            global_system_interrupt: iso.global_system_interrupt,
            active_low: matches!(iso.polarity, acpi::platform::interrupt::Polarity::ActiveLow),
            level_triggered: matches!(iso.trigger_mode, acpi::platform::interrupt::TriggerMode::Level),
        });
    }

    Ok(AcpiInfo { lapic_base, ioapic_base, overrides, hhdm_offset })
}
