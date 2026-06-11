//! Wasmtime Component Model runners.
//!
//! * `run_component` — the bring-up gate (`ruos:bringup`, boot-check).
//! * `run_tui_component` — runs a `ruos:tui/tui-app` component (e.g.
//!   `rtop.cwasm`) dynamically linked against the shared `tui.cwasm`
//!   provider: both components are instantiated in ONE store; the app's
//!   `ruos:tui/canvas` imports are satisfied by kernel func shims that
//!   forward to the provider instance's exports (typed call + post_return).
//!   The `ruos:tui/host` imports (sysinfo blobs, keys, raw mode, tty out)
//!   are implemented here against kernel services.

use crate::kprintln;
use crate::wasm::wt::engine;
use alloc::string::String;
use alloc::vec::Vec;
use wasmtime::component::{Component, ComponentType, HasSelf, Lift, Linker, Lower, TypedFunc};
use wasmtime::{Store, StoreContextMut};

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

pub fn run_component(cwasm: &[u8]) -> i32 {
    let engine = engine();
    let component = match unsafe { Component::deserialize(engine, cwasm) } {
        Ok(c) => c,
        Err(e) => { kprintln!("ruos: component deserialize err: {:?}", e); return -1; }
    };
    let mut store = Store::new(engine, BringupHost);
    // Bring-up boot-check, not a compositor window: no watchdog (the default
    // deadline 0 would trap the first guest instruction otherwise).
    store.set_epoch_deadline(crate::wasm::wt::NO_DEADLINE_TICKS);
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

// ── ruos:tui — dynamic component-to-component linking ───────────────────────

/// Host-side bindgen for the `ruos:tui/host` interface only (world
/// `tui-host`); `canvas` is linked manually with func shims, so it must NOT
/// get a bindgen Host trait (its impl needs the store to re-enter wasm).
mod hostif {
    wasmtime::component::bindgen!({
        path: "../wit/ruos-tui.wit",
        world: "tui-host",
    });
}

/// `ruos:tui/canvas` record types, wire-compatible with the WIT records.
/// Kept here (not bindgen) so the canvas shims can name them in TypedFunc
/// signatures without dragging in a second world's bindings.
#[derive(ComponentType, Lift, Lower, Copy, Clone)]
#[component(record)]
struct Rect {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

#[derive(ComponentType, Lift, Lower, Copy, Clone)]
#[component(record)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
}

/// Typed handles into the tui-provider instance's canvas exports.
/// `TypedFunc` is a small Copy handle, so the linker shims can own a copy
/// and re-enter the provider through the StoreContextMut they receive.
#[derive(Copy, Clone)]
struct CanvasFns {
    init:       TypedFunc<(u16, u16), ()>,
    clear:      TypedFunc<(), ()>,
    draw_text:  TypedFunc<(String, Rect, Color), ()>,
    draw_bar:   TypedFunc<(u8, Rect, Color), ()>,
    draw_table: TypedFunc<(Vec<String>, Vec<Vec<String>>, Vec<u16>, Rect), ()>,
    flush:      TypedFunc<(), (String,)>,
}

/// Store data for a tui-app run.
struct TuiAppState {
    /// PTY idx of the app's controlling terminal (keys + termios).
    pts: Option<usize>,
    /// VFS fd open for WRITE on /dev/pts/N (write-tty); None → console.
    out_fd: Option<crate::vfs::Fd>,
    /// Termios saved by set-raw(true), restored by set-raw(false)/teardown.
    saved_termios: Option<crate::pty::termios::Termios>,
}

impl hostif::ruos::tui::host::Host for TuiAppState {
    fn cpustat(&mut self) -> Vec<u8> {
        crate::wasm::host::sysinfo::cpustat_blob()
    }

    fn proc_stat(&mut self) -> Vec<u8> {
        crate::wasm::host::sysinfo::proc_stat_blob()
    }

    fn meminfo(&mut self) -> Vec<u8> {
        crate::wasm::host::sysinfo::meminfo_blob().to_vec()
    }

    fn uptime(&mut self) -> u64 {
        crate::timer::ticks()
    }

    /// Wait up to `timeout_ticks` (100 Hz) for one byte from the PTY slave
    /// ring. Component calls have no fiber to suspend on, so the wait happens
    /// in-place on the ComputeApp core — but it SLEEPS (`sti; hlt`) instead
    /// of spinning: the AP's own 100 Hz LAPIC timer wakes it every tick to
    /// re-check ring/deadline/kill, so the core idles at ~0% between checks
    /// and a keystroke is picked up within ≤10 ms. On the BSP it would starve
    /// the executor that PUMPS the PTY, so it returns EOF immediately there
    /// (interactive mode needs SMP).
    /// Returns -1 EOF/no-tty/killed, 0 timeout, 1..=256 byte+1.
    fn poll_key(&mut self, timeout_ticks: i64) -> i32 {
        let Some(idx) = self.pts else { return -1; };
        if crate::cpu::cpu_id() == 0 { return -1; }
        let deadline = crate::timer::ticks() + timeout_ticks.max(0) as u64;
        loop {
            if crate::pty::is_shutdown(idx) { return -1; }
            // `kill <pid>` / cooked-VINTR on the foreground app: report EOF so
            // the app unwinds its loop and runs its own terminal teardown
            // (the apps spend ~99% of their time inside this call).
            if crate::pty::foreground_pid(idx)
                .map(|p| crate::proc::is_kill_pending(p))
                .unwrap_or(false)
            {
                return -1;
            }
            if let Some(b) = crate::pty::slave_rx_ring(idx).pop() {
                return 1 + b as i32;
            }
            if crate::timer::ticks() >= deadline { return 0; }
            // Sleep until the next interrupt (AP timer tick or any IPI); the
            // sti shadow means the wake IRQ can't slip between sti and hlt.
            // No locks are held here, so parking the core is safe.
            x86_64::instructions::interrupts::enable_and_hlt();
        }
    }

    fn set_raw(&mut self, raw: bool) {
        let Some(idx) = self.pts else { return; };
        use crate::pty::termios::{ICANON, ECHO, ISIG};
        if raw {
            let saved = crate::pty::termios_snapshot(idx);
            let mut t = saved;
            t.c_lflag &= !(ICANON | ECHO | ISIG);
            self.saved_termios = Some(saved);
            crate::pty::set_termios(idx, t);
        } else if let Some(saved) = self.saved_termios.take() {
            crate::pty::set_termios(idx, saved);
        }
    }

    fn write_tty(&mut self, s: String) {
        match self.out_fd {
            Some(fd) => { let _ = crate::vfs::block_on(crate::vfs::write(fd, s.as_bytes())); }
            None => {
                use core::fmt::Write as _;
                let mut c = crate::console::CONSOLE.lock();
                let _ = c.write_str(&s);
            }
        }
    }
}

/// Extract the canvas TypedFuncs from the instantiated provider.
fn canvas_fns(
    store: &mut Store<TuiAppState>,
    component: &Component,
    instance: &wasmtime::component::Instance,
) -> Result<CanvasFns, wasmtime::Error> {
    let iface = component
        .get_export_index(None, "ruos:tui/canvas")
        .ok_or_else(|| wasmtime::Error::msg("tui.cwasm: no ruos:tui/canvas export"))?;
    let mut idx = |name: &str| {
        component
            .get_export_index(Some(&iface), name)
            .ok_or_else(|| wasmtime::Error::msg("tui.cwasm: missing canvas func"))
    };
    let init       = idx("init")?;
    let clear      = idx("clear")?;
    let draw_text  = idx("draw-text")?;
    let draw_bar   = idx("draw-bar")?;
    let draw_table = idx("draw-table")?;
    let flush      = idx("flush")?;
    Ok(CanvasFns {
        init:       instance.get_typed_func(&mut *store, init)?,
        clear:      instance.get_typed_func(&mut *store, clear)?,
        draw_text:  instance.get_typed_func(&mut *store, draw_text)?,
        draw_bar:   instance.get_typed_func(&mut *store, draw_bar)?,
        draw_table: instance.get_typed_func(&mut *store, draw_table)?,
        flush:      instance.get_typed_func(&mut *store, flush)?,
    })
}

/// Register `ruos:tui/canvas` shims that forward into the provider instance
/// (wasmtime 45 runs post-return automatically after each typed call).
fn add_canvas_shims(
    linker: &mut Linker<TuiAppState>,
    c: CanvasFns,
) -> Result<(), wasmtime::Error> {
    let mut li = linker.instance("ruos:tui/canvas")?;
    let f = c.init;
    li.func_wrap("init", move |mut s: StoreContextMut<'_, TuiAppState>, p: (u16, u16)| {
        f.call(&mut s, p)
    })?;
    let f = c.clear;
    li.func_wrap("clear", move |mut s: StoreContextMut<'_, TuiAppState>, _: ()| {
        f.call(&mut s, ())
    })?;
    let f = c.draw_text;
    li.func_wrap("draw-text", move |mut s: StoreContextMut<'_, TuiAppState>, p: (String, Rect, Color)| {
        f.call(&mut s, p)
    })?;
    let f = c.draw_bar;
    li.func_wrap("draw-bar", move |mut s: StoreContextMut<'_, TuiAppState>, p: (u8, Rect, Color)| {
        f.call(&mut s, p)
    })?;
    let f = c.draw_table;
    li.func_wrap("draw-table", move |mut s: StoreContextMut<'_, TuiAppState>, p: (Vec<String>, Vec<Vec<String>>, Vec<u16>, Rect)| {
        f.call(&mut s, p)
    })?;
    let f = c.flush;
    li.func_wrap("flush", move |mut s: StoreContextMut<'_, TuiAppState>, _: ()| {
        f.call(&mut s, ())
    })?;
    Ok(())
}

/// Run a `ruos:tui/tui-app` component (`run(once) -> s32`) against the
/// shared tui provider. Placement contract is run_cwasm's: called on a
/// ComputeApp core when one exists (interactive poll-key spin-waits there),
/// BSP-inline otherwise (poll-key degrades to EOF → one frame, no loop).
pub fn run_tui_component(app_cwasm: &[u8], tui_cwasm: &[u8], once: bool, pts: Option<usize>) -> i32 {
    let engine = engine();
    let tui_comp = match unsafe { Component::deserialize(engine, tui_cwasm) } {
        Ok(c) => c,
        Err(e) => { kprintln!("ruos: tui deserialize err: {:?}", e); return 126; }
    };
    let app_comp = match unsafe { Component::deserialize(engine, app_cwasm) } {
        Ok(c) => c,
        Err(e) => { kprintln!("ruos: tui-app deserialize err: {:?}", e); return 126; }
    };

    let mut state = TuiAppState { pts, out_fd: None, saved_termios: None };
    if let Some(n) = pts {
        let path = alloc::format!("/dev/pts/{}", n);
        if let Ok(fd) = crate::vfs::block_on(crate::vfs::open(&path, crate::vfs::OpenFlags::WRITE)) {
            state.out_fd = Some(fd);
        }
    }
    let mut store = Store::new(engine, state);

    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }

    // 1. Provider first: no imports, instantiated from an empty linker.
    let tui_linker: Linker<TuiAppState> = Linker::new(engine);
    let tui_inst = match tui_linker.instantiate(&mut store, &tui_comp) {
        Ok(i) => i,
        Err(e) => { kprintln!("ruos: tui instantiate err: {:?}", e); return 126; }
    };
    let canvas = match canvas_fns(&mut store, &tui_comp, &tui_inst) {
        Ok(c) => c,
        Err(e) => { kprintln!("ruos: tui exports err: {:?}", e); return 126; }
    };

    // 2. App linker: host iface (kernel) + canvas shims (→ provider).
    let mut linker: Linker<TuiAppState> = Linker::new(engine);
    if let Err(e) = hostif::TuiHost::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s) {
        kprintln!("ruos: tui host link err: {:?}", e); return 126;
    }
    if let Err(e) = add_canvas_shims(&mut linker, canvas) {
        kprintln!("ruos: tui canvas link err: {:?}", e); return 126;
    }
    let app_inst = match linker.instantiate(&mut store, &app_comp) {
        Ok(i) => i,
        Err(e) => { kprintln!("ruos: tui-app instantiate err: {:?}", e); return 126; }
    };

    let run = match app_inst.get_typed_func::<(bool,), (i32,)>(&mut store, "run") {
        Ok(f) => f,
        Err(e) => { kprintln!("ruos: tui-app no run export: {:?}", e); return 126; }
    };
    let code = match run.call(&mut store, (once,)) {
        Ok((c,)) => c,
        Err(e) => { kprintln!("ruos: tui-app trap: {:?}", e); 134 }
    };

    // Teardown: never leave the PTY raw, even if the app trapped mid-run.
    if let Some(saved) = store.data_mut().saved_termios.take() {
        if let Some(idx) = store.data().pts {
            crate::pty::set_termios(idx, saved);
        }
    }
    if let Some(fd) = store.data().out_fd {
        let _ = crate::vfs::block_on(crate::vfs::close(fd));
    }
    code
}
