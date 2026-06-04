//! Component Model bring-up: run a `ruos:bringup` component via wasmtime's no_std
//! component runtime (Component::deserialize + component::Linker), proving the AOT
//! component path works on bare metal. Mirrors run_cwasm's engine reuse.

use crate::kprintln;
use crate::wasm::wt::engine;
use alloc::string::String;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::Store;

// Generate host bindings from the SAME WIT the guest used. `path` is relative to
// the kernel crate root.
wasmtime::component::bindgen!({
    path: "../wit/ruos-bringup.wit",
    world: "bringup",
});

struct BringupHost;

impl ruos::bringup::system::Host for BringupHost {
    fn log(&mut self, msg: String) {
        kprintln!("[component] {}", msg);
    }
    fn poweroff(&mut self) {
        crate::power::poweroff();
    }
}

/// Deserialize + instantiate + call `run` on a precompiled bring-up component.
pub fn run_component(cwasm: &[u8]) -> i32 {
    let engine = engine();
    // SAFETY: produced by wt-precompile --component for this exact engine Config.
    let component = match unsafe { Component::deserialize(engine, cwasm) } {
        Ok(c) => c,
        Err(e) => { kprintln!("ruos: component deserialize err: {:?}", e); return -1; }
    };
    let mut store = Store::new(engine, BringupHost);
    let mut linker: Linker<BringupHost> = Linker::new(engine);
    if let Err(e) = Bringup::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s) {
        kprintln!("ruos: component link err: {:?}", e); return -2;
    }
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    let bringup = match Bringup::instantiate(&mut store, &component, &linker) {
        Ok(b) => b,
        Err(e) => { kprintln!("ruos: component instantiate err: {:?}", e); return -3; }
    };
    match bringup.call_run(&mut store) {
        Ok(code) => code,
        Err(e) => { kprintln!("ruos: component run err: {:?}", e); -4 }
    }
}
