//! PtyPair: master-slave bytestream pair + line buffer + termios + wakers.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::task::Waker;
use super::termios::Termios;

pub struct PtyPair {
    pub master_in:    VecDeque<u8>,
    pub master_out:   VecDeque<u8>,
    pub slave_rx:     VecDeque<u8>,
    pub slave_tx:     VecDeque<u8>,
    pub line_buffer:  Vec<u8>,
    pub termios:      Termios,
    pub master_waker: Option<Waker>,
    pub slave_waker:  Option<Waker>,
    /// PID of the app running in the foreground on this pair (set by the exec
    /// worker while a child runs, cleared when it exits). `^C` (VINTR) in cooked
    /// mode cooperatively kills this pid; a slave read returns EOF once its kill
    /// is pending, so a stdin-blocked app unblocks and exits. `None` = at the
    /// shell prompt, where `^C` only clears the current line.
    pub foreground_pid: Option<u32>,
}

impl PtyPair {
    pub const fn new() -> Self {
        Self {
            master_in:    VecDeque::new(),
            master_out:   VecDeque::new(),
            slave_rx:     VecDeque::new(),
            slave_tx:     VecDeque::new(),
            line_buffer:  Vec::new(),
            termios:      Termios::default_cooked(),
            master_waker: None,
            slave_waker:  None,
            foreground_pid: None,
        }
    }
}
