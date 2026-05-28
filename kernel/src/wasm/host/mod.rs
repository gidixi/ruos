pub mod fd;
pub mod lifecycle;

use wasmi::{Error, Linker};
use crate::wasm::state::RuntimeState;

pub fn install(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    lifecycle::link(linker)?;
    fd::link(linker)?;
    Ok(())
}
