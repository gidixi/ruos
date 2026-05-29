# Step 12 — PTY (pseudo-terminal + line discipline)

**Data:** 2026-05-29
**Roadmap step:** 12
**Stato:** spec approvata, da implementare

## Contesto

Step 11 shell.wasm letta keyboard direct (FD 0 → SuspendReason::KbdReadChar
→ keyboard::queue::read_char). No line discipline, no echo, no editing, no
history, no tab. Output via FdEntry::StdoutConsole → CONSOLE.lock.

Roadmap step 12 originale: PTY pair, line discipline. La shell gira sopra
PTY come bash sotto xterm. Stessa astrazione che userà SSH (Step 15).

## Obiettivo

Subsystem PTY completo, multi-pair, line discipline POSIX-style, termios
host fns minimal, shell.wasm con line editor full-featured (frecce, history,
tab, Ctrl-A/E/L/C, backspace correct).

## Decisioni strategiche (brainstorm)

1. **Multi-PTY pool** statico di 4 pair pre-allocati al boot. Slave entries
   esposti come `/dev/pts/0..3`. Niente `/dev/ptmx` allocator dinamico per
   Step 12 (rimandato a Step 15 SSH quando arriverà multi-tenancy reale).
   Pair 0 wired al boot per shell locale. Pair 1-3 disponibili per SSH/futuro.
2. **Master access**: kernel-internal (`pty::master_read`/`master_write` via
   pair index). Non esposto via VFS. SSH (Step 15) deciderà se promuovere
   master a `/dev/ptmx`.
3. **Termios subset**: `c_iflag` (ICRNL), `c_oflag` (OPOST/ONLCR),
   `c_lflag` (ICANON, ECHO, ISIG, IEXTEN), `c_cc[NCCS]` (VEOF, VEOL,
   VERASE, VINTR). `c_cflag` ignored (no baud). 60-byte struct layout
   matched to wasi-libc.
4. **Line editor lato shell.wasm** (non kernel): shell setta raw mode via
   `tcsetattr`, gestisce arrow/history/tab/Ctrl-keys in user space. Kernel
   ldisc fa solo cooked-mode default (echo + backspace + line buffer +
   newline flush + Ctrl-C signal) per programmi che non setterano raw.
5. **Drop legacy paths**: `keyboard::queue` module + `FdEntry::Stdin` +
   `SuspendReason::KbdReadChar` retired. Tutto stdin va via VFS
   (`/dev/pts/0` apertura → `FdEntry::Vfs(fd)`).
6. **0xE0 latching** (closes Step 9 F3): keyboard ISR latcha extended
   prefix, decode tabella estesa, emette ANSI escape sequences
   (`\x1b[A` up / `[B` down / `[C` right / `[D` left).

## Architettura

```
keyboard ISR              shell.wasm
   │                          │
   ▼                          ▼ fd_read(0)
pty[0].master_in              SuspendReason::VfsRead
   │                          │
   ▼ ldisc::process_input     ▼ fiber dispatch
   ├──► echo to master_out    │
   └──► buffer / flush \n     ▼
            │              vfs::read(slave_fd, buf).await
            ▼                  │
       pty[0].slave_in         ▼
            │              PtySlaveFile::read
            └─── waker ───►   wait on slave_rx queue
                              │
                              ▼ on flush, slave_waker.wake()
                              return Ready(bytes)

shell.wasm                console_drain_task
   │ fd_write(1)                │ loop
   ▼                            │
SuspendReason::VfsWrite         ▼
   │                       pty[0].master_read().await
   ▼ fiber dispatch             │
vfs::write(slave_fd, bytes).await
   │                            ▼
   ▼                       drain to CONSOLE.lock().write_str
PtySlaveFile::write
   │
   ▼ ldisc::process_output (\n → \r\n if OPOST)
pty[0].master_out
   │
   └─── waker ───► console_drain_task wakes
```

## Componenti

### `kernel/src/pty/mod.rs`

```rust
pub mod termios;
pub mod ldisc;
pub mod pair;
pub mod allocator;

use pair::PtyPair;
use spin::Mutex;

pub const NUM_PAIRS: usize = 4;

static PAIRS: [Mutex<PtyPair>; NUM_PAIRS] = [
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
];

pub fn pair(idx: usize) -> &'static Mutex<PtyPair> { &PAIRS[idx] }

/// Push a byte from the terminal-side source (keyboard ISR) into pair
/// `idx`'s master input. Triggers line discipline processing.
pub fn master_input_push(idx: usize, byte: u8);

/// Async read from pair `idx`'s master output queue. Used by the
/// console_drain_task to feed bytes onto the fb console.
pub async fn master_output_read(idx: usize) -> u8;

pub fn init();   // pre-allocate; called from boot fs phase after vfs::init
```

### `kernel/src/pty/pair.rs`

```rust
use alloc::collections::VecDeque;
use core::task::Waker;
use super::termios::Termios;

pub struct PtyPair {
    pub master_in: VecDeque<u8>,        // raw input from keyboard ISR
    pub master_out: VecDeque<u8>,       // post-ldisc output → console drain
    pub slave_rx: VecDeque<u8>,         // input ready for shell.wasm fd_read
    pub slave_tx: VecDeque<u8>,         // raw output from shell.wasm fd_write
    pub line_buffer: alloc::vec::Vec<u8>, // cooked-mode accumulator
    pub termios: Termios,
    pub master_waker: Option<Waker>,
    pub slave_waker: Option<Waker>,
}

impl PtyPair {
    pub const fn new() -> Self { /* ... */ }
}
```

VecDeque for queues — adequate for Step 12 throughput. Bounded later if needed.

### `kernel/src/pty/termios.rs`

```rust
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
        cc[VINTR]  = 0x03;  // Ctrl-C
        cc[VERASE] = 0x7F;  // DEL
        cc[VEOF]   = 0x04;  // Ctrl-D
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

Layout matches wasi-libc's `__wasi_termios_t` — verify alignment with
wasm-objdump per shell.wasm imports if struct doesn't load cleanly.

### `kernel/src/pty/ldisc.rs`

```rust
use super::pair::PtyPair;
use super::termios::*;

/// Called from `master_input_push` after the byte is queued. Processes
/// according to current termios.c_lflag.
pub fn process_input(pair: &mut PtyPair, byte: u8) {
    // ICRNL: \r → \n on input
    let byte = if pair.termios.c_iflag & ICRNL != 0 && byte == b'\r' {
        b'\n'
    } else { byte };

    if pair.termios.c_lflag & ICANON == 0 {
        // Raw mode: push directly to slave_rx
        pair.slave_rx.push_back(byte);
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return;
    }

    // Cooked mode
    if pair.termios.c_lflag & ISIG != 0 && byte == pair.termios.c_cc[VINTR] {
        // Ctrl-C: discard line buffer, echo ^C\n
        pair.line_buffer.clear();
        if pair.termios.c_lflag & ECHO != 0 {
            pair.master_out.extend_from_slice(b"^C\r\n");
            if let Some(w) = pair.master_waker.take() { w.wake(); }
        }
        return;
    }

    if byte == pair.termios.c_cc[VERASE] {
        // Backspace: pop buffer + echo \b \b
        if pair.line_buffer.pop().is_some() && pair.termios.c_lflag & ECHO != 0 {
            pair.master_out.extend_from_slice(b"\x08 \x08");
            if let Some(w) = pair.master_waker.take() { w.wake(); }
        }
        return;
    }

    if byte == b'\n' {
        pair.line_buffer.push(b'\n');
        // Flush full line to slave_rx
        for b in pair.line_buffer.drain(..) {
            pair.slave_rx.push_back(b);
        }
        if pair.termios.c_lflag & ECHO != 0 {
            pair.master_out.extend_from_slice(b"\r\n");
            if let Some(w) = pair.master_waker.take() { w.wake(); }
        }
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return;
    }

    if byte == pair.termios.c_cc[VEOF] {
        // Ctrl-D: flush whatever's in buffer (no \n)
        for b in pair.line_buffer.drain(..) {
            pair.slave_rx.push_back(b);
        }
        if let Some(w) = pair.slave_waker.take() { w.wake(); }
        return;
    }

    // Regular char: buffer + echo
    pair.line_buffer.push(byte);
    if pair.termios.c_lflag & ECHO != 0 {
        pair.master_out.push_back(byte);
        if let Some(w) = pair.master_waker.take() { w.wake(); }
    }
}

/// Called from `PtySlaveFile::write` for each byte the slave (shell) writes.
pub fn process_output(pair: &mut PtyPair, byte: u8) {
    if pair.termios.c_oflag & (OPOST | ONLCR) == (OPOST | ONLCR) && byte == b'\n' {
        pair.master_out.push_back(b'\r');
    }
    pair.master_out.push_back(byte);
    if let Some(w) = pair.master_waker.take() { w.wake(); }
}
```

### `kernel/src/vfs/devices/pty.rs` (new submodule)

```rust
//! PTY slave file backing for /dev/pts/<idx>.

use crate::vfs::error::VfsError;
use crate::vfs::file::{File, Whence};

pub struct PtySlaveFile {
    pub idx: usize,
}

impl File for PtySlaveFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        // Future: lock pair, check slave_rx; if empty, register waker.
        super::pty_read_slave(self.idx, buf).await
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        super::pty_write_slave(self.idx, buf).await
    }
    async fn seek(&mut self, _: i64, _: Whence) -> Result<u64, VfsError> {
        Err(VfsError::NotPermitted)
    }
}
```

`pty_read_slave` / `pty_write_slave` are async free fns living in
`vfs/devices/mod.rs` that build futures around the pair Mutex + waker.

Add `TmpKind::PtySlave(usize)` variant. tmpfs::open for that kind →
`FileImpl::PtySlave(PtySlaveFile { idx })`.

Mount entries created at `vfs::init()`:
```rust
fs.mkdir(&["dev", "pts"])?;
for i in 0..pty::NUM_PAIRS {
    fs.insert_inode(&["dev", "pts", &i.to_string()], TmpInode {
        kind: TmpKind::PtySlave(i),
        ...
    })?;
}
```

### `kernel/src/wasm/host/term.rs` (new)

```rust
//! WASIX-style termios host fns under module "ruos".

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::{RuntimeState, FdEntry};
use crate::wasm::host::lifecycle::wasm_memory;

const TCSANOW:   i32 = 0;

pub fn tcgetattr(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    termios_ptr: i32,
) -> Result<i32, Error> {
    let pty_idx = lookup_pty(&caller, fd)?;
    let pair = crate::pty::pair(pty_idx);
    let g = pair.lock();
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &g.termios as *const _ as *const u8,
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
    _action: i32,  // we always apply TCSANOW
    termios_ptr: i32,
) -> Result<i32, Error> {
    let pty_idx = lookup_pty(&caller, fd)?;
    let mem = wasm_memory(&caller)?;
    let mut termios = crate::pty::termios::Termios::default_cooked();
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut termios as *mut _ as *mut u8,
            core::mem::size_of::<crate::pty::termios::Termios>(),
        )
    };
    mem.read(&caller, termios_ptr as usize, bytes)
        .map_err(|_| Error::i32_exit(-1))?;
    let pair = crate::pty::pair(pty_idx);
    pair.lock().termios = termios;
    Ok(0)
}

pub fn isatty(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
) -> Result<i32, Error> {
    Ok(if lookup_pty(&caller, fd).is_ok() { 1 } else { 0 })
}

fn lookup_pty(caller: &Caller<'_, RuntimeState>, fd: i32) -> Result<usize, Error> {
    // Map wasm-side FD → VFS Fd → PtySlaveFile.idx.
    // Implementation: walk caller.data().fds[fd as usize], if it's
    // FdEntry::Vfs(vfs_fd), peek the FileImpl variant. For simplicity,
    // store the PTY idx alongside FdEntry::Vfs (extend variant).
    // Specifically: FdEntry becomes Pty(usize) variant set at open time.
    todo!("see state.rs change below")
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "tcgetattr", tcgetattr)?
        .func_wrap("ruos", "tcsetattr", tcsetattr)?
        .func_wrap("ruos", "isatty", isatty)?;
    Ok(())
}
```

### `kernel/src/wasm/state.rs` change

```rust
pub enum FdEntry {
    StdoutConsole,                  // legacy, may retire after Step 12 boot
    Vfs(crate::vfs::Fd),
    Socket(usize),
    Pty(usize),                     // NEW: wasm-side handle directly to PTY idx
}
```

(Drop `Stdin` variant — replaced by VFS open of `/dev/pts/0` returning a
PTY-backed Vfs::Fd. We could just rely on `Vfs(_)`, but a direct Pty(usize)
entry sidesteps the VFS open dance for the boot wiring of FD 0/1/2.)

Actually simpler: `Vfs(Fd)` already works since `/dev/pts/0` open returns
a Fd that backs a PtySlaveFile. The `lookup_pty` host fn in term.rs walks:
FdEntry::Vfs(fd) → consult global FDS table → check if FileImpl is
PtySlave(idx) → return idx. No new FdEntry variant.

Let's go with that. `tcgetattr` etc. peek the FileImpl through the
FDS table.

### `kernel/src/executor/console_drain.rs` (new)

```rust
#[embassy_executor::task]
pub async fn console_drain_task() {
    use core::fmt::Write;
    loop {
        let b = crate::pty::master_output_read(0).await;
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut c = crate::console::CONSOLE.lock();
            let buf = [b];
            let s = core::str::from_utf8(&buf).unwrap_or("?");
            let _ = c.write_str(s);
        });
    }
}
```

Spawned by `executor::run`.

### `kernel/src/keyboard/mod.rs` change

```rust
//! PS/2 keyboard ISR: scancodes → ASCII or ANSI escape, push into PTY 0.

use core::sync::atomic::{AtomicBool, Ordering};
static EXTENDED: AtomicBool = AtomicBool::new(false);

// Set 1 extended scancodes → ANSI sequences
fn extended_to_ansi(scancode: u8) -> Option<&'static [u8]> {
    match scancode {
        0x48 => Some(b"\x1b[A"),  // Up
        0x50 => Some(b"\x1b[B"),  // Down
        0x4D => Some(b"\x1b[C"),  // Right
        0x4B => Some(b"\x1b[D"),  // Left
        0x47 => Some(b"\x1b[H"),  // Home
        0x4F => Some(b"\x1b[F"),  // End
        0x53 => Some(b"\x1b[3~"), // Delete
        _ => None,
    }
}

pub extern "x86-interrupt" fn keyboard_handler(_: InterruptStackFrame) {
    let scancode: u8 = unsafe { Port::<u8>::new(0x60).read() };
    if scancode == 0xE0 {
        EXTENDED.store(true, Ordering::SeqCst);
        crate::apic::lapic::eoi();
        return;
    }
    if EXTENDED.swap(false, Ordering::SeqCst) {
        // Key-release in extended: bit 7 set → ignore
        if scancode < 0x80 {
            if let Some(seq) = extended_to_ansi(scancode) {
                for &b in seq { crate::pty::master_input_push(0, b); }
            }
        }
        crate::apic::lapic::eoi();
        return;
    }
    // Regular scancode → ASCII via SCANCODE_MAP (existing)
    if scancode < 0x80 {
        let idx = scancode as usize;
        if idx < SCANCODE_MAP.len() {
            let ch = SCANCODE_MAP[idx];
            if ch != 0 {
                crate::pty::master_input_push(0, ch);
            }
        }
    }
    crate::apic::lapic::eoi();
}
```

Drop `keyboard::queue` module entirely. Any remaining references retired.

### `user/shell/src/main.rs` rewrite (Task 5)

```rust
#[link(wasm_import_module = "ruos")]
extern "C" {
    fn tcgetattr(fd: i32, p: u32) -> i32;
    fn tcsetattr(fd: i32, a: i32, p: u32) -> i32;
    fn exec(p_ptr: u32, p_len: u32, a_ptr: u32, a_len: u32, ec_ptr: u32) -> i32;
    fn readdir(p_ptr: u32, p_len: u32, b_ptr: u32, b_len: u32, n_ptr: u32) -> i32;
}

#[repr(C)]
struct Termios {
    iflag: u32, oflag: u32, cflag: u32, lflag: u32,
    cc: [u8; 32],
    ispeed: u32, ospeed: u32,
}

const ICANON: u32 = 0o0002;
const ECHO:   u32 = 0o0010;
const ISIG:   u32 = 0o0001;

fn enter_raw_mode() -> Termios {
    let mut saved = Termios { ... zero ... };
    unsafe { tcgetattr(0, &mut saved as *mut _ as u32); }
    let mut raw = saved;
    raw.lflag &= !(ICANON | ECHO | ISIG);
    unsafe { tcsetattr(0, 0, &raw as *const _ as u32); }
    saved
}

fn restore_termios(t: &Termios) {
    unsafe { tcsetattr(0, 0, t as *const _ as u32); }
}

// Line editor with arrows + history + tab
fn read_line_raw(history: &[String]) -> Option<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut cursor: usize = 0;
    let mut hist_idx: Option<usize> = None;
    loop {
        let b = read_one_byte()?;
        match b {
            b'\n' | b'\r' => { println!(); return Some(String::from_utf8(buf).ok()?); }
            0x7F | 0x08 => { /* backspace */ }
            0x01 => { /* Ctrl-A: cursor to start */ }
            0x05 => { /* Ctrl-E: cursor to end */ }
            0x0C => { /* Ctrl-L: clear */ }
            0x03 => { /* Ctrl-C: discard, return None */ return None; }
            b'\t' => { /* tab completion */ }
            0x1B => { /* ESC sequence: read [ + X */
                let _ = read_one_byte()?;  // [
                let arrow = read_one_byte()?;
                match arrow {
                    b'A' => { /* up: history[--idx] */ }
                    b'B' => { /* down */ }
                    b'C' => { /* right */ }
                    b'D' => { /* left */ }
                    _ => {}
                }
            }
            c if c >= 0x20 => { /* printable: insert at cursor, redraw */ }
            _ => {}
        }
    }
}
```

Full body in plan.

History persistente: skipped Step 12 (no /var/ writable yet); in-memory only.

## Boot wiring

`kernel/src/boot/phases/fs.rs`:
```rust
let mounts = vfs::init().map_err(...)?;
crate::binfo!("fs", "tmpfs at /");
crate::pty::init();
crate::binfo!("fs", "{} PTY pair{}", NUM_PAIRS, if NUM_PAIRS == 1 { "" } else { "s" });
let n = modules::mount_all();
crate::binfo!("fs", "{} boot modules", n);
```

`vfs::init` additionally creates `/dev/pts` directory + 4 inodes
(`/dev/pts/0..3`) of `TmpKind::PtySlave(N)`. tmpfs::open dispatches
PtySlave kind → `FileImpl::PtySlave(PtySlaveFile{idx})`.

`kernel/src/executor/mod.rs::run`:
```rust
spawner.spawn(tick_task()).unwrap();
spawner.spawn(net_poll_task()).unwrap();
spawner.spawn(exec_worker_task()).unwrap();
spawner.spawn(console_drain_task()).unwrap();   // NEW
spawner.spawn(wasm_task("/bin/shell.wasm")).unwrap();
```

`kernel/src/wasm/state.rs::new`:
```rust
fn new() -> Self {
    let mut fds = vec![None; 16];
    // FD 0/1/2 = /dev/pts/0 (read+write)
    use crate::vfs::block_on;
    let stdin = block_on(crate::vfs::open("/dev/pts/0", OpenFlags::READ | OpenFlags::WRITE)).ok();
    let stdout = block_on(crate::vfs::open("/dev/pts/0", OpenFlags::READ | OpenFlags::WRITE)).ok();
    let stderr = block_on(crate::vfs::open("/dev/pts/0", OpenFlags::READ | OpenFlags::WRITE)).ok();
    fds[0] = stdin.map(FdEntry::Vfs);
    fds[1] = stdout.map(FdEntry::Vfs);
    fds[2] = stderr.map(FdEntry::Vfs);
    Self { fds, args: vec![], env: vec![], exit_code: AtomicI32::new(0) }
}
```

Note: `vfs::block_on` is Step 7's noop-waker. tmpfs open is single-poll
async ready, so this works synchronously. Acceptable for init-time use.

## Smoke contract

`make run-test` HELLO **unchanged**: `shell: init.sh complete`.

Boot output should show:
```
[T+x] INFO fs   tmpfs at /
[T+x] INFO fs   4 PTY pairs
[T+x] INFO fs   N boot modules
```

Then init.sh runs as before, sentinel hits, shell enters interactive
mode (raw-mode + line editor) — invisible to run-test (no keyboard input).

**Manual VBox smoke**: shell prompt → type partial command → Tab
completes → type → Backspace works → ↑ recalls last command → ←/→
moves cursor → Ctrl-A jumps start → Ctrl-E jumps end → Ctrl-L clears →
Enter executes → output renders → all responsive.

## Componenti / file (riepilogo)

**Nuovi:**
- `kernel/src/pty/{mod,pair,ldisc,termios,allocator}.rs`
- `kernel/src/vfs/devices/pty.rs` (sotto-modulo nuovo dir `devices/`?
  o nello stesso file `devices.rs` esistente — implementer decide;
  esistente `devices.rs` può ospitare struct PtySlaveFile aggiunto)
- `kernel/src/wasm/host/term.rs`
- `kernel/src/executor/console_drain.rs` (o inline in executor/mod.rs)

**Modificati:**
- `kernel/src/keyboard/mod.rs` (0xE0 latch, ANSI emit, → PTY)
- `kernel/src/vfs/{mod,fs,tmpfs,devices,file}.rs` (PtySlaveFile + TmpKind
  PtySlave + mount /dev/pts/0..3)
- `kernel/src/wasm/state.rs` (FD 0/1/2 boot init via /dev/pts/0)
- `kernel/src/wasm/host/mod.rs` (link term)
- `kernel/src/wasm/suspend.rs` (drop KbdReadChar)
- `kernel/src/wasm/fiber.rs` (drop KbdReadChar dispatch arm)
- `kernel/src/wasm/host/fd.rs` (drop Stdin special-case in fd_read)
- `kernel/src/executor/mod.rs` (spawn console_drain_task, drop legacy
  keyboard refs)
- `kernel/src/boot/phases/fs.rs` (pty::init log)
- `user/shell/src/main.rs` (raw mode + line editor)

**Eliminati:**
- `kernel/src/keyboard/queue.rs` (sostituito da PTY ldisc)
- `FdEntry::Stdin` variant
- `SuspendReason::KbdReadChar` variant
- `keyboard::queue` module references

## Decomposizione 5 task

| # | Task | Effort | Sentinel/Test |
|---|------|--------|---------------|
| 1 | PTY core: `pty/{mod,pair,ldisc,termios}.rs` + unit-level kernel test (smoke via boot-checks: master_input_push 'a','b','\n' → slave_rx contains "ab\n") | medio | `[T+x] INFO pty test cooked OK` con boot-checks |
| 2 | VFS integration: `TmpKind::PtySlave`, `PtySlaveFile` File impl, mount `/dev/pts/0..3` al boot, `vfs::open("/dev/pts/0")` → PtySlaveFile FD | medio | shell.wasm può `open("/dev/pts/0") + read/write` (added boot smoke) |
| 3 | Wire keyboard ISR + 0xE0 latch + ANSI + console_drain_task + boot FD 0/1/2 init in RuntimeState; drop kbd queue + Stdin/KbdReadChar | grosso | `make run-test` PASS, sentinel preserved; manual: digito 'abc\n' in VBox vede echo + shell runs `abc` (not found, ok) |
| 4 | WASIX `tcgetattr/tcsetattr/isatty` host fns in `wasm/host/term.rs`; struct layout verified | piccolo | shell switches raw mode without crash; tcgetattr ritorna struct default |
| 5 | Shell line editor: raw mode at startup, arrows + history + tab + Ctrl-A/E/L/C; restore termios on exit | grosso | manual VBox: tutti i comandi classici funzionano |

## Out of scope (rimandati)

- `/dev/ptmx` dynamic allocator → Step 15 SSH
- `ptsname(fd)` → Step 15
- Window size (`TIOCGWINSZ`) → Step 13 GUI
- Job control (`SIGTSTP`, foreground process group) → mai (no signals)
- Multi-terminal switch (Ctrl-Alt-F1..F6) → mai/Step 13
- Pipe (`|`) e redirection (`>`/`<`) — necessitano PTY-aware shell parsing
  + nuovi fd wiring; differiti a Step 12.5 o 13
- History persistente su VFS → Step 12.5
- Vi-mode line editing → mai
- Process groups & sessions → mai
- `select()`/`poll()` su FD multiple → semi-supportato via poll_oneoff;
  Step 14 quando arriverà network proper

## Open points (decisioni in implementazione)

- **wasi-libc termios struct binary layout**: 60-byte ABI standard. Se
  shell.wasm fa `tcgetattr` via wasi-libc binding e legge offset diversi,
  controllare con wasm-objdump l'import + struct size. Probabilmente
  shell.wasm chiama direct extern "C" sotto module "ruos", saltando
  wasi-libc — in tal caso layout è quello che noi scegliamo.
- **VecDeque bounded vs unbounded**: bounded a 4 KiB per direzione sembra
  sano; flow-control non implementato. Run con `make run-test` corto
  trabocca? Improbabile per shell ma da monitorare.
- **Drop legacy `keyboard::queue`**: T3 step. Verificare che nessuno
  riferimento residuo (grep + cargo build clean).
- **Slave waker single vs multi**: per Step 12 shell singolo reader →
  single Waker basta. Step 15 SSH potrebbe avere più reader concorrenti
  → VecDeque<Waker> da fare allora.

## Closing items dei followup precedenti

- Step 9 F3 (0xE0 latching) ✓ chiuso da T3
- Step 10.5 F4 (keyboard queue single-Waker race) ✓ chiuso da T3 (queue
  retirata, replaced by PTY ldisc)
- Step 11 F7 (KbdReadChar runtime debug) ✓ irrelevante (KbdReadChar
  retired)
