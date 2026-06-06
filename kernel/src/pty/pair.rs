//! PtyPair: master output stream + line buffer + termios + master waker.
//!
//! Owner-local state only: `master_out` (slave→master, written by the owner via
//! `process_output` — app stdout is routed to the owner over the bus, echo is
//! owner-local), `line_buffer`, `termios`, `master_waker`. The slave-input path
//! (`slave_rx`) moved to a per-pair lock-free SPSC ring (`super::SLAVE_RX`); the
//! slave consumer waker moved to `super::SLAVE_WAKER`; `foreground_pid` moved to
//! the `super::FOREGROUND` atomic — so an app core never locks the pair.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::task::Waker;
use super::termios::Termios;

pub struct PtyPair {
    pub master_out:   VecDeque<u8>,
    pub line_buffer:  Vec<u8>,
    pub termios:      Termios,
    pub master_waker: Option<Waker>,
}

impl PtyPair {
    pub const fn new() -> Self {
        Self {
            master_out:   VecDeque::new(),
            line_buffer:  Vec::new(),
            termios:      Termios::default_cooked(),
            master_waker: None,
        }
    }
}
