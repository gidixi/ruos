# Boot phase refactor — pro-grade kmain + structured boot log

**Data:** 2026-05-29
**Stato:** spec approvata, da implementare

## Contesto

`kernel/src/main.rs::kmain` è 240 righe di init inline misto a smoke test:
- Heap alloc + `Box::new(0xCAFEBABE)` + `Vec` smoke nel boot path
- Paging map+write+read+unmap smoke nel boot path
- `kprintln!("ruos: X ok ...")` ad-hoc, formato libero, niente livello,
  niente timestamp, niente modulo
- Error handling = `match { Ok => kprintln(ok); Err => kprintln(err); hcf() }`
  ripetuto a ogni step
- Tutto in `kmain`, nessuna funzione di phase

Vuole avere il look di un kernel "vero" (Linux/seL4/Redox style): kmain
corto, phases esplicite, logger strutturato, banner di boot, smoke gated
dietro feature.

## Obiettivo

`kmain` < 40 righe. Boot path organizzato in **6 fasi** esplicite:
1. **arch** — GDT + IDT + TSS + #BP smoke
2. **mem** — heap + frames + paging
3. **interrupts** — PIC disable + ACPI + LAPIC + IOAPIC + timer
4. **devices** — keyboard PS/2 + framebuffer + ANSI
5. **fs** — VFS init + modules mount
6. **userland** — net init + executor::run (mai ritorna)

Logger strutturato: `[T+1.234s] INFO  mod: msg`. Banner ASCII con
versione + git SHA + memoria + features. Smoke test gated dietro feature
`boot-checks`, off di default.

## Decisioni strategiche

1. **Opt A** scelto: refactor completo. Phases + logger + banner +
   smoke gated. ~3 task subagent, ~30-40 LoC kmain finale.
2. **Logger backend**: emette su CONSOLE.lock() esistente. Singolo
   helper `boot::log::{info,warn,error}(module, args)` con formato
   `[T+SECS.MILLISs] LEVEL mod: msg\n`.
3. **Timestamp source**: `timer::ticks() * 10ms`. Pre-timer-init le
   righe escono con `[T+pre]` (timer non ancora attivo). Post-init
   timestamp veri.
4. **Banner**: stampato subito dopo serial init, prima di tutto il
   resto. Versione = `env!("CARGO_PKG_VERSION")`. Git SHA via build
   script (`build.rs` legge `git rev-parse --short HEAD`, embed in
   env var). Build date via macro standard.
5. **BootError**: enum unificato in `boot/error.rs`. Ogni phase
   ritorna `Result<(), BootError>`. `boot::run()` matcha e logga.
6. **Smoke test feature**: `boot-checks` Cargo feature. Default OFF.
   Quando ON: heap alloc smoke, paging map smoke, VFS smoke. Off:
   silent fast boot.

## Architettura

```
kernel/src/boot/
  mod.rs          — Phase enum + run() driver + BootError
  log.rs          — info!/warn!/error! macro + Logger impl
  banner.rs       — banner ASCII stamping
  error.rs        — BootError enum
  phases/
    mod.rs        — module declarations
    arch.rs       — gdt + idt + sti smoke trigger
    mem.rs        — heap + frames + paging
    interrupts.rs — pic + acpi + apic + timer
    devices.rs    — keyboard + framebuffer + ansi
    fs.rs         — vfs + modules
    userland.rs   — net + executor (never returns)
```

**post-refactor `kmain`**:
```rust
#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    // Serial up first — boot log goes nowhere else.
    crate::serial::SERIAL.lock().init();
    if !BASE_REVISION.is_supported() {
        // Pre-banner panic: still write something to serial.
        kprintln!("ruos: limine base revision not supported");
        hcf();
    }
    boot::banner::stamp();
    if let Err(e) = boot::run() {
        boot::log::error("boot", format_args!("phase failed: {:?}", e));
        hcf();
    }
    // boot::phases::userland::init runs executor::run which never returns.
    unreachable!()
}
```

**`boot::run`**:
```rust
pub fn run() -> Result<(), BootError> {
    phases::arch::init()?;
    phases::mem::init()?;
    phases::interrupts::init()?;
    phases::devices::init()?;
    phases::fs::init()?;
    phases::userland::init()  // -> ! (mai ritorna)
}
```

**`boot::log`** (per-livello, ISR-safe via interrupts::without_interrupts):
```rust
pub fn info(module: &str, args: core::fmt::Arguments) {
    emit('I', module, args);
}
pub fn warn(module: &str, args: core::fmt::Arguments) {
    emit('W', module, args);
}
pub fn error(module: &str, args: core::fmt::Arguments) {
    emit('E', module, args);
}

fn emit(level: char, module: &str, args: core::fmt::Arguments) {
    use x86_64::instructions::interrupts::without_interrupts;
    without_interrupts(|| {
        let ticks = crate::timer::ticks();
        let s = ticks / 100;
        let ms = (ticks % 100) * 10;
        let mut c = crate::console::CONSOLE.lock();
        use core::fmt::Write;
        let _ = writeln!(c, "[T+{:5}.{:03}s] {} {:8} {}", s, ms, level, module, args);
    });
}

#[macro_export]
macro_rules! binfo {
    ($mod:literal, $($arg:tt)*) => { $crate::boot::log::info($mod, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! bwarn {
    ($mod:literal, $($arg:tt)*) => { $crate::boot::log::warn($mod, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! berr {
    ($mod:literal, $($arg:tt)*) => { $crate::boot::log::error($mod, format_args!($($arg)*)) };
}
```

**`boot::banner`**:
```rust
pub fn stamp() {
    use core::fmt::Write;
    use x86_64::instructions::interrupts::without_interrupts;
    let version = env!("CARGO_PKG_VERSION");
    let sha = option_env!("RUOS_GIT_SHA").unwrap_or("unknown");
    let date = option_env!("RUOS_BUILD_DATE").unwrap_or("unknown");
    without_interrupts(|| {
        let mut c = crate::console::CONSOLE.lock();
        let _ = writeln!(c, "");
        let _ = writeln!(c, "  ╔═══════════════════════════════════════════════╗");
        let _ = writeln!(c, "  ║   ruos v{:<8}  ({}, {})           ║", version, sha, date);
        let _ = writeln!(c, "  ║   x86_64-unknown-none / Limine 11.4.1         ║");
        let _ = writeln!(c, "  ║   WASIX bootstrap + cooperative fibers        ║");
        let _ = writeln!(c, "  ╚═══════════════════════════════════════════════╝");
        let _ = writeln!(c, "");
    });
}
```

**`build.rs`** (new at `kernel/build.rs`):
```rust
use std::process::Command;
fn main() {
    let sha = Command::new("git").args(["rev-parse", "--short", "HEAD"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUOS_GIT_SHA={}", sha);
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    println!("cargo:rustc-env=RUOS_BUILD_DATE={}", date);
    println!("cargo:rerun-if-changed=src/");
}
```

(Skip `chrono` dep — use `std::process::Command` + `date -u +%Y-%m-%d`
shell out instead, no extra dep.)

## Smoke test feature gating

`kernel/Cargo.toml`:
```toml
[features]
boot-checks = []
```

Smoke macro in `boot/mod.rs`:
```rust
#[cfg(feature = "boot-checks")]
pub fn smoke_heap() { ... }
#[cfg(not(feature = "boot-checks"))]
pub fn smoke_heap() {}
```

Default boot: `make build` con feature OFF → no smoke output.
`make test-boot` (target nuovo): feature ON, run-test stronger.

Per Step 11 compatibility: `make run-test` deve continuare a passare il
sentinel `shell: init.sh complete`. Smoke OFF doesn't break that.

## Output cambia

### Da

```
ruos: hello serial
ruos: heap ok base=0xFFFF800000100000 size=4194304
ruos: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]
ruos: idt up
ruos: bp ok rip=0xFFFFFFFF8000ADB2
ruos: acpi ok lapic=0xFEE00000 ioapic=0xFEC00000 overrides=5
ruos: frames total=130951 used=3254 free=127697
ruos: paging up
ruos: map test ok virt=0x400000000000 phys=0x61000
ruos: lapic calibrated 62596700 ticks/sec, periodic count=625967
ruos: vfs init ok mounts=1
ruos: vfs smoke ok n=3 buf=[abc]
ruos: fb ok 1280x800 pitch=5120 bpp=32
ruos: fb test ok
ruos: fb attached
[31mERR[0m hello via ansi
ruos: ansi test ok
ruos: module mounted at /init.wasm (NNNNN bytes)
ruos: module mounted at /server.wasm (NNNNN bytes)
... (5 more)
ruos: mounted 8 boot modules
ruos: net init ok addr=127.0.0.1/8
ruos: real ping-pong (no preload)
ruos: executor up
init.wasm: argv0=/init.wasm
... (rest of shell flow)
```

### A

```

  ╔═══════════════════════════════════════════════╗
  ║   ruos v0.1.0    (844cbbf, 2026-05-29)        ║
  ║   x86_64-unknown-none / Limine 11.4.1         ║
  ║   WASIX bootstrap + cooperative fibers        ║
  ╚═══════════════════════════════════════════════╝

[T+    0.000s] I arch     GDT + IDT + TSS up
[T+    0.000s] I mem      heap 4 MiB at 0xFFFF800000100000
[T+    0.012s] I mem      frames 127697/130951 free
[T+    0.014s] I mem      paging OK (HHDM offset 0xFFFF800000000000)
[T+    0.045s] I intr     LAPIC 62.5 MHz @ 100 Hz timer
[T+    0.050s] I intr     IOAPIC IRQ1 keyboard wired
[T+    0.061s] I dev      framebuffer 1280x800x32 + ANSI parser
[T+    0.062s] I fs       tmpfs at /
[T+    0.080s] I fs       8 boot modules mounted
[T+    0.082s] I net      127.0.0.1/8 (loopback)
[T+    0.085s] I user     executor up: tick + net_poll + 4 wasm tasks + exec_worker

init.wasm: argv0=/init.wasm
... (wasm output unchanged)
shell: init.sh complete
```

## Decomposizione 3 task

1. **T1 — Boot infra + banner**: nuovo `kernel/src/boot/{mod,log,banner,error}.rs`.
   `Logger` + `binfo!`/`bwarn!`/`berr!` macro + `BootError` enum +
   `Phase` enum (lasciato come placeholder per T2) + banner stamper.
   `build.rs` per RUOS_GIT_SHA + RUOS_BUILD_DATE. `kmain` chiama
   `boot::banner::stamp()` ma resto invariato. Sentinel: banner appare,
   `shell: init.sh complete` resta.
2. **T2 — Phases extraction**: split kmain in 6 `phases/*.rs`.
   `boot::run()` driver. kmain shrinks a ~30 righe. Smoke test gated
   con `#[cfg(feature = "boot-checks")]`. Logger migrato per phases.
   Sentinel: `shell: init.sh complete` resta, output ora strutturato.
3. **T3 — Cleanup + log migration finale**: tutti i `kprintln!("ruos: …")`
   restanti (in moduli non-boot: timer, vfs, ecc.) migrati a
   `binfo!("source", …)`. `make test-boot` target nuovo (feature
   `boot-checks` + assert stronger). Final regression check.

## Smoke contract

`make run-test` HELLO **unchanged**: `shell: init.sh complete`. La
refactor non cambia il behavior osservabile dei wasm — solo il formato
delle righe kernel boot.

Aggiungo `make test-boot` target nuovo che builda con `--features boot-checks`
e asserta sentinel `boot: all checks passed` (post-smoke).

## File toccati (riepilogo)

**Nuovi:**
- `kernel/src/boot/mod.rs` + `error.rs` + `log.rs` + `banner.rs`
- `kernel/src/boot/phases/{mod,arch,mem,interrupts,devices,fs,userland}.rs`
- `kernel/build.rs` (env vars RUOS_GIT_SHA + RUOS_BUILD_DATE)

**Modificati:**
- `kernel/src/main.rs` — drasticamente ridotto (240 → ~30 righe)
- `kernel/Cargo.toml` — `[features] boot-checks = []`
- `kernel/src/{timer,vfs/mod,keyboard/mod,…}.rs` — kprintln → binfo
  (selettivamente, dove "ruos: X" log ad-hoc)
- `Makefile` — `make test-boot` target

## Out of scope

- ASCII art più elaborata
- `dmesg`-like log buffer (persistent log in RAM) → Step 12+
- Boot timing per-phase breakdown table → Step 13+
- Banner via framebuffer (oggi solo serial via CONSOLE)
- `loglevel=` kernel cmdline param (Linux-style) → quando avremo
  cmdline parsing
- Rename `kprintln!` → `pr_*` macro convention (Linux-style) — rimane
  come low-level escape hatch
