pub mod lifecycle;
pub mod fd;
pub mod path;
pub mod clock;
pub mod random;
pub mod sock;
pub mod proc;
pub mod term;
pub mod sysinfo;
pub mod service;
pub mod mem;
pub mod smp;

use wasmi::{Linker, Error};
use crate::wasm::state::RuntimeState;

pub fn install(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    lifecycle::link(linker)?;
    fd::link(linker)?;
    path::link(linker)?;
    clock::link(linker)?;
    random::link(linker)?;
    sock::link(linker)?;
    proc::link(linker)?;
    term::link(linker)?;
    sysinfo::link(linker)?;
    service::link(linker)?;
    smp::link(linker)?;
    Ok(())
}
