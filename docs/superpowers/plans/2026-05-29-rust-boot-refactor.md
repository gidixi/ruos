# Boot Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** kmain da 240 a ~30 righe; 6 phases esplicite; logger strutturato; banner ASCII; smoke gated.

**Architecture:** `kernel/src/boot/` modulo nuovo. Phases come fns `init() -> Result<(), BootError>`. `boot::run()` driver chiamato da kmain. Logger emette `[T+SECS.MILLISs] L mod: msg` via CONSOLE.

**Tech Stack:** Rust no_std + alloc, `build.rs` per env vars git SHA + build date. Cargo feature `boot-checks` per smoke opt-in.

**Spec:** `docs/superpowers/specs/2026-05-29-rust-boot-refactor-design.md`

**Branch:** `feature/boot-refactor` (already created)

**Build host:** WSL Ubuntu:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
```

**Changelog:** spec=85, plan=86, T1=87, T2=88, T3=89.

**Git identity (mandatory):**
```
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit ...
```
Co-author trailer mandatory:
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

**Smoke contract**: `make run-test` HELLO **unchanged** through all 3
tasks: `shell: init.sh complete`. Refactor non cambia wasm behavior.

---

## File Structure

**New:**
- `kernel/build.rs` — emette `RUOS_GIT_SHA` + `RUOS_BUILD_DATE` env vars
- `kernel/src/boot/mod.rs` — Phase enum + `run()` + macro re-export
- `kernel/src/boot/error.rs` — `BootError` enum
- `kernel/src/boot/log.rs` — `info`/`warn`/`error` + `binfo!`/`bwarn!`/`berr!`
- `kernel/src/boot/banner.rs` — `stamp()` ASCII banner
- `kernel/src/boot/phases/mod.rs` — module decls
- `kernel/src/boot/phases/arch.rs` (T2)
- `kernel/src/boot/phases/mem.rs` (T2)
- `kernel/src/boot/phases/interrupts.rs` (T2)
- `kernel/src/boot/phases/devices.rs` (T2)
- `kernel/src/boot/phases/fs.rs` (T2)
- `kernel/src/boot/phases/userland.rs` (T2)

**Modified:**
- `kernel/Cargo.toml` — `build = "build.rs"` + `[features] boot-checks = []`
- `kernel/src/main.rs` — T1 adds banner call, T2 shrinks to ~30 lines
- `kernel/src/timer.rs`, `kernel/src/vfs/mod.rs`, etc. (T3) — kprintln → binfo
- `Makefile` — `make test-boot` target (T3)

---

## Task 1: Boot infra (logger + banner + error + build.rs)

**Files:**
- Create: `kernel/build.rs`
- Create: `kernel/src/boot/{mod,log,banner,error}.rs`
- Modify: `kernel/src/main.rs` — `mod boot;` + `boot::banner::stamp()` after serial init
- Modify: `kernel/Cargo.toml` — `build = "build.rs"`

**Smoke contract:** unchanged `shell: init.sh complete`. Output gains banner block at top.

This task lays infra without disrupting existing init. kmain still does all init inline, just gains a banner stamp call early.

- [ ] **Step 1.1: Create `kernel/build.rs`**

```rust
use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUOS_GIT_SHA={}", sha);

    let date = Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUOS_BUILD_DATE={}", date);

    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=../.git/HEAD");
}
```

- [ ] **Step 1.2: Wire build.rs in `kernel/Cargo.toml`**

In `[package]` section, add:
```toml
build = "build.rs"
```

- [ ] **Step 1.3: Create `kernel/src/boot/error.rs`**

```rust
//! Unified boot error type. Each phase returns Result<(), BootError>.

#[derive(Debug)]
pub enum BootError {
    LimineUnsupported,
    HeapInit(&'static str),
    AcpiInit(&'static str),
    FramesInit(&'static str),
    PagingInit(&'static str),
    TimerInit(&'static str),
    VfsInit(&'static str),
    ModulesMount(&'static str),
    NetInit(&'static str),
    Other(&'static str),
}
```

- [ ] **Step 1.4: Create `kernel/src/boot/log.rs`**

```rust
//! Structured boot logger. Format: `[T+SECS.MILLISs] L  mod: msg`.

use core::fmt::Write;
use x86_64::instructions::interrupts::without_interrupts;

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
    without_interrupts(|| {
        let ticks = crate::timer::ticks();
        let s = ticks / 100;
        let ms = (ticks % 100) * 10;
        let mut c = crate::console::CONSOLE.lock();
        let _ = writeln!(c, "[T+{:5}.{:03}s] {}  {:8} {}", s, ms, level, module, args);
    });
}

#[macro_export]
macro_rules! binfo {
    ($module:literal, $($arg:tt)*) => {
        $crate::boot::log::info($module, core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! bwarn {
    ($module:literal, $($arg:tt)*) => {
        $crate::boot::log::warn($module, core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! berr {
    ($module:literal, $($arg:tt)*) => {
        $crate::boot::log::error($module, core::format_args!($($arg)*))
    };
}
```

- [ ] **Step 1.5: Create `kernel/src/boot/banner.rs`**

```rust
//! Boot banner. Stamped right after serial init, before any phase.

use core::fmt::Write;
use x86_64::instructions::interrupts::without_interrupts;

pub fn stamp() {
    let version = env!("CARGO_PKG_VERSION");
    let sha = option_env!("RUOS_GIT_SHA").unwrap_or("unknown");
    let date = option_env!("RUOS_BUILD_DATE").unwrap_or("unknown");

    without_interrupts(|| {
        let mut c = crate::console::CONSOLE.lock();
        let _ = writeln!(c);
        let _ = writeln!(c, "  ╔═══════════════════════════════════════════════╗");
        let _ = writeln!(c, "  ║   ruos v{:<8}  ({}, {})           ║", version, sha, date);
        let _ = writeln!(c, "  ║   x86_64-unknown-none / Limine 11.4.1         ║");
        let _ = writeln!(c, "  ║   WASIX bootstrap + cooperative fibers        ║");
        let _ = writeln!(c, "  ╚═══════════════════════════════════════════════╝");
        let _ = writeln!(c);
    });
}
```

- [ ] **Step 1.6: Create `kernel/src/boot/mod.rs`**

```rust
//! Boot phase orchestration.

pub mod log;
pub mod banner;
pub mod error;

pub use error::BootError;

// Phases come in Task 2.
```

- [ ] **Step 1.7: Wire `mod boot;` + banner in `kernel/src/main.rs`**

Add `mod boot;` to the module declarations (alongside `mod serial;`).

In `kmain`, after `crate::serial::SERIAL.lock().init();` and the Limine base revision check, add:

```rust
    boot::banner::stamp();
```

- [ ] **Step 1.8: Build + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
```

If `build.rs` doesn't run, ensure `build = "build.rs"` is in `[package]` (not `[dependencies]`).

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | head -30'
```

Expected serial start:
```
ruos: hello serial

  ╔═══════════════════════════════════════════════╗
  ║   ruos v0.1.0    (<sha>, <date>)              ║
  ║   x86_64-unknown-none / Limine 11.4.1         ║
  ║   WASIX bootstrap + cooperative fibers        ║
  ╚═══════════════════════════════════════════════╝

ruos: heap ok ...
... (rest unchanged)
shell: init.sh complete
TEST_PASS
```

Sentinel still PASS.

- [ ] **Step 1.9: Changelog + commit**

Create `CHANGELOG/87-26-05-29-boot-refactor-infra.md`:

```markdown
# 87 — Boot infra: banner + logger + error + build.rs (T1)

**Data:** 2026-05-29

## Cosa

- `kernel/build.rs`: emette `RUOS_GIT_SHA` (git rev-parse) + `RUOS_BUILD_DATE` (date -u).
- `kernel/Cargo.toml`: `build = "build.rs"`.
- `kernel/src/boot/{mod,log,banner,error}.rs` (nuovi).
- `boot::log` + macro `binfo!`/`bwarn!`/`berr!` (non ancora chiamate da
  nessuno; T2/T3 le useranno).
- `boot::banner::stamp()` chiamata in `kmain` subito dopo serial init.
- `BootError` enum (varianti per ogni phase fail mode).

## Perché

Step 1 del boot refactor: posa l'infrastruttura senza modificare init
flow. kmain ora stampa banner + esegue init come prima. Output esistente
invariato sotto al banner.

## File toccati

- kernel/build.rs (nuovo)
- kernel/Cargo.toml
- kernel/src/boot/{mod,log,banner,error}.rs (nuovi)
- kernel/src/main.rs
- CHANGELOG/87-26-05-29-boot-refactor-infra.md (nuovo)
```

```bash
git add kernel/build.rs kernel/Cargo.toml kernel/src/boot/ kernel/src/main.rs CHANGELOG/87-26-05-29-boot-refactor-infra.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): boot infra (banner + logger + BootError + build.rs)

build.rs emits RUOS_GIT_SHA via 'git rev-parse --short HEAD' and
RUOS_BUILD_DATE via 'date -u +%Y-%m-%d'. boot::banner::stamp()
writes an ASCII banner with version/sha/date to console after
serial init.

boot::log + binfo!/bwarn!/berr! macros provide a structured emit
format [T+SECS.MILLISs] L  mod: msg targeted at the existing
CONSOLE. Not yet called from kmain; T2/T3 wire them in.

BootError enum collects all phase failure modes for the future
boot::run driver.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Phases extraction (kmain shrink + smoke gated)

**Files:**
- Create: `kernel/src/boot/phases/mod.rs`
- Create: `kernel/src/boot/phases/{arch,mem,interrupts,devices,fs,userland}.rs`
- Modify: `kernel/src/boot/mod.rs` — add `pub mod phases;` + `pub fn run()`
- Modify: `kernel/src/main.rs` — drastic shrink
- Modify: `kernel/Cargo.toml` — `[features] boot-checks = []`

**Smoke contract:** unchanged `shell: init.sh complete`. Output structure
changes (logs now via binfo, smoke gated off).

- [ ] **Step 2.1: Add `boot-checks` feature**

In `kernel/Cargo.toml`, add:
```toml
[features]
boot-checks = []
```

- [ ] **Step 2.2: Create `kernel/src/boot/phases/mod.rs`**

```rust
pub mod arch;
pub mod mem;
pub mod interrupts;
pub mod devices;
pub mod fs;
pub mod userland;
```

- [ ] **Step 2.3: Extend `kernel/src/boot/mod.rs` with `run`**

```rust
pub mod log;
pub mod banner;
pub mod error;
pub mod phases;

pub use error::BootError;

pub fn run() -> Result<!, BootError> {
    phases::arch::init()?;
    phases::mem::init()?;
    phases::interrupts::init()?;
    phases::devices::init()?;
    phases::fs::init()?;
    phases::userland::init()  // -> ! (never returns)
}
```

(Note: `Result<!, BootError>` is the never-type — replace with `Result<core::convert::Infallible, BootError>` if `!` isn't stable in your nightly.)

- [ ] **Step 2.4: Create `kernel/src/boot/phases/arch.rs`**

Extract the GDT + IDT init from the current kmain (lines around 88-94):

```rust
//! Phase 1: arch — GDT + IDT + TSS.

use crate::boot::BootError;
use crate::binfo;

pub fn init() -> Result<(), BootError> {
    crate::gdt::init();
    crate::idt::init();
    binfo!("arch", "GDT + IDT + TSS up");

    #[cfg(feature = "boot-checks")]
    {
        // #BP smoke test: traps are not maskable by IF.
        unsafe { core::arch::asm!("int3"); }
        binfo!("arch", "#BP smoke OK");
    }

    Ok(())
}
```

- [ ] **Step 2.5: Create `kernel/src/boot/phases/mem.rs`**

```rust
//! Phase 2: mem — heap + frames + paging.

use crate::boot::BootError;
use crate::{binfo, memory};

pub fn init() -> Result<(), BootError> {
    let info = memory::init_heap()
        .map_err(|_| BootError::HeapInit("init_heap failed"))?;
    binfo!("mem", "heap {} KiB at 0x{:X}", info.size / 1024, info.virt_base);

    #[cfg(feature = "boot-checks")]
    {
        use alloc::boxed::Box;
        use alloc::vec::Vec;
        let b = Box::new(0xCAFEBABEu64);
        let v: Vec<u32> = (0..5).collect();
        binfo!("mem", "alloc smoke box=0x{:X} vec={:?}", *b, v);
    }

    // ACPI parse depends on heap, but its result is consumed by the
    // interrupts phase. We park it in a OnceCell-style static.
    let acpi_info = crate::acpi_init::parse()
        .map_err(|_| BootError::AcpiInit("acpi_init::parse failed"))?;
    binfo!("mem", "ACPI lapic=0x{:X} ioapic=0x{:X}",
        acpi_info.lapic_base, acpi_info.ioapic_base);
    super::set_acpi_info(acpi_info);

    let counts = memory::init_frames()
        .map_err(|_| BootError::FramesInit("init_frames failed"))?;
    binfo!("mem", "frames {}/{} free",
        counts.free, counts.total);

    let hhdm = super::get_acpi_info().hhdm_offset;
    memory::init_mapper(hhdm);
    binfo!("mem", "paging OK (HHDM 0x{:X})", hhdm);

    #[cfg(feature = "boot-checks")]
    {
        use x86_64::structures::paging::PageTableFlags;
        let virt = x86_64::VirtAddr::new(0x4000_0000_0000);
        let frame = memory::allocate_frame()
            .ok_or(BootError::PagingInit("no frame for smoke"))?;
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_EXECUTE;
        memory::map_page(virt, frame.start_address(), flags)
            .map_err(|_| BootError::PagingInit("map_page failed"))?;
        unsafe { virt.as_mut_ptr::<u64>().write_volatile(0xC0FFEE); }
        let back = unsafe { virt.as_ptr::<u64>().read_volatile() };
        memory::unmap_page(virt)
            .map_err(|_| BootError::PagingInit("unmap_page failed"))?;
        memory::free_frame(frame);
        binfo!("mem", "paging smoke OK (0x{:X})", back);
    }

    Ok(())
}
```

The `super::set_acpi_info` / `get_acpi_info` plumbing keeps ACPI state
between phases. Add this to `kernel/src/boot/phases/mod.rs`:

```rust
use spin::Mutex;
use crate::acpi_init::AcpiInfo;

static ACPI: Mutex<Option<AcpiInfo>> = Mutex::new(None);

pub(super) fn set_acpi_info(info: AcpiInfo) {
    *ACPI.lock() = Some(info);
}

pub(super) fn get_acpi_info() -> AcpiInfo {
    ACPI.lock().clone().expect("acpi not yet initialized")
}
```

`AcpiInfo` must derive `Clone`. Check `kernel/src/acpi_init.rs` and add `#[derive(Clone)]` to the struct if missing.

- [ ] **Step 2.6: Create `kernel/src/boot/phases/interrupts.rs`**

```rust
//! Phase 3: interrupts — PIC disable + LAPIC + IOAPIC + timer.

use crate::boot::BootError;
use crate::{binfo, pic, apic, timer, idt};

pub fn init() -> Result<(), BootError> {
    pic::disable();
    binfo!("intr", "legacy PIC disabled");

    let acpi = super::get_acpi_info();
    apic::lapic::init(acpi.lapic_base);
    apic::ioapic::init(acpi.ioapic_base);
    binfo!("intr", "LAPIC + IOAPIC up");

    timer::init(100)
        .map_err(|_| BootError::TimerInit("timer init"))?;
    binfo!("intr", "timer 100 Hz");

    apic::ioapic::redirect(1, idt::VEC_KEYBOARD, &acpi.overrides);
    binfo!("intr", "IRQ1 keyboard wired");

    x86_64::instructions::interrupts::enable();
    binfo!("intr", "IF enabled");

    Ok(())
}
```

(Adapt to actual existing API names — check current `main.rs` for what timer/apic/ioapic init looks like.)

- [ ] **Step 2.7: Create `kernel/src/boot/phases/devices.rs`**

```rust
//! Phase 4: devices — framebuffer + ANSI.

use crate::boot::BootError;
use crate::binfo;

pub fn init() -> Result<(), BootError> {
    match crate::console::fb_init::init() {
        Ok(info) => {
            binfo!("dev", "framebuffer {}x{}x{}", info.width, info.height, info.bpp);
            crate::console::attach_framebuffer(info);
            binfo!("dev", "ANSI parser + cursor blink active");
        }
        Err(e) => {
            crate::bwarn!("dev", "framebuffer unavailable: {:?}", e);
        }
    }

    #[cfg(feature = "boot-checks")]
    crate::kprintln!("\x1b[31mERR\x1b[0m hello via ansi");

    Ok(())
}
```

(Adapt `fb_init` / `attach_framebuffer` names to the current console API.)

- [ ] **Step 2.8: Create `kernel/src/boot/phases/fs.rs`**

```rust
//! Phase 5: fs — VFS + modules mount.

use crate::boot::BootError;
use crate::{binfo, vfs, modules};

pub fn init() -> Result<(), BootError> {
    let mounts = vfs::init()
        .map_err(|_| BootError::VfsInit("vfs::init"))?;
    binfo!("fs", "tmpfs mounted at / ({} mount{})",
        mounts, if mounts == 1 { "" } else { "s" });

    #[cfg(feature = "boot-checks")]
    {
        let smoke = vfs::block_on(async {
            use vfs::{open, write, read, close, OpenFlags};
            let fd = open("/tmp/_smoke", OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ).await?;
            write(fd, b"abc").await?;
            let mut buf = [0u8; 3];
            let _ = read(fd, &mut buf).await?;
            close(fd).await?;
            Ok::<[u8; 3], vfs::VfsError>(buf)
        });
        if let Ok(buf) = smoke {
            binfo!("fs", "smoke {:?}", &buf);
        }
    }

    let n = modules::mount_all();
    binfo!("fs", "{} boot modules mounted", n);

    Ok(())
}
```

- [ ] **Step 2.9: Create `kernel/src/boot/phases/userland.rs`**

```rust
//! Phase 6: userland — net + executor. Never returns.

use crate::boot::BootError;
use crate::{binfo, net};

pub fn init() -> Result<(), BootError> {
    net::init();
    binfo!("net", "127.0.0.1/8 (loopback only)");

    binfo!("user", "executor up");
    crate::executor::run();  // -> ! never returns
    // Unreachable.
}
```

- [ ] **Step 2.10: Shrink `kernel/src/main.rs`**

Replace `kmain` body so it's just:

```rust
#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    crate::serial::SERIAL.lock().init();
    if !BASE_REVISION.is_supported() {
        kprintln!("ruos: limine base revision not supported");
        hcf();
    }
    boot::banner::stamp();
    if let Err(e) = boot::run() {
        crate::berr!("boot", "phase failed: {:?}", e);
        hcf();
    }
    // boot::run() returns Result<!, _> → Ok(_) is unreachable; userland never returns.
    unreachable!()
}
```

If `Result<!, BootError>` isn't accepted, use `Result<core::convert::Infallible, BootError>` and `_ => unreachable!()`.

Delete the rest of the old kmain inline init (the heap/idt/acpi/frames/paging/timer/kbd/vfs/fb/net/executor blocks). All moved to phases.

- [ ] **Step 2.11: Build + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -15'
```

Iterate on errors. Common:
- `AcpiInfo` doesn't impl Clone → add derive
- `info.width` etc. may be `info.fb_width` — check `console::fb::FbInfo` definition
- `Result<!, _>` requires `#![feature(never_type)]` on some nightlies → switch to Infallible

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | tail -30'
```

Expected: `shell: init.sh complete` sentinel still PASS. Boot logs now use binfo format. Output cleaner without smoke noise.

- [ ] **Step 2.12: Changelog + commit**

Create `CHANGELOG/88-26-05-29-boot-refactor-phases.md`:

```markdown
# 88 — Boot phases extraction + kmain shrink (T2)

**Data:** 2026-05-29

## Cosa

- `kernel/src/boot/phases/{arch,mem,interrupts,devices,fs,userland}.rs`
  (nuovi). Ognuno = `pub fn init() -> Result<(), BootError>`.
- `boot::run()` driver chiama tutte le 6 phases in sequenza.
- `kmain` ridotto da ~240 a ~25 righe: serial + banner + boot::run +
  hcf su error.
- ACPI info parkata in `boot::phases::ACPI` (Mutex) tra mem e
  interrupts phase.
- Smoke test (heap alloc box+vec, paging map, vfs smoke, ANSI hello)
  gated dietro `#[cfg(feature = "boot-checks")]`.
- `kernel/Cargo.toml`: `[features] boot-checks = []` (default OFF).
- AcpiInfo gained Clone derive per parkare tra phases.

## Perché

Step 2 del boot refactor. Phases esplicite + smoke off di default =
boot output pulito + structure pro.

## File toccati

- kernel/src/boot/mod.rs
- kernel/src/boot/phases/{mod,arch,mem,interrupts,devices,fs,userland}.rs (nuovi)
- kernel/src/main.rs
- kernel/src/acpi_init.rs (Clone derive)
- kernel/Cargo.toml
- CHANGELOG/88-26-05-29-boot-refactor-phases.md (nuovo)
```

```bash
git add kernel/Cargo.toml kernel/src/boot/ kernel/src/main.rs kernel/src/acpi_init.rs CHANGELOG/88-26-05-29-boot-refactor-phases.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): boot phases extraction + kmain shrink

kmain shrinks from ~240 to ~25 lines: serial init, banner stamp,
boot::run() driver, hcf on phase error. Inline init is split into
six phase modules under boot/phases: arch (GDT/IDT/TSS), mem
(heap + frames + paging + ACPI parse), interrupts (PIC + LAPIC +
IOAPIC + timer + IRQ1), devices (framebuffer + ANSI), fs (VFS +
modules), userland (net + executor — never returns).

Smoke tests (heap alloc box+vec, paging map+write+read+unmap,
VFS smoke, ANSI hello) move behind #[cfg(feature = 'boot-checks')]
so the default boot is silent and fast. make build leaves them
out; make test-boot (T3) builds them in.

ACPI info parks in boot::phases::ACPI between mem and interrupts.
AcpiInfo gains #[derive(Clone)].

Sentinel shell: init.sh complete PASS unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Log migration finale + `make test-boot` target

**Files:**
- Modify: various source files that still use `kprintln!("ruos: ...")` for ad-hoc init logs
- Modify: `Makefile` — `test-boot` target with `--features boot-checks`

**Smoke contract:** unchanged `shell: init.sh complete`. `make test-boot`
also passes with smoke output included.

- [ ] **Step 3.1: Audit remaining `kprintln!("ruos: ...")` calls**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && grep -rn "kprintln!.*ruos:" kernel/src/ | grep -v boot/'
```

Likely sources: `timer.rs` ("lapic calibrated", "100 Hz"), `vfs/devices.rs`, `keyboard/mod.rs`, `wasm/mod.rs` ("wasm exited cleanly"), `executor/mod.rs` ("executor up", "real ping-pong").

For each: replace `kprintln!("ruos: subsys foo bar", …)` with `binfo!("subsys", "foo bar", …)`. Keep `kprintln!` only where structured logging isn't appropriate (e.g., `wasm fiber: suspend Foo {...}` debug spam — that's not boot-time).

Example for `timer.rs`:
```rust
// before
kprintln!("ruos: lapic calibrated {} ticks/sec, periodic count={}",
    lapic_per_sec, initial_count);
// after
crate::binfo!("intr", "LAPIC {} Hz @ {} ticks/quantum",
    lapic_per_sec, initial_count);
```

(Note: this `binfo!` runs from inside `timer::init` which is now called from `boot::phases::interrupts::init`. Naming the module `"intr"` matches the phase.)

**DO NOT migrate**:
- `kprintln!("ruos: wasm fiber: ...")` — runtime debug
- `kprintln!("ruos: hello serial")` — pre-banner, pre-logger
- `kprintln!("ruos: shell.wasm exited cleanly")` — post-boot, runtime
- Anything emitted from inside wasm fiber dispatch

ONLY migrate one-shot init logs.

- [ ] **Step 3.2: Add `make test-boot` target**

In root `Makefile`, after the existing `run-test` rule, add:

```makefile
.PHONY: test-boot
test-boot:
	rm -rf build/iso_root build/os.iso
	cd kernel && cargo build --release \
		-Zbuild-std=core,compiler_builtins,alloc \
		-Zbuild-std-features=compiler-builtins-mem \
		--target ../linker.json \
		--features boot-checks
	$(MAKE) build/os.iso \
		KERNEL_ELF=kernel/target/linker/release/kernel
	@echo "--- test-boot (with smoke) ---"
	timeout 60 $(QEMU) -m 256M -no-reboot -display none -serial stdio \
		-cdrom build/os.iso \
		| tee build/test-boot.log
	@grep -qF "smoke" build/test-boot.log && grep -qF "shell: init.sh complete" build/test-boot.log
	@echo "TEST_BOOT_PASS"
```

(Adapt to actual variable names: `KERNEL_ELF`, `QEMU`, target spec — these may differ.)

- [ ] **Step 3.3: Build + test (default)**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | tail -25'
```

Expected: banner + binfo logs + shell sentinel. No smoke output (boot-checks off).

- [ ] **Step 3.4: Build + test (boot-checks)**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make test-boot 2>&1 | tail -30'
```

Expected: banner + binfo logs WITH smoke lines (`mem: alloc smoke …`, `mem: paging smoke OK`, `fs: smoke [97, 98, 99]`, etc.) + shell sentinel + `TEST_BOOT_PASS`.

- [ ] **Step 3.5: Changelog + commit**

Create `CHANGELOG/89-26-05-29-boot-refactor-cleanup.md`:

```markdown
# 89 — Boot refactor cleanup: log migration + test-boot target (T3)

**Data:** 2026-05-29

## Cosa

- Tutti i `kprintln!("ruos: <subsys> …")` di init time migrati a
  `binfo!("<subsys>", …)`. Strucutred format ovunque per boot logs.
- `kprintln!` resta per: pre-serial logs, wasm runtime debug, post-boot
  user output (es. `shell.wasm exited cleanly`).
- `Makefile` aggiunge `make test-boot` target che builda con
  `--features boot-checks` e verifica sia il sentinel `shell: init.sh
  complete` sia almeno una riga `smoke`.

## Perché

Step 3 finale del boot refactor. Convergenza format log + opt-in
smoke per test fuller.

## File toccati

- kernel/src/{timer,vfs/*,executor/*,wasm/mod}.rs (selettivo)
- Makefile
- CHANGELOG/89-26-05-29-boot-refactor-cleanup.md (nuovo)
```

```bash
git add kernel/src/ Makefile CHANGELOG/89-26-05-29-boot-refactor-cleanup.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): boot log migration + test-boot target

Migrates remaining 'ruos: <subsys> ...' init-time kprintln! calls in
timer, vfs, keyboard, executor, and wasm/mod.rs to the structured
binfo! macro under the appropriate phase module name. Runtime debug
prints (wasm fiber suspends, post-boot wasm exit logs) stay as
kprintln! since they don't belong in the boot log.

make test-boot is a new target that builds with --features
boot-checks, runs run-test, and asserts both shell: init.sh complete
and at least one 'smoke' line. CI-friendly fuller check.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-review (controller)

**Spec coverage:**
| Spec requirement | Task |
|---|---|
| Banner ASCII + version + git SHA + date | T1 |
| Structured logger `[T+…s] L mod: msg` | T1 |
| BootError enum | T1 |
| `build.rs` for env vars | T1 |
| `boot::run` 6-phase driver | T2 |
| kmain ≤ 40 lines | T2 |
| Smoke tests gated `boot-checks` | T2 |
| Init-log migration kprintln → binfo | T3 |
| `make test-boot` target | T3 |

**Type consistency**: BootError variants flow through phases as `?`. ACPI info passed via static Mutex between mem and interrupts.

**Open risks:**
- `Result<!, BootError>` may need `Infallible` shim.
- AcpiInfo Clone derive may conflict with existing trait impls.
- `console::fb_init` API may differ from snippet.

---

## After all tasks complete

1. `make build` clean.
2. `make run-test` PASS — `shell: init.sh complete`.
3. `make test-boot` PASS — sentinel + smoke lines.
4. Final whole-implementation review.
5. Non-blocking findings → `docs/followups/boot-refactor.md`.
6. Merge `feature/boot-refactor` → `main` no-ff, push, delete branch.
