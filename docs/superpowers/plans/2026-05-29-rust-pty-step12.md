# PTY Step 12 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Multi-PTY pool (4 pair) + line discipline + termios host fns + shell.wasm raw-mode line editor with arrows/history/tab/Ctrl-keys.

**Architecture:** `kernel/src/pty/` modulo (pair + ldisc + termios). Slave esposti `/dev/pts/0..3` come VFS entries via `TmpKind::PtySlave(idx)`. Pair 0 wired al boot: keyboard ISR → pair 0 master input → ldisc → slave RX → shell.wasm fd_read; shell.wasm fd_write → slave TX → ldisc → master output → console_drain_task → fb CONSOLE. termios via WASIX-style host fns sotto module "ruos".

**Tech Stack:** Rust no_std, wasi-libc termios layout, custom ANSI escape emission per arrow keys. Niente nuovo crate dep.

**Spec:** `docs/superpowers/specs/2026-05-29-rust-pty-step12-design.md`

**Branch:** `feature/step-12-pty` (already created)

**Build host:** WSL Ubuntu, all commands via:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
```

**Changelog:** spec=92, plan=93, T1=94, T2=95, T3=96, T4=97, T5=98.

**Git identity (mandatory):**
```
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit ...
```
Co-author trailer at end of every commit:
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

**Smoke contract**: `make run-test` HELLO **unchanged** `shell: init.sh complete` through all 5 tasks. PTY is plumbing — shell still completes init.sh. Manual VBox testing validates line editor.

**Established patterns (Step 10/10.5/11):**
- wasmi 1.0.9: `Error::host(SuspendReason::*)`, `Error::i32_exit(-1)`.
- Fiber dispatch in `wasm/fiber.rs::dispatch`.
- VFS file impls live in `kernel/src/vfs/devices.rs` (extend with PtySlaveFile).
- tmpfs has `TmpKind` enum with kinds (extend with `PtySlave(usize)`).
- Boot phases in `kernel/src/boot/phases/` — fs.rs adds pty init.
- structured logger: `binfo!("pty", "...")`.

---

## File Structure

**New:**
- `kernel/src/pty/mod.rs` — module root + static `PAIRS` + `init()` + push/read helpers
- `kernel/src/pty/pair.rs` — `PtyPair` struct (queues + waker + line_buffer + termios)
- `kernel/src/pty/ldisc.rs` — `process_input` (cooked + raw) + `process_output`
- `kernel/src/pty/termios.rs` — `Termios` struct + flag constants

**Modified:**
- `kernel/src/main.rs` — `mod pty;`
- `kernel/src/vfs/file.rs` — `FileImpl::PtySlave(PtySlaveFile)` variant
- `kernel/src/vfs/devices.rs` — add `PtySlaveFile` struct + impl File
- `kernel/src/vfs/tmpfs.rs` — `TmpKind::PtySlave(usize)` + open dispatch
- `kernel/src/vfs/mod.rs` — `init()` mounts `/dev/pts/0..3`
- `kernel/src/keyboard/mod.rs` — 0xE0 latch + ANSI emit + → PTY 0
- `kernel/src/wasm/state.rs` — `RuntimeState::new()` opens `/dev/pts/0` thrice for FD 0/1/2
- `kernel/src/wasm/suspend.rs` — drop `KbdReadChar` variant
- `kernel/src/wasm/fiber.rs` — drop `KbdReadChar` dispatch arm
- `kernel/src/wasm/host/fd.rs` — drop `Stdin` early branch in fd_read
- `kernel/src/wasm/host/mod.rs` — link `term` module
- `kernel/src/wasm/host/term.rs` — new file, tcgetattr/tcsetattr/isatty
- `kernel/src/executor/mod.rs` — spawn `console_drain_task`
- `kernel/src/boot/phases/fs.rs` — `pty::init()` after vfs::init
- `user/shell/src/main.rs` — raw mode + line editor

**Deleted:**
- `kernel/src/keyboard/queue.rs`
- `FdEntry::Stdin` variant from state.rs

---

## Task 1: PTY core (pair + ldisc + termios)

**Files:**
- Create: `kernel/src/pty/mod.rs`
- Create: `kernel/src/pty/pair.rs`
- Create: `kernel/src/pty/ldisc.rs`
- Create: `kernel/src/pty/termios.rs`
- Modify: `kernel/src/main.rs` — `mod pty;`

**Smoke contract:** unchanged `shell: init.sh complete`. T1 adds infra; nothing wired yet so behavior unchanged.

- [ ] **Step 1.1: Create `kernel/src/pty/termios.rs`**

```rust
//! POSIX termios subset for PTY line discipline.
//!
//! 60-byte ABI matching wasi-libc's __wasi_termios_t so wasm guests
//! can read/write the struct directly via tcgetattr/tcsetattr.

pub const NCCS: usize = 32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    pub c_iflag:  u32,
    pub c_oflag:  u32,
    pub c_cflag:  u32,
    pub c_lflag:  u32,
    pub c_cc:     [u8; NCCS],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

// c_iflag bits
pub const ICRNL:  u32 = 0o0100;

// c_oflag bits
pub const OPOST:  u32 = 0o0001;
pub const ONLCR:  u32 = 0o0004;

// c_lflag bits
pub const ISIG:   u32 = 0o0001;
pub const ICANON: u32 = 0o0002;
pub const ECHO:   u32 = 0o0010;
pub const IEXTEN: u32 = 0o100000;

// c_cc indices
pub const VINTR:  usize = 0;
pub const VERASE: usize = 2;
pub const VEOF:   usize = 4;
pub const VEOL:   usize = 5;

impl Termios {
    pub const fn default_cooked() -> Self {
        let mut cc = [0u8; NCCS];
        cc[VINTR]  = 0x03;
        cc[VERASE] = 0x7F;
        cc[VEOF]   = 0x04;
        cc[VEOL]   = 0x00;
        Self {
            c_iflag:  ICRNL,
            c_oflag:  OPOST | ONLCR,
            c_cflag:  0,
            c_lflag:  ISIG | ICANON | ECHO | IEXTEN,
            c_cc:     cc,
            c_ispeed: 38400,
            c_ospeed: 38400,
        }
    }
}
```

- [ ] **Step 1.2: Create `kernel/src/pty/pair.rs`**

```rust
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
        }
    }
}
```

- [ ] **Step 1.3: Create `kernel/src/pty/ldisc.rs`**

```rust
//! Line discipline. Cooked mode (default) does echo + backspace + line
//! buffer + Ctrl-C signaling. Raw mode passes bytes through unchanged.

use super::pair::PtyPair;
use super::termios::*;

/// Process one input byte arriving at the master end (e.g. from
/// keyboard ISR). Handles termios.c_lflag modes.
pub fn process_input(pair: &mut PtyPair, byte: u8) {
    // ICRNL: \r → \n on input
    let byte = if pair.termios.c_iflag & ICRNL != 0 && byte == b'\r' {
        b'\n'
    } else { byte };

    if pair.termios.c_lflag & ICANON == 0 {
        // Raw mode
        pair.slave_rx.push_back(byte);
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return;
    }

    // ─── Cooked mode ───────────────────────────────────────────
    if pair.termios.c_lflag & ISIG != 0 && byte == pair.termios.c_cc[VINTR] {
        pair.line_buffer.clear();
        if pair.termios.c_lflag & ECHO != 0 {
            for &b in b"^C\r\n" { pair.master_out.push_back(b); }
            if let Some(w) = pair.master_waker.take() { w.wake(); }
        }
        return;
    }

    if byte == pair.termios.c_cc[VERASE] {
        if pair.line_buffer.pop().is_some() && pair.termios.c_lflag & ECHO != 0 {
            for &b in b"\x08 \x08" { pair.master_out.push_back(b); }
            if let Some(w) = pair.master_waker.take() { w.wake(); }
        }
        return;
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
        return;
    }

    if byte == pair.termios.c_cc[VEOF] {
        for b in pair.line_buffer.drain(..) {
            pair.slave_rx.push_back(b);
        }
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return;
    }

    // Regular char
    pair.line_buffer.push(byte);
    if pair.termios.c_lflag & ECHO != 0 {
        pair.master_out.push_back(byte);
        if let Some(w) = pair.master_waker.take() { w.wake(); }
    }
}

/// Process one output byte heading from slave to master (shell stdout).
pub fn process_output(pair: &mut PtyPair, byte: u8) {
    if pair.termios.c_oflag & (OPOST | ONLCR) == (OPOST | ONLCR) && byte == b'\n' {
        pair.master_out.push_back(b'\r');
    }
    pair.master_out.push_back(byte);
    if let Some(w) = pair.master_waker.take() { w.wake(); }
}
```

- [ ] **Step 1.4: Create `kernel/src/pty/mod.rs`**

```rust
//! PTY subsystem. Static pool of 4 pseudo-terminal pairs.

pub mod termios;
pub mod ldisc;
pub mod pair;

use spin::Mutex;
use pair::PtyPair;

pub const NUM_PAIRS: usize = 4;

static PAIRS: [Mutex<PtyPair>; NUM_PAIRS] = [
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
];

pub fn pair(idx: usize) -> &'static Mutex<PtyPair> {
    &PAIRS[idx]
}

/// Called from boot fs phase after vfs::init.
pub fn init() {
    crate::binfo!("pty", "{} pairs ready", NUM_PAIRS);
}

/// Push a byte into pair `idx`'s master input. Runs line discipline.
/// Safe to call from ISR context (uses without_interrupts not needed
/// since ISR already has IF=0; just lock).
pub fn master_input_push(idx: usize, byte: u8) {
    if idx >= NUM_PAIRS { return; }
    let mut g = PAIRS[idx].lock();
    ldisc::process_input(&mut g, byte);
}

/// Future-friendly read of one byte from pair `idx`'s master output.
/// Used by console_drain_task.
pub async fn master_output_read(idx: usize) -> u8 {
    use core::future::poll_fn;
    use core::task::Poll;
    poll_fn(|cx| {
        use x86_64::instructions::interrupts::without_interrupts;
        without_interrupts(|| {
            let mut g = PAIRS[idx].lock();
            match g.master_out.pop_front() {
                Some(b) => Poll::Ready(b),
                None => {
                    g.master_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
    }).await
}
```

- [ ] **Step 1.5: Add `mod pty;` in `kernel/src/main.rs`**

Add alongside the existing `mod` declarations.

- [ ] **Step 1.6: Build + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | tail -10'
```

Expected: clean build (some unused-fn warnings on new pub fns since
nobody calls them yet). Sentinel still PASS — PTY infra not wired in T1.

- [ ] **Step 1.7: Changelog + commit**

`CHANGELOG/94-26-05-29-pty-core.md`:

```markdown
# 94 — PTY core: pair + ldisc + termios (T1)

**Data:** 2026-05-29

## Cosa

- `kernel/src/pty/termios.rs`: 60-byte Termios struct + flag constants
  (ICRNL/OPOST/ONLCR/ISIG/ICANON/ECHO/IEXTEN/VINTR/VERASE/VEOF/VEOL).
- `kernel/src/pty/pair.rs`: `PtyPair` con 4 VecDeque + line_buffer +
  termios + 2 wakers.
- `kernel/src/pty/ldisc.rs`: `process_input` (cooked + raw) +
  `process_output` (OPOST/ONLCR).
- `kernel/src/pty/mod.rs`: 4 static `Mutex<PtyPair>` + `master_input_push`
  + async `master_output_read`.
- `kernel/src/main.rs`: `mod pty;`.

## Perché

Step 12 T1: infrastruttura PTY senza wiring. T2-T5 collegano tutto.

## File toccati

- kernel/src/pty/{mod,pair,ldisc,termios}.rs (nuovi)
- kernel/src/main.rs
- CHANGELOG/94-26-05-29-pty-core.md (nuovo)
```

```bash
git add kernel/src/pty/ kernel/src/main.rs CHANGELOG/94-26-05-29-pty-core.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): PTY core (pair + ldisc + termios)

Static pool of 4 PtyPair (queues + waker + line_buffer + termios).
ldisc::process_input handles cooked (echo + backspace + line buffer +
Ctrl-C signaling) and raw modes per termios.c_lflag.
ldisc::process_output converts \\n → \\r\\n when OPOST|ONLCR set.
master_input_push runs ldisc; master_output_read is async with waker.

No wiring yet — T2 mounts /dev/pts/N, T3 wires keyboard ISR + console
drain task.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: VFS integration (PtySlaveFile + /dev/pts/0..3)

**Files:**
- Modify: `kernel/src/vfs/file.rs` (FileImpl::PtySlave variant)
- Modify: `kernel/src/vfs/devices.rs` (PtySlaveFile struct + impl File)
- Modify: `kernel/src/vfs/tmpfs.rs` (TmpKind::PtySlave(usize))
- Modify: `kernel/src/vfs/mod.rs` (mount /dev/pts/0..3 in init())
- Modify: `kernel/src/boot/phases/fs.rs` (call pty::init())

**Smoke contract:** unchanged `shell: init.sh complete`. T2 adds device entries; not yet read/written by anyone.

- [ ] **Step 2.1: Add `FileImpl::PtySlave` variant**

In `kernel/src/vfs/file.rs`, extend `FileImpl`:

```rust
pub enum FileImpl {
    Tmp(TmpfsFile),
    Console(ConsoleFile),
    Null(NullFile),
    Zero(ZeroFile),
    PtySlave(PtySlaveFile),  // NEW
}
```

Update each `match` arm in `FileImpl::read`/`write`/`seek` to delegate to PtySlave. (Three new arms.)

```rust
            FileImpl::PtySlave(f) => f.read(buf).await,
            FileImpl::PtySlave(f) => f.write(buf).await,
            FileImpl::PtySlave(f) => f.seek(off, whence).await,
```

Import: `use crate::vfs::devices::PtySlaveFile;`.

- [ ] **Step 2.2: Add PtySlaveFile in `kernel/src/vfs/devices.rs`**

Append to existing devices.rs:

```rust
//! PTY slave: bridges the wasm-side `/dev/pts/<idx>` open to the
//! kernel `pty::pair(idx)` queues + line discipline.

use core::future::poll_fn;
use core::task::Poll;

pub struct PtySlaveFile {
    pub idx: usize,
}

impl File for PtySlaveFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() { return Ok(0); }
        let idx = self.idx;
        poll_fn(|cx| {
            use x86_64::instructions::interrupts::without_interrupts;
            without_interrupts(|| {
                let mut g = crate::pty::pair(idx).lock();
                let mut n = 0;
                while n < buf.len() {
                    match g.slave_rx.pop_front() {
                        Some(b) => { buf[n] = b; n += 1; }
                        None => break,
                    }
                }
                if n > 0 {
                    Poll::Ready(Ok(n))
                } else {
                    g.slave_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            })
        }).await
    }

    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() { return Ok(0); }
        let idx = self.idx;
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut g = crate::pty::pair(idx).lock();
            for &b in buf {
                crate::pty::ldisc::process_output(&mut g, b);
            }
        });
        Ok(buf.len())
    }

    async fn seek(&mut self, _: i64, _: Whence) -> Result<u64, VfsError> {
        Err(VfsError::NotPermitted)
    }
}
```

- [ ] **Step 2.3: Add `TmpKind::PtySlave(usize)` in `kernel/src/vfs/tmpfs.rs`**

```rust
#[derive(Copy, Clone)]
pub enum TmpKind {
    Dir, Reg,
    DevConsole, DevNull, DevZero,
    PtySlave(usize),    // NEW
}
```

In `Tmpfs::open` (the existing match on kind after walk), add:

```rust
            TmpKind::PtySlave(idx) => Ok(FileImpl::PtySlave(
                crate::vfs::devices::PtySlaveFile { idx }
            )),
```

Note: `TmpKind::PtySlave(usize)` makes `TmpKind` non-Copy because of
the inner usize. It already derives Copy — wait, usize is Copy, so
`TmpKind::PtySlave(usize)` is still Copy. ✓

- [ ] **Step 2.4: Mount `/dev/pts/0..3` in `kernel/src/vfs/mod.rs::init`**

After the existing `/dev/zero` insert, add:

```rust
    fs.mkdir(&["dev", "pts"])?;
    for i in 0..crate::pty::NUM_PAIRS {
        // alloc-format the index to a static-lived String
        let name = alloc::format!("{}", i);
        // Need to pass &[&str] but `name` is owned. Workaround: leak
        // a small static-lifetime str (NUM_PAIRS=4 → 4 leaks of "0".."3",
        // 4 bytes total). Or use a const array of literal strs.
        let name_static: &'static str = alloc::boxed::Box::leak(name.into_boxed_str());
        fs.insert_inode(&["dev", "pts", name_static], TmpInode {
            kind: TmpKind::PtySlave(i),
            children: alloc::collections::BTreeMap::new(),
            content: alloc::vec::Vec::new(),
        })?;
    }
```

Simpler if you prefer: use a const slice `["0","1","2","3"]`:

```rust
    fs.mkdir(&["dev", "pts"])?;
    const NAMES: [&str; crate::pty::NUM_PAIRS] = ["0", "1", "2", "3"];
    for (i, name) in NAMES.iter().enumerate() {
        fs.insert_inode(&["dev", "pts", name], TmpInode {
            kind: TmpKind::PtySlave(i),
            children: alloc::collections::BTreeMap::new(),
            content: alloc::vec::Vec::new(),
        })?;
    }
```

Use the second form if `NUM_PAIRS` is `pub const` accessible.

- [ ] **Step 2.5: Call `pty::init` in `kernel/src/boot/phases/fs.rs`**

After `vfs::init` succeeds:

```rust
    crate::pty::init();
```

Add inside the existing `fs::init` between the vfs mount log and modules mount.

- [ ] **Step 2.6: Build + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | grep -E "pty|TEST_PASS|shell:" | tail -10'
```

Expected: boot log includes `INFO pty  4 pairs ready`. Sentinel still PASS.

- [ ] **Step 2.7: Changelog + commit**

`CHANGELOG/95-26-05-29-pty-vfs.md` (template per plan style).

```bash
git add kernel/src/vfs/ kernel/src/boot/phases/fs.rs CHANGELOG/95-26-05-29-pty-vfs.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): PTY VFS integration (/dev/pts/0..3 + PtySlaveFile)

FileImpl gains PtySlave(PtySlaveFile { idx }) variant. PtySlaveFile
async read drains pair[idx].slave_rx (registers slave_waker on empty);
write loops process_output(pair, b) for each byte.

TmpKind::PtySlave(usize) routes tmpfs::open to FileImpl::PtySlave.
vfs::init() mkdirs /dev/pts and inserts 4 slave inodes (/dev/pts/0..3).
boot::phases::fs::init calls pty::init() so the PAIRS array is
explicitly logged.

No reader/writer yet — T3 wires keyboard → master input + console
drain task → master output, and shell.wasm boot FDs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Keyboard ISR + console_drain + boot FD setup (BIG)

**Files:**
- Modify: `kernel/src/keyboard/mod.rs` — 0xE0 latch + ANSI emit + push to PTY 0
- Delete: `kernel/src/keyboard/queue.rs`
- Modify: `kernel/src/executor/mod.rs` — add `console_drain_task` + spawn
- Modify: `kernel/src/wasm/state.rs` — RuntimeState::new opens /dev/pts/0 for FD 0/1/2
- Modify: `kernel/src/wasm/suspend.rs` — drop `KbdReadChar` variant
- Modify: `kernel/src/wasm/fiber.rs` — drop `KbdReadChar` dispatch arm
- Modify: `kernel/src/wasm/host/fd.rs` — drop FdEntry::Stdin early-trap branch
- Modify: `kernel/src/wasm/state.rs` — drop `FdEntry::Stdin` variant (if still present)

**Smoke contract:** sentinel preserved + shell.wasm reads /dev/pts/0 instead of legacy Stdin. Init.sh runs (cooked mode default, shell prints output via PTY).

- [ ] **Step 3.1: Update `kernel/src/keyboard/mod.rs`**

Replace contents. Keep `SCANCODE_MAP` from Step 9. Add EXTENDED latch + extended_to_ansi:

```rust
//! PS/2 keyboard ISR. Scancode → ASCII (regular) or ANSI escape
//! sequence (extended), pushed into PTY 0 master input.

use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::instructions::port::Port;
use x86_64::structures::idt::InterruptStackFrame;
use crate::{apic, idt};
use crate::acpi_init::IrqOverride;

// ── Existing SCANCODE_MAP (Set 1 → ASCII) — keep as in Step 9 ──
static SCANCODE_MAP: [u8; 89] = [ /* …unchanged… */ ];

static EXTENDED: AtomicBool = AtomicBool::new(false);

fn extended_to_ansi(scancode: u8) -> Option<&'static [u8]> {
    match scancode {
        0x48 => Some(b"\x1b[A"),
        0x50 => Some(b"\x1b[B"),
        0x4D => Some(b"\x1b[C"),
        0x4B => Some(b"\x1b[D"),
        0x47 => Some(b"\x1b[H"),
        0x4F => Some(b"\x1b[F"),
        0x53 => Some(b"\x1b[3~"),
        _ => None,
    }
}

pub extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    let mut data: Port<u8> = Port::new(0x60);
    let scancode = unsafe { data.read() };

    if scancode == 0xE0 {
        EXTENDED.store(true, Ordering::SeqCst);
        apic::lapic::eoi();
        return;
    }

    if EXTENDED.swap(false, Ordering::SeqCst) {
        if scancode < 0x80 {
            if let Some(seq) = extended_to_ansi(scancode) {
                for &b in seq {
                    crate::pty::master_input_push(0, b);
                }
            }
        }
        apic::lapic::eoi();
        return;
    }

    if scancode < 0x80 {
        let idx = scancode as usize;
        if idx < SCANCODE_MAP.len() {
            let ch = SCANCODE_MAP[idx];
            if ch != 0 {
                crate::pty::master_input_push(0, ch);
            }
        }
    }

    apic::lapic::eoi();
}

pub fn init(overrides: &[IrqOverride]) {
    apic::ioapic::redirect(1, idt::VEC_KEYBOARD, overrides);
}
```

Copy the existing `SCANCODE_MAP` contents from the current keyboard/mod.rs.

- [ ] **Step 3.2: Delete `kernel/src/keyboard/queue.rs`**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && rm kernel/src/keyboard/queue.rs'
```

Remove `pub mod queue;` from `keyboard/mod.rs`.

- [ ] **Step 3.3: Add `console_drain_task` in `kernel/src/executor/mod.rs`**

Add task definition:

```rust
#[embassy_executor::task]
async fn console_drain_task() {
    loop {
        let b = crate::pty::master_output_read(0).await;
        x86_64::instructions::interrupts::without_interrupts(|| {
            use core::fmt::Write;
            let mut c = crate::console::CONSOLE.lock();
            let buf = [b];
            let s = core::str::from_utf8(&buf).unwrap_or("?");
            let _ = c.write_str(s);
        });
    }
}
```

In `run()`, spawn it:

```rust
    spawner.spawn(console_drain_task()).unwrap();
```

(Bump pool_size if needed.)

- [ ] **Step 3.4: Update `RuntimeState::new` to open /dev/pts/0**

In `kernel/src/wasm/state.rs`:

```rust
impl RuntimeState {
    pub fn new() -> Self {
        let mut fds: Vec<Option<FdEntry>> = (0..16).map(|_| None).collect();
        use crate::vfs;
        // Open /dev/pts/0 thrice for FD 0/1/2. tmpfs returns ready in
        // single poll, so block_on (Step 7 noop-waker) works fine here.
        for slot in 0..3 {
            match vfs::block_on(vfs::open(
                "/dev/pts/0",
                vfs::OpenFlags::READ | vfs::OpenFlags::WRITE,
            )) {
                Ok(fd) => { fds[slot] = Some(FdEntry::Vfs(fd)); }
                Err(_) => {} // leave None; wasm may fail to read/write
            }
        }
        Self { fds, args: Vec::new(), env: Vec::new(), exit_code: AtomicI32::new(0) }
    }
}
```

Drop `FdEntry::Stdin` variant from the enum (no longer needed):

```rust
pub enum FdEntry {
    StdoutConsole,   // still used? actually — if nothing references it, drop too
    Vfs(crate::vfs::Fd),
    Socket(usize),
}
```

Actually keep `StdoutConsole` for backwards compat (some debug paths
may still use it) — drop only `Stdin`. Implementer decides.

- [ ] **Step 3.5: Drop KbdReadChar everywhere**

In `kernel/src/wasm/suspend.rs`, remove `SuspendReason::KbdReadChar { ... }` variant.

In `kernel/src/wasm/fiber.rs::dispatch`, remove the arm that matches `SuspendReason::KbdReadChar`.

In `kernel/src/wasm/host/fd.rs::fd_read`, remove the early branch:
```rust
            FdEntry::Stdin => { /* trap KbdReadChar */ }
```

After T3 the Vfs(fd) arm handles all reads, including the one that
backs /dev/pts/0.

- [ ] **Step 3.6: Build + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | tail -25'
```

Expected: boot log includes `INFO pty  4 pairs ready`. Sentinel
`shell: init.sh complete` still PASS.

Critical: shell.wasm output (`echo ruos boot OK`, `shell: init.sh complete`,
`ruos shell ready...`) now flows through PTY 0 → console_drain_task → fb.
If anything is broken in that chain, output won't appear and the sentinel
won't fire.

Iterate up to 5 build/test cycles. Common pitfalls:
- `RuntimeState::new()` called before vfs::init → /dev/pts/0 doesn't exist
  yet. Verify: state.rs is only used inside Fiber::new which is called
  via wasm_task, spawned only inside `executor::run` which is the last
  thing in the boot path. By that time vfs::init is done.
- Echo loop: shell.wasm writes "X" → ldisc::process_output appends "X"
  to master_out → drain task reads → fb writes "X". For now no shell-side
  termios change, so ldisc cooked default applies. echo on master input
  echoes terminal input keystrokes (cooked).
- `master_input_push` lock contention: spin::Mutex inside ISR is OK
  single-CPU. Each ISR runs to completion before next IRQ.

- [ ] **Step 3.7: Changelog + commit**

`CHANGELOG/96-26-05-29-pty-wire.md` (template).

```bash
git add kernel/src/keyboard/ kernel/src/executor/ kernel/src/wasm/ CHANGELOG/96-26-05-29-pty-wire.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): wire keyboard ISR + console_drain + RuntimeState stdio to PTY 0

Keyboard ISR replaces queue.push with pty::master_input_push(0, b).
0xE0 prefix latches via AtomicBool EXTENDED; next non-release scancode
emits ANSI escape sequence (\\x1b[A/B/C/D for arrows, [H/F for home/end,
[3~ for delete) — closes Step 9 F3 followup. Legacy keyboard/queue.rs
removed.

console_drain_task spawned by executor::run: loops master_output_read(0)
and writes each byte to CONSOLE.lock() under without_interrupts.

RuntimeState::new() opens /dev/pts/0 thrice (FD 0/1/2) via vfs::block_on
+ vfs::open. Drops FdEntry::Stdin variant + SuspendReason::KbdReadChar +
fiber dispatch arm + fd_read Stdin early-trap branch. All shell I/O now
flows through the VFS PtySlaveFile path → PTY ldisc → console drain.

Closes Step 10.5 F4 followup (keyboard queue single-Waker race retired
with the queue).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: WASIX termios host fns

**Files:**
- Create: `kernel/src/wasm/host/term.rs`
- Modify: `kernel/src/wasm/host/mod.rs` (link term)

**Smoke contract:** unchanged. Adds host fn surface; no caller yet (T5 shell uses them).

- [ ] **Step 4.1: Create `kernel/src/wasm/host/term.rs`**

```rust
//! WASIX-style termios host fns under module "ruos".

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::{RuntimeState, FdEntry};
use crate::wasm::host::lifecycle::wasm_memory;

pub fn tcgetattr(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    termios_ptr: i32,
) -> Result<i32, Error> {
    let pty_idx = match fd_to_pty(&caller, fd) {
        Some(idx) => idx,
        None => return Ok(25), // ENOTTY
    };
    let pair = crate::pty::pair(pty_idx);
    let g = pair.lock();
    let t = g.termios;
    drop(g);
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &t as *const _ as *const u8,
            core::mem::size_of::<crate::pty::termios::Termios>(),
        )
    };
    let mem = wasm_memory(&caller)?;
    mem.write(&caller, termios_ptr as usize, bytes)
        .map_err(|_| Error::i32_exit(-1))?;
    Ok(0)
}

pub fn tcsetattr(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    _action: i32,
    termios_ptr: i32,
) -> Result<i32, Error> {
    let pty_idx = match fd_to_pty(&caller, fd) {
        Some(idx) => idx,
        None => return Ok(25),
    };
    let mut termios = crate::pty::termios::Termios::default_cooked();
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut termios as *mut _ as *mut u8,
            core::mem::size_of::<crate::pty::termios::Termios>(),
        )
    };
    let mem = wasm_memory(&caller)?;
    mem.read(&caller, termios_ptr as usize, bytes)
        .map_err(|_| Error::i32_exit(-1))?;
    crate::pty::pair(pty_idx).lock().termios = termios;
    Ok(0)
}

pub fn isatty(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
) -> Result<i32, Error> {
    Ok(if fd_to_pty(&caller, fd).is_some() { 1 } else { 0 })
}

/// Map wasm-side FD → PTY idx if and only if it backs a PtySlaveFile.
fn fd_to_pty(caller: &Caller<'_, RuntimeState>, fd: i32) -> Option<usize> {
    let entry = caller.data().fds.get(fd as usize)?.as_ref()?;
    let vfs_fd = match entry {
        FdEntry::Vfs(f) => *f,
        _ => return None,
    };
    // Peek into the global FDS table to find the FileImpl variant.
    let t = crate::vfs::fd::FDS.lock();
    let slot = t.get(vfs_fd as usize)?.as_ref()?;
    match &slot.file {
        crate::vfs::file::FileImpl::PtySlave(p) => Some(p.idx),
        _ => None,
    }
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "tcgetattr", tcgetattr)?
        .func_wrap("ruos", "tcsetattr", tcsetattr)?
        .func_wrap("ruos", "isatty", isatty)?;
    Ok(())
}
```

Note: `crate::vfs::fd::FDS` and `FdEntry { file }` fields must be
`pub(crate)`. If they're tighter, expose them.

- [ ] **Step 4.2: Link in `kernel/src/wasm/host/mod.rs`**

```rust
pub mod term;
// ... existing pub mods ...

pub fn install(linker: &mut wasmi::Linker<RuntimeState>) -> Result<(), wasmi::Error> {
    lifecycle::link(linker)?;
    fd::link(linker)?;
    path::link(linker)?;
    clock::link(linker)?;
    random::link(linker)?;
    sock::link(linker)?;
    proc::link(linker)?;
    term::link(linker)?;       // NEW
    Ok(())
}
```

- [ ] **Step 4.3: Build + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | tail -10'
```

Expected: clean build. Sentinel PASS unchanged.

- [ ] **Step 4.4: Changelog + commit**

`CHANGELOG/97-26-05-29-pty-termios-host-fns.md` (template).

```bash
git add kernel/src/wasm/host/ CHANGELOG/97-26-05-29-pty-termios-host-fns.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): WASIX termios host fns (tcgetattr/tcsetattr/isatty)

Three host fns under module 'ruos':
  tcgetattr(fd, ptr)        — write current pair termios to wasm memory
  tcsetattr(fd, action, ptr) — overwrite pair termios from wasm memory
  isatty(fd)                — 1 if fd backs a PtySlaveFile, else 0

fd_to_pty helper walks RuntimeState.fds → vfs::FDS table → FileImpl
variant to recover the PTY pair index. Returns ENOTTY (25) for non-PTY
FDs.

Action argument ignored — we always apply TCSANOW semantics (immediate).

T5 shell.wasm will use these to switch raw mode at boot.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Shell line editor (BIG, ~500 LoC user/shell/)

**Files:**
- Modify: `user/shell/src/main.rs` — raw mode + line editor

**Smoke contract:** unchanged sentinel. Manual VBox: full line editor works.

- [ ] **Step 5.1: Rewrite `user/shell/src/main.rs`**

This is a substantial rewrite. The new structure:

```rust
use std::fs;
use std::io::{Read, Write};
use std::sync::Mutex;

static CWD: Mutex<String> = Mutex::new(String::new());
static HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn exec(p_ptr: u32, p_len: u32, a_ptr: u32, a_len: u32, ec_ptr: u32) -> i32;
    fn readdir(p_ptr: u32, p_len: u32, b_ptr: u32, b_len: u32, n_ptr: u32) -> i32;
    fn tcgetattr(fd: i32, ptr: u32) -> i32;
    fn tcsetattr(fd: i32, action: i32, ptr: u32) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    iflag: u32, oflag: u32, cflag: u32, lflag: u32,
    cc: [u8; 32],
    ispeed: u32, ospeed: u32,
}
impl Termios { fn zero() -> Self { Self { iflag:0,oflag:0,cflag:0,lflag:0,cc:[0;32],ispeed:0,ospeed:0 } } }

const ICANON: u32 = 0o0002;
const ECHO:   u32 = 0o0010;
const ISIG:   u32 = 0o0001;

fn save_and_raw() -> Termios {
    let mut saved = Termios::zero();
    unsafe { tcgetattr(0, &mut saved as *mut _ as u32); }
    let mut raw = saved;
    raw.lflag &= !(ICANON | ECHO | ISIG);
    unsafe { tcsetattr(0, 0, &raw as *const _ as u32); }
    saved
}

fn restore(t: &Termios) {
    unsafe { tcsetattr(0, 0, t as *const _ as u32); }
}

fn read_byte() -> Option<u8> {
    let mut b = [0u8; 1];
    match std::io::stdin().read(&mut b) {
        Ok(1) => Some(b[0]),
        _ => None,
    }
}

fn redraw_line(prompt: &str, buf: &[u8], cursor: usize) {
    print!("\r\x1b[2K{}{}", prompt, std::str::from_utf8(buf).unwrap_or(""));
    // Move cursor: at end now; back to `cursor` position
    if cursor < buf.len() {
        print!("\x1b[{}D", buf.len() - cursor);
    }
    std::io::stdout().flush().ok();
}

fn tab_complete(prefix: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = vec!["cd".into(), "pwd".into(), "exit".into(), "help".into()];
    // List /bin/*.wasm
    let mut buf = vec![0u8; 4096];
    let mut n: u32 = 0;
    let p = "/bin";
    let errno = unsafe {
        readdir(p.as_ptr() as u32, p.len() as u32,
                buf.as_mut_ptr() as u32, buf.len() as u32,
                &mut n as *mut u32 as u32)
    };
    if errno == 0 {
        let mut o = 0;
        while o + 12 <= n as usize {
            let nlen = u16::from_le_bytes([buf[o+2], buf[o+3]]) as usize;
            o += 12;
            if o + nlen > n as usize { break; }
            if let Ok(name) = std::str::from_utf8(&buf[o..o+nlen]) {
                if name.ends_with(".wasm") {
                    out.push(name.trim_end_matches(".wasm").to_string());
                }
            }
            o += nlen;
        }
    }
    let pref = std::str::from_utf8(prefix).unwrap_or("");
    out.retain(|c| c.starts_with(pref));
    out
}

fn read_line_raw(prompt: &str) -> Option<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut cursor: usize = 0;
    let mut hist_idx: Option<usize> = None;
    let history_snapshot = HISTORY.lock().unwrap().clone();
    redraw_line(prompt, &buf, cursor);
    loop {
        let b = read_byte()?;
        match b {
            b'\n' | b'\r' => {
                println!();
                if !buf.is_empty() {
                    HISTORY.lock().unwrap().push(String::from_utf8_lossy(&buf).into_owned());
                }
                return Some(String::from_utf8_lossy(&buf).into_owned());
            }
            0x7F | 0x08 => {
                if cursor > 0 {
                    buf.remove(cursor - 1);
                    cursor -= 1;
                    redraw_line(prompt, &buf, cursor);
                }
            }
            0x01 => { cursor = 0; redraw_line(prompt, &buf, cursor); } // Ctrl-A
            0x05 => { cursor = buf.len(); redraw_line(prompt, &buf, cursor); } // Ctrl-E
            0x0C => { print!("\x1b[2J\x1b[H"); redraw_line(prompt, &buf, cursor); } // Ctrl-L
            0x03 => { println!("^C"); return Some(String::new()); } // Ctrl-C
            b'\t' => {
                let start = (0..cursor).rev().take_while(|&i| !buf[i].is_ascii_whitespace()).count();
                let token_start = cursor - start;
                let candidates = tab_complete(&buf[token_start..cursor]);
                if candidates.len() == 1 {
                    let comp = candidates[0].as_bytes();
                    let prefix_len = cursor - token_start;
                    if comp.len() > prefix_len {
                        let suffix = &comp[prefix_len..];
                        buf.splice(cursor..cursor, suffix.iter().copied());
                        cursor += suffix.len();
                        redraw_line(prompt, &buf, cursor);
                    }
                } else if candidates.len() > 1 {
                    println!();
                    for c in &candidates { print!("{}  ", c); }
                    println!();
                    redraw_line(prompt, &buf, cursor);
                }
            }
            0x1B => {
                let _ = read_byte()?; // [
                let arrow = read_byte()?;
                match arrow {
                    b'A' => {
                        // up
                        let idx = match hist_idx {
                            None => history_snapshot.len(),
                            Some(i) => i,
                        };
                        if idx > 0 {
                            hist_idx = Some(idx - 1);
                            buf.clear();
                            buf.extend_from_slice(history_snapshot[idx - 1].as_bytes());
                            cursor = buf.len();
                            redraw_line(prompt, &buf, cursor);
                        }
                    }
                    b'B' => {
                        // down
                        if let Some(i) = hist_idx {
                            let next = i + 1;
                            if next < history_snapshot.len() {
                                hist_idx = Some(next);
                                buf.clear();
                                buf.extend_from_slice(history_snapshot[next].as_bytes());
                                cursor = buf.len();
                            } else {
                                hist_idx = None;
                                buf.clear();
                                cursor = 0;
                            }
                            redraw_line(prompt, &buf, cursor);
                        }
                    }
                    b'C' => {
                        if cursor < buf.len() {
                            cursor += 1;
                            print!("\x1b[C");
                            std::io::stdout().flush().ok();
                        }
                    }
                    b'D' => {
                        if cursor > 0 {
                            cursor -= 1;
                            print!("\x1b[D");
                            std::io::stdout().flush().ok();
                        }
                    }
                    _ => {}
                }
            }
            c if c >= 0x20 => {
                buf.insert(cursor, c);
                cursor += 1;
                redraw_line(prompt, &buf, cursor);
            }
            _ => {}
        }
    }
}

fn main() {
    *CWD.lock().unwrap() = "/".to_string();

    // init.sh runs in cooked mode (default termios).
    if let Ok(script) = fs::read_to_string("/etc/init.sh") {
        for line in script.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            run_command(line);
        }
        println!("shell: init.sh complete");
    } else {
        println!("shell: /etc/init.sh not found");
    }

    std::thread::sleep(std::time::Duration::from_millis(1000));
    print!("\x1b[2J\x1b[H");
    std::io::stdout().flush().ok();
    println!("\x1b[1;32mruos shell ready. type 'help' for builtins.\x1b[0m");

    let saved = save_and_raw();
    loop {
        let cwd = CWD.lock().unwrap().clone();
        let prompt = format!("ruos:{}$ ", cwd);
        match read_line_raw(&prompt) {
            Some(line) => {
                let trimmed = line.trim();
                if trimmed == "exit" { break; }
                if trimmed.is_empty() { continue; }
                run_command(trimmed);
            }
            None => break,
        }
    }
    restore(&saved);
}

fn run_command(line: &str) { /* … unchanged from Step 11 … */ }
fn builtin_cd(argv: &[&str]) { /* … unchanged … */ }
fn builtin_pwd() { /* … unchanged … */ }
fn builtin_help() { /* … unchanged … */ }
fn exec_external(cmd: &str, argv: &[&str]) -> i32 { /* … unchanged … */ }
fn try_exec(path: &str, argv: &[&str]) -> Option<i32> { /* … unchanged … */ }
```

Copy the unchanged helpers from the current Step 11 shell. Add the new
`save_and_raw`/`restore`/`read_byte`/`redraw_line`/`tab_complete`/`read_line_raw`.

The old `read_line` and `print_prompt` are no longer used in interactive
mode (replaced by `read_line_raw` + inline prompt).

- [ ] **Step 5.2: Rebuild user wasm**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make user-wasm 2>&1 | tail -5'
```

- [ ] **Step 5.3: Build kernel + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | tail -15'
```

Sentinel `shell: init.sh complete` still PASS — init.sh runs in cooked
mode before raw mode kicks in.

- [ ] **Step 5.4: Manual VBox smoke (cannot be done by subagent)**

If you're a non-interactive subagent, report this step as requiring
human verification. Skip and proceed to commit; flag explicitly in
the changelog.

If a human is driving: rebuild ISO (`make iso`), mount in VBox, boot.
After "ruos shell ready" prompt:
- type `helxx[backspace][backspace]lp` → see correction + `help`
- Tab after typing `l` → completes to `ls`
- Press ↑ → recalls "help"
- Press ←/→ → cursor moves
- Ctrl-A → cursor to start
- Ctrl-E → cursor to end
- Ctrl-L → screen clears, prompt redrawn
- Ctrl-C → cancels current line
- `ls /` → runs ls.wasm

- [ ] **Step 5.5: Changelog + commit**

`CHANGELOG/98-26-05-29-pty-shell-line-editor.md`:

```markdown
# 98 — Shell line editor (T5)

**Data:** 2026-05-29

## Cosa

- `user/shell/src/main.rs` riscritta: dopo `init.sh complete` + clear
  screen, shell entra in raw mode via `tcsetattr(0, 0, raw)` con
  `ICANON|ECHO|ISIG` disabled.
- Line editor con: arrows ↑↓ history navigation, ←→ cursor, Tab
  completion contro builtin + /bin/*.wasm via ruos_readdir,
  Backspace, Ctrl-A (line start), Ctrl-E (line end), Ctrl-L (clear
  screen), Ctrl-C (cancel current line).
- ANSI escape decoding (`\x1b[A`/`B`/`C`/`D`) per arrow keys.
- `HISTORY: Mutex<Vec<String>>` in-memory (history persistent
  rimandata a Step 12.5).

## Perché

Quinto e ultimo task Step 12. Sblocca shell user-friendly.

## File toccati

- user/shell/src/main.rs
- user-bin/shell.wasm (rigenerato)
- CHANGELOG/98-26-05-29-pty-shell-line-editor.md (nuovo)
```

```bash
git add user/shell/src/main.rs user-bin/shell.wasm CHANGELOG/98-26-05-29-pty-shell-line-editor.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): shell raw-mode line editor (arrows + history + tab)

shell.wasm: after init.sh + 1s + clear screen, save current termios,
switch to raw mode (ICANON|ECHO|ISIG off), enter line editor loop.
Restores termios on exit.

Line editor handles: \\n/\\r (commit + history append), 0x7F/0x08
(backspace), 0x01 (Ctrl-A line start), 0x05 (Ctrl-E line end), 0x0C
(Ctrl-L clear), 0x03 (Ctrl-C discard), \\t (tab completion against
builtins + /bin/*.wasm via ruos_readdir), \\x1b[A/B (up/down history),
\\x1b[C/D (cursor right/left).

Step 12 complete: full PTY + line discipline + termios + shell line
editor. Manual VBox smoke needed (subagent cannot drive interactive
QEMU).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-review (controller)

**Spec coverage:**
| Spec requirement | Task |
|---|---|
| `PtyPair` struct + queues + waker + termios | T1 |
| `Termios` 60-byte ABI | T1 |
| `ldisc::process_input` cooked + raw + ICRNL | T1 |
| `ldisc::process_output` OPOST/ONLCR | T1 |
| 4 static pairs + `master_input_push` + async `master_output_read` | T1 |
| `/dev/pts/0..3` mount + PtySlaveFile | T2 |
| Keyboard ISR → PTY 0 + 0xE0 latch + ANSI emit | T3 |
| `console_drain_task` | T3 |
| RuntimeState FD 0/1/2 boot init | T3 |
| Drop legacy keyboard queue + Stdin + KbdReadChar | T3 |
| `tcgetattr` / `tcsetattr` / `isatty` host fns | T4 |
| Shell line editor (arrows + history + tab + Ctrl-keys) | T5 |

**Type/API consistency**: `Termios` defined once in T1 (kernel-side),
mirrored in T5 (shell-side). 60-byte layout must match exactly.

**Open risks:**
- `vfs::block_on(vfs::open(...))` inside `RuntimeState::new()` may
  fail if vfs::init hasn't run yet. Verify call order.
- `FDS.lock()` access in `term::fd_to_pty` — `FDS` may be `pub(crate)`;
  if private, expose.
- Multi-iov fd_write/fd_read on PTY slave currently EINVAL (Step 10
  carry-over). shell.wasm prints `print!` which uses 1-iov, OK.

---

## After all tasks complete

1. `make build` clean.
2. `make run-test` PASS (sentinel `shell: init.sh complete`).
3. `make test-boot` PASS (boot-checks).
4. Manual VBox smoke (REQUIRED): full line editor works.
5. Final whole-implementation review.
6. Non-blocking findings → `docs/followups/step-12.md`.
7. Merge `feature/step-12-pty` → `main` no-ff, push, delete branch.
