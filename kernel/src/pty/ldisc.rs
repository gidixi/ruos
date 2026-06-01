//! Line discipline. Cooked mode (default) does echo + backspace + line
//! buffer + Ctrl-C signaling. Raw mode passes bytes through unchanged.

use super::pair::PtyPair;
use super::termios::*;

/// Process one input byte arriving at the master end (e.g. from the keyboard
/// ISR or the SSH bridge). Handles termios.c_lflag modes.
///
/// Returns `Some(pid)` when a cooked-mode `^C` (VINTR) should cooperatively
/// kill the foreground app `pid`; the caller performs the kill AFTER releasing
/// the pair lock (this fn must not touch the proc registry under the lock).
#[must_use]
pub fn process_input(pair: &mut PtyPair, byte: u8) -> Option<u32> {
    // ICRNL: \r -> \n on input
    let byte = if pair.termios.c_iflag & ICRNL != 0 && byte == b'\r' {
        b'\n'
    } else { byte };

    if pair.termios.c_lflag & ICANON == 0 {
        // Raw mode: byte passes through unchanged (apps like rtop read ^C=0x03
        // themselves). No signal handling.
        pair.slave_rx.push_back(byte);
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return None;
    }

    // --- Cooked mode ---
    if pair.termios.c_lflag & ISIG != 0 && byte == pair.termios.c_cc[VINTR] {
        pair.line_buffer.clear();
        if pair.termios.c_lflag & ECHO != 0 {
            for &b in b"^C\r\n" { pair.master_out.push_back(b); }
            if let Some(w) = pair.master_waker.take() { w.wake(); }
        }
        // Wake any stdin-blocked foreground reader so its read re-polls and
        // returns EOF (the read checks foreground_pid's kill flag). The caller
        // sets that flag via the returned pid.
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return pair.foreground_pid;
    }

    if byte == pair.termios.c_cc[VERASE] {
        if pair.line_buffer.pop().is_some() && pair.termios.c_lflag & ECHO != 0 {
            for &b in b"\x08 \x08" { pair.master_out.push_back(b); }
            if let Some(w) = pair.master_waker.take() { w.wake(); }
        }
        return None;
    }

    if byte == b'\n' {
        pair.line_buffer.push(b'\n');
        for b in pair.line_buffer.drain(..) {
            pair.slave_rx.push_back(b);
        }
        if pair.termios.c_lflag & ECHO != 0 {
            for &b in b"\r\n" { pair.master_out.push_back(b); }
            if let Some(w) = pair.master_waker.take() { w.wake(); }
        }
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return None;
    }

    if byte == pair.termios.c_cc[VEOF] {
        for b in pair.line_buffer.drain(..) {
            pair.slave_rx.push_back(b);
        }
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return None;
    }

    // Regular char
    pair.line_buffer.push(byte);
    if pair.termios.c_lflag & ECHO != 0 {
        pair.master_out.push_back(byte);
        if let Some(w) = pair.master_waker.take() { w.wake(); }
    }
    None
}

/// Process one output byte heading from slave to master (shell stdout).
pub fn process_output(pair: &mut PtyPair, byte: u8) {
    if pair.termios.c_oflag & (OPOST | ONLCR) == (OPOST | ONLCR) && byte == b'\n' {
        pair.master_out.push_back(b'\r');
    }
    pair.master_out.push_back(byte);
    if let Some(w) = pair.master_waker.take() { w.wake(); }
}
