//! Boot phase sub-modules. Each module exposes a single `init()` that runs one
//! logical phase of kernel bring-up and returns a `BootError` on failure.

pub mod arch;
pub mod mem;
pub mod interrupts;
pub mod pci;
pub mod usb;
pub mod devices;
pub mod fs;
pub mod storage;
pub mod media_bin;
pub mod userland;

use spin::Mutex;
use crate::acpi_init::AcpiInfo;

/// ACPI information produced by `mem` phase and consumed by `interrupts` phase.
static ACPI: Mutex<Option<AcpiInfo>> = Mutex::new(None);

pub(super) fn set_acpi_info(info: AcpiInfo) {
    *ACPI.lock() = Some(info);
}

pub(super) fn get_acpi_info() -> AcpiInfo {
    ACPI.lock().clone().expect("acpi not initialized before interrupts phase")
}
