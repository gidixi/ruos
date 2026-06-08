//! AHCI HBA — global controller registers + reset + port enumeration.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

use pci_types::Bar;
use x86_64::{PhysAddr, VirtAddr};

/// Global HBA register offsets from ABAR (AHCI 1.3 §3.1).
const REG_CAP:  usize = 0x00; // Capabilities
const REG_GHC:  usize = 0x04; // Global Host Control
const REG_IS:   usize = 0x08; // Interrupt Status
const REG_PI:   usize = 0x0C; // Ports Implemented (bitmap, 32 bits)
const REG_VS:   usize = 0x10; // Version

const GHC_AE:   u32 = 1 << 31; // AHCI Enable
const GHC_HR:   u32 = 1 << 0;  // HBA Reset (self-clearing)
const GHC_IE:   u32 = 1 << 1;  // IRQ enable (we leave masked for polling)

/// Cap on the reset-wait loop. At 100 Hz this is ~1 s.
const RESET_TIMEOUT_TICKS: u64 = 100;

#[derive(Debug, Clone, Copy)]
pub enum AhciError {
    NotFound,
    BarMissing,
    ResetTimeout,
    UnsupportedVersion(u32),
}

impl fmt::Display for AhciError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AhciError::NotFound                => write!(f, "no AHCI HBA found"),
            AhciError::BarMissing              => write!(f, "BAR5 missing"),
            AhciError::ResetTimeout            => write!(f, "HBA reset timeout"),
            AhciError::UnsupportedVersion(v)   => write!(f, "unsupported version 0x{:08x}", v),
        }
    }
}

/// Snapshot of the discovered HBA. Stored after `init()` so the storage phase
/// can hand it to `port::AhciPort` constructors.
#[derive(Clone, Copy)]
pub struct Hba {
    /// MMIO virtual base (ABAR mapped via `map_io_range`).
    pub abar: VirtAddr,
    /// `CAP` snapshot at init time.
    pub cap:  u32,
    /// `VS` snapshot.
    pub vs:   u32,
    /// `PI` (ports-implemented) bitmap — bit N set ⇔ port N populated by HBA.
    pub pi:   u32,
}

impl Hba {
    /// Read a 32-bit HBA register (volatile).
    #[inline]
    pub fn reg_read(&self, off: usize) -> u32 {
        unsafe { read_volatile((self.abar.as_u64() as usize + off) as *const u32) }
    }
    /// Write a 32-bit HBA register (volatile).
    #[inline]
    pub fn reg_write(&self, off: usize, v: u32) {
        unsafe { write_volatile((self.abar.as_u64() as usize + off) as *mut u32, v); }
    }

    /// Discover the first AHCI HBA on the PCI bus, map ABAR, reset, enable
    /// AHCI mode.
    pub fn find_and_init() -> Result<Self, AhciError> {
        let dev = crate::pci::devices().into_iter().find(|d|
            d.class == 0x01 && d.subclass == 0x06 && d.prog_if == 0x01
        ).ok_or(AhciError::NotFound)?;
        Self::init_dev(dev)
    }

    /// Every AHCI controller present on the PCI bus, initialized, EXCEPT the one
    /// whose ABAR equals `skip` (the already-initialized boot HBA — we don't want
    /// to reset it again here). Used to find an ATAPI CD-ROM that may live on a
    /// different HBA than the boot one (e.g. QEMU q35 builtin ICH9 vs an added
    /// `-device ahci`). On real single-HBA hardware this returns nothing extra.
    pub fn find_all_except(skip: Option<VirtAddr>) -> alloc::vec::Vec<Self> {
        let mut out = alloc::vec::Vec::new();
        for dev in crate::pci::devices().into_iter().filter(|d|
            d.class == 0x01 && d.subclass == 0x06 && d.prog_if == 0x01
        ) {
            // Compute this controller's ABAR WITHOUT initializing, so we can skip
            // the boot HBA (re-resetting it would disturb the SATA /mnt bringup).
            let phys = match dev.bar(5) {
                Some(Bar::Memory32 { address, .. }) => address as u64,
                Some(Bar::Memory64 { address, .. }) => address,
                _ => continue,
            };
            if Some(crate::memory::mapper::hhdm_virt(PhysAddr::new(phys))) == skip {
                continue;
            }
            if let Ok(h) = Self::init_dev(dev) { out.push(h); }
        }
        out
    }

    /// Initialize a specific AHCI controller: map ABAR, enable AHCI mode, reset.
    pub fn init_dev(dev: crate::pci::PciDevice) -> Result<Self, AhciError> {
        dev.enable_mmio();
        dev.enable_bus_master();

        let (phys, size) = match dev.bar(5).ok_or(AhciError::BarMissing)? {
            Bar::Memory32 { address, size, .. } => (address as u64, size as u64),
            Bar::Memory64 { address, size, .. } => (address, size),
            Bar::Io { .. } => return Err(AhciError::BarMissing),
        };
        let abar = crate::memory::mapper::map_io_range(PhysAddr::new(phys), size as usize)
            .map_err(|_| AhciError::BarMissing)?;

        let hba = Self { abar, cap: 0, vs: 0, pi: 0 };

        // 1. Enable AHCI host-controlled mode (some firmware boots in legacy
        //    IDE compat mode; AHCI enable is mandatory before any other
        //    config is meaningful).
        let ghc = hba.reg_read(REG_GHC);
        hba.reg_write(REG_GHC, ghc | GHC_AE);

        // 2. HBA reset.
        hba.reg_write(REG_GHC, hba.reg_read(REG_GHC) | GHC_HR);
        let start = crate::timer::ticks();
        while (hba.reg_read(REG_GHC) & GHC_HR) != 0 {
            if crate::timer::ticks().wrapping_sub(start) > RESET_TIMEOUT_TICKS {
                return Err(AhciError::ResetTimeout);
            }
            core::hint::spin_loop();
        }

        // 3. Re-enable AHCI mode after reset (HR clears AE).
        hba.reg_write(REG_GHC, hba.reg_read(REG_GHC) | GHC_AE);
        // Mask all HBA-level interrupts — polling mode for Step 15.
        hba.reg_write(REG_GHC, hba.reg_read(REG_GHC) & !GHC_IE);

        let cap = hba.reg_read(REG_CAP);
        let vs  = hba.reg_read(REG_VS);
        let pi  = hba.reg_read(REG_PI);

        let port_count = pi.count_ones();
        crate::binfo!(
            "ahci",
            "HBA up cap=0x{:08x} vs=0x{:08x} ports={} pi=0x{:08x}",
            cap, vs, port_count, pi,
        );

        Ok(Self { abar, cap, vs, pi })
    }
}
