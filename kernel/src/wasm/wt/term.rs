//! Terminal host module (`term`) — bridges a GUI terminal window to a real shell
//! on a kernel PTY pair. Modeled on `wm::add_to_linker`: raw `func_wrap` host fns
//! over the EXISTING PTY API (`crate::pty::*`) + `spawn_shell_on_pty`. This is the
//! same bridge SSH uses (`ssh/sunset_io.rs`), with "egui window" in place of
//! "SSH channel": claim a pair + spawn `/bin/shell.wasm`, push the user's bytes to
//! the master input (through the line discipline), drain master output
//! non-blocking, and SIGHUP + release on close.
//!
//! Handle = the PTY pair index (`0..NUM_PAIRS`). `-1` means "no free pair".
//! All five fns are no-ops / `-1` for an out-of-range handle (graceful).

use alloc::vec::Vec;
use wasmtime::{Caller, Linker};
use crate::wasm::wt::wm::HasWindow;

/// Register `term.{open,read,write,resize,close}` on `linker`. Mirror of
/// `wm::add_to_linker`; called from the same window-Linker build sites.
pub fn add_to_linker<T: HasWindow + 'static>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    // term.open() -> i32: claim a free PTY pair and spawn the shell on it.
    // Returns the handle (pair idx), or -1 if all pairs are busy. Pair 0 is the
    // framebuffer console's boot shell — start at 1 (same convention as SSH).
    linker.func_wrap("term", "open", |_caller: Caller<'_, T>| -> i32 {
        for idx in 1..crate::pty::NUM_PAIRS {
            if crate::pty::try_claim(idx) {
                crate::pty::set_origin(idx, crate::pty::PtyOrigin::LocalGui);
                crate::wasm::ssh_spawn::spawn_shell_on_pty(idx);
                return idx as i32;
            }
        }
        -1
    })?;

    // term.read(h, ptr, cap) -> i32: drain up to `cap` bytes of shell output into
    // guest memory at `ptr` (NON-blocking). Returns the byte count, 0 if nothing
    // is ready, or -1 (EOF) once the shell exited AND its output is fully drained
    // (same condition as the SSH bridge's channel-close, sunset_io.rs).
    linker.func_wrap("term", "read",
        |mut caller: Caller<'_, T>, h: i32, ptr: i32, cap: i32| -> i32 {
            if h < 0 { return -1; }
            let idx = h as usize;
            if idx >= crate::pty::NUM_PAIRS { return -1; }
            if !crate::pty::is_claimed(idx) && crate::pty::master_output_len(idx) == 0 {
                return -1; // EOF: shell gone, nothing left to read.
            }
            let want = cap.max(0) as usize;
            let mut out: Vec<u8> = Vec::with_capacity(want.min(4096));
            while out.len() < want {
                match crate::pty::master_output_try(idx) {
                    Some(b) => out.push(b),
                    None => break,
                }
            }
            if out.is_empty() { return 0; }
            crate::wasm::wt::mem::write(&mut caller, ptr as u32, &out);
            out.len() as i32
        })?;

    // term.write(h, ptr, len): user keystrokes -> shell input (via line discipline).
    linker.func_wrap("term", "write",
        |mut caller: Caller<'_, T>, h: i32, ptr: i32, len: i32| {
            if h < 0 { return; }
            let idx = h as usize;
            if idx >= crate::pty::NUM_PAIRS || !crate::pty::is_claimed(idx) { return; }
            if let Some(b) = crate::wasm::wt::mem::read(&mut caller, ptr as u32, len as u32) {
                for &byte in &b { crate::pty::master_input_push(idx, byte); }
            }
        })?;

    // term.resize(h, cols, rows): record the window size for the pair.
    linker.func_wrap("term", "resize",
        |_caller: Caller<'_, T>, h: i32, cols: i32, rows: i32| {
            if h < 0 { return; }
            let idx = h as usize;
            if idx >= crate::pty::NUM_PAIRS { return; }
            crate::pty::set_winsize(idx, cols.max(0) as u16, rows.max(0) as u16);
        })?;

    // term.close(h): SIGHUP the shell + release the pair for reuse.
    linker.func_wrap("term", "close", |_caller: Caller<'_, T>, h: i32| {
        if h < 0 { return; }
        let idx = h as usize;
        if idx >= crate::pty::NUM_PAIRS { return; }
        crate::pty::request_shutdown(idx);
        crate::pty::release(idx);
    })?;

    Ok(())
}
