# Rust Kernel Heap + Global Allocator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `#[global_allocator]` for the Rust kernel backed by 4 MiB of real RAM (Limine memory map + HHDM offset), so `alloc::{Box,Vec,String,BTreeMap}` work; verify at boot with a serial-logged smoke test.

**Architecture:** A new `kernel/src/memory.rs` declares a `Talck<spin::Mutex<()>, ErrOnOom>` global allocator and an `init_heap()` that reads Limine's `MemoryMapRequest` + `HhdmRequest` responses, picks the first usable entry of at least `HEAP_SIZE`, and claims it. `kmain` calls `init_heap()` after serial init, logs the outcome, and runs a `Box`/`Vec` smoke test whose output is asserted by `make run-test`.

**Tech Stack:** Rust nightly (pinned `nightly-2026-05-26`), crate `talc` (~4.x), crate `spin` (0.9.x), existing `limine` (0.6.3), `uart_16550` (0.3.x). All build/run via WSL Ubuntu.

---

## Key facts

- All build/run runs in **WSL Ubuntu** as root, sourcing `$HOME/.cargo/env`:
  ```
  wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
  ```
  Edit files with Edit/Write on Windows paths. Git in the normal shell. Branch `feature/rust-heap-allocator`. Do not push, do not skip hooks.
- The kernel currently boots, prints `MinimalOS-rs: hello serial`, and halts. `build-std` already includes `alloc`.
- The limine crate is **0.6.3**, base revision 6; Limine bootloader pinned at **v11.4.1-binary**. Memory-map and HHDM are part of the same protocol.
- **Integration risk:** the exact `talc` API (Talck/Talc/ClaimOnOom/ErrOnOom/Span) and the limine 0.6.3 type/path for `MemoryMapRequest` / `HhdmRequest` / the USABLE entry-type constant may differ from the canonical code below. If `cargo build` reports a mismatch, make the minimal adaptation and report what you changed; do not redesign. The `extern "C" fn kmain`/`hcf`/panic structure stays.
- **Spec:** `docs/superpowers/specs/2026-05-28-rust-heap-allocator-design.md`.

## File structure

- `kernel/Cargo.toml` — add `talc` and `spin` deps (modify).
- `kernel/src/memory.rs` — new module: allocator static + `HEAP_SIZE` + `init_heap()` + `HeapInfo`/`HeapInitError`.
- `kernel/src/main.rs` — `extern crate alloc;`, `mod memory;`, new Limine request statics, `kmain` wires `init_heap` + smoke test (modify).
- `Makefile` — change the asserted serial line to the alloc-test signature (modify).

---

## Task 1: Add deps, declare global allocator, enable `alloc`

**Files:**
- Modify: `kernel/Cargo.toml`
- Create: `kernel/src/memory.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/16-26-05-28-rust-heap-allocator.md`

- [ ] **Step 1: Add dependencies to `kernel/Cargo.toml`**

In the `[dependencies]` table, add (alongside the existing `limine` and `uart_16550` lines):

```toml
talc = "4"
spin = "0.9"
```

(If `talc = "4"` does not resolve, use the latest published `4.x` — or the latest published whichever major if the API has shifted — and record the resolved version in the changelog.)

- [ ] **Step 2: Create `kernel/src/memory.rs` with the global allocator declaration**

Create `kernel/src/memory.rs`:

```rust
//! Kernel heap: global allocator (talc) and `init_heap()` (added in Task 2).
//!
//! Backing memory comes from a region described by Limine's memory map,
//! accessed virtually via Limine's HHDM offset.

use talc::{ErrOnOom, Talc, Talck};

/// Heap size in bytes: 4 MiB.
pub const HEAP_SIZE: usize = 4 * 1024 * 1024;

#[global_allocator]
pub static ALLOCATOR: Talck<spin::Mutex<()>, ErrOnOom> = Talc::new(ErrOnOom).lock();
```

(If the resolved `talc` exposes `Talck` / `ErrOnOom` under different paths, adjust the `use` line minimally; `Talc::new(ErrOnOom).lock()` is the canonical const-constructor pattern across recent talc 4.x.)

- [ ] **Step 3: Wire the module + enable `alloc` in `kernel/src/main.rs`**

In `kernel/src/main.rs`, add to the very top (after `#![no_std]` and `#![no_main]`):

```rust
extern crate alloc;
```

And immediately after the existing `mod serial;` line, add:

```rust
mod memory;
```

Do not change anything else in `main.rs` in this task; `kmain` still only does the existing hello/halt.

- [ ] **Step 4: Build the kernel to confirm green**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && cargo build 2>&1 | tail -10'
```
Expected: `Finished`. If `Talck`/`ErrOnOom` import fails against the resolved talc, adjust the `use` line minimally and re-build. Do not change behavior beyond making the names resolve.

- [ ] **Step 5: Confirm the kernel still boots (allocator declared but never invoked)**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -4'
```
Expected: `TEST_PASS` (the existing hello-line assertion still holds because the allocator is declared but no allocation happens yet).

- [ ] **Step 6: Write the changelog entry**

Create `CHANGELOG/16-26-05-28-rust-heap-allocator.md`:

```markdown
# 16 — Global allocator dichiarato (talc) + alloc abilitato

**Data:** 2026-05-28

## Cosa
- Aggiunte deps talc + spin a kernel/Cargo.toml.
- Nuovo modulo kernel/src/memory.rs: ALLOCATOR statico Talck con ErrOnOom,
  costante HEAP_SIZE (4 MiB).
- kernel/src/main.rs: `extern crate alloc;` + `mod memory;`.
- Allocator dichiarato ma non ancora inizializzato; il kernel continua a fare
  solo "hello serial" + halt.

## Perché
Primo passo dello Step 4 della roadmap: rendere disponibile il global allocator
per `alloc` (Box/Vec/String/BTreeMap); l'inizializzazione effettiva arriva
nel task successivo.

## File toccati
- kernel/Cargo.toml
- kernel/src/memory.rs
- kernel/src/main.rs
- CHANGELOG/16-26-05-28-rust-heap-allocator.md
```

- [ ] **Step 7: Commit**

```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/memory.rs kernel/src/main.rs \
        CHANGELOG/16-26-05-28-rust-heap-allocator.md
git commit -m "feat(rust): declare talc global allocator and enable alloc"
```

---

## Task 2: `init_heap` + Limine requests + boot-time smoke test

**Files:**
- Modify: `kernel/src/memory.rs`
- Modify: `kernel/src/main.rs`
- Modify: `Makefile`
- Create: `CHANGELOG/17-26-05-28-heap-init-smoke-test.md`

- [ ] **Step 1: Add the heap init API to `kernel/src/memory.rs`**

Append to `kernel/src/memory.rs` (after the existing items):

```rust
use core::fmt;
use limine::memory_map::EntryType;
use talc::Span;

/// The actual `MemoryMapRequest` / `HhdmRequest` statics live in `main.rs` so they
/// sit next to the other Limine `.requests` items and inside the existing markers.
/// This module reads them via the `crate::` path.

#[derive(Debug, Copy, Clone)]
pub struct HeapInfo {
    pub phys_base: u64,
    pub virt_base: u64,
    pub size: usize,
}

#[derive(Debug, Copy, Clone)]
pub enum HeapInitError {
    NoMemoryMap,
    NoHhdm,
    NoUsableRegion,
    ClaimFailed,
}

impl fmt::Display for HeapInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeapInitError::NoMemoryMap     => f.write_str("no memory map"),
            HeapInitError::NoHhdm          => f.write_str("no hhdm"),
            HeapInitError::NoUsableRegion  => f.write_str("no usable region"),
            HeapInitError::ClaimFailed     => f.write_str("claim"),
        }
    }
}

pub fn init_heap() -> Result<HeapInfo, HeapInitError> {
    let memmap = crate::MEMMAP_REQUEST.get_response().ok_or(HeapInitError::NoMemoryMap)?;
    let hhdm   = crate::HHDM_REQUEST.get_response().ok_or(HeapInitError::NoHhdm)?;
    let hhdm_offset = hhdm.offset();

    let entry = memmap.entries()
        .iter()
        .find(|e| e.entry_type == EntryType::USABLE && (e.length as usize) >= HEAP_SIZE)
        .ok_or(HeapInitError::NoUsableRegion)?;

    let phys_base = entry.base;
    let virt_base = phys_base + hhdm_offset;

    unsafe {
        ALLOCATOR
            .lock()
            .claim(Span::from_base_size(virt_base as *mut u8, HEAP_SIZE))
            .map_err(|_| HeapInitError::ClaimFailed)?;
    }

    Ok(HeapInfo { phys_base, virt_base, size: HEAP_SIZE })
}
```

(API integration notes: in limine 0.6.3 the entry struct exposes `base`, `length`, and `entry_type: EntryType`; `EntryType::USABLE` is the constant; `HhdmResponse::offset()` returns the HHDM offset. If a method is named differently in the resolved version — e.g., `entries: &[Entry]` vs an accessor, or `EntryType::Usable` capitalization — adjust minimally. `Span::from_base_size(*mut u8, usize)` is in talc 4.x; if absent, use the equivalent constructor and report.)

- [ ] **Step 2: Add the Limine request statics to `kernel/src/main.rs`**

In `kernel/src/main.rs`, immediately after the existing `static BASE_REVISION: ...` block (and inside the marker bracket — the existing markers stay around them):

```rust
use limine::request::{HhdmRequest, MemoryMapRequest};

#[used]
#[link_section = ".requests"]
pub static MEMMAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[link_section = ".requests"]
pub static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();
```

Keep `_START_MARKER` above and `_END_MARKER` below these — they bracket the whole request set. (If the existing markers are not bracketing all requests after this addition, move them so they do.)

- [ ] **Step 3: Wire init + smoke test in `kmain`**

Replace the body of `kmain` in `kernel/src/main.rs` (currently ending with `hcf()`) with the new sequence. The full new `kmain` reads:

```rust
#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    use core::fmt::Write;
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    // Serial first: any failure below must be observable on the wire.
    let mut serial = serial::Serial::new();
    serial.init();
    let _ = serial.write_str("MinimalOS-rs: hello serial\n");

    if !BASE_REVISION.is_supported() {
        let _ = serial.write_str("MinimalOS-rs: unsupported Limine base revision\n");
        hcf();
    }

    // Heap init.
    let info = match memory::init_heap() {
        Ok(info) => info,
        Err(e) => {
            let _ = writeln!(serial, "MinimalOS-rs: heap fail: {}", e);
            hcf();
        }
    };
    let _ = writeln!(
        serial,
        "MinimalOS-rs: heap ok base=0x{:X} size={}",
        info.virt_base, info.size
    );

    // Smoke test: prove Box and Vec work through the global allocator.
    let b = Box::new(0xCAFEBABEu64);
    let v: Vec<u32> = (0..5).collect();
    let _ = writeln!(
        serial,
        "MinimalOS-rs: alloc box=0x{:X} vec={:?}",
        *b, v
    );

    hcf();
}
```

(`alloc::boxed::Box` and `alloc::vec::Vec` require `extern crate alloc;` from Task 1 — already added.)

- [ ] **Step 4: Update the run-test assertion in `Makefile`**

Change the `HELLO` variable (and the only thing the `run-test` recipe `grep -q`s) to the alloc-test signature, since reaching the alloc line proves the hello line printed first. In `Makefile`, replace the existing line:

```make
HELLO     := MinimalOS-rs: hello serial
```

with:

```make
HELLO     := MinimalOS-rs: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]
```

The variable name is kept for diff minimality; its meaning is now "the full-success signature." The recipe `grep -q "$(HELLO)" build/serial.log` already uses literal matching, which is correct for these characters.

- [ ] **Step 5: Build and run the boot test**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -8'
```
Expected serial log (captured in `build/serial.log`):
```
MinimalOS-rs: hello serial
MinimalOS-rs: heap ok base=0x... size=4194304
MinimalOS-rs: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]
```
And the final line of make output is `TEST_PASS`. If `TEST_FAIL`, inspect `build/serial.log`; common causes: limine request type/path mismatch, an EntryType name mismatch, missing `entries()` accessor, talc `claim` signature drift, or a panic before serial init. Report and adapt minimally.

- [ ] **Step 6: Write the changelog entry**

Create `CHANGELOG/17-26-05-28-heap-init-smoke-test.md`:

```markdown
# 17 — Heap init (Limine memmap + HHDM) + smoke test alloc

**Data:** 2026-05-28

## Cosa
- kernel/src/memory.rs: HeapInfo, HeapInitError + Display, init_heap() che legge
  MemoryMapRequest e HhdmRequest, sceglie il primo entry USABLE >= HEAP_SIZE,
  calcola virt_base = phys_base + hhdm_offset e fa claim su talc.
- kernel/src/main.rs: nuove richieste Limine MEMMAP_REQUEST e HHDM_REQUEST
  (sezione .requests, bracketed dai marker esistenti); kmain inizializza la
  seriale, controlla base revision, chiama init_heap, logga base+size, esegue
  smoke test Box::new(0xCAFEBABE) + Vec::from(0..5) e logga risultati.
- Makefile: HELLO assert aggiornato alla riga "alloc box=0xCAFEBABE
  vec=[0, 1, 2, 3, 4]" (hello rimane prerequisito implicito).

## Perché
Completa lo Step 4 della roadmap: heap kernel funzionante, alloc utilizzabile da
tutti gli strati successivi.

## File toccati
- kernel/src/memory.rs
- kernel/src/main.rs
- Makefile
- CHANGELOG/17-26-05-28-heap-init-smoke-test.md
```

- [ ] **Step 7: Commit**

```bash
git add kernel/src/memory.rs kernel/src/main.rs Makefile \
        CHANGELOG/17-26-05-28-heap-init-smoke-test.md
git commit -m "feat(rust): heap init from Limine memmap+HHDM and alloc smoke test"
```

---

## Notes for the implementer

- **All build/run via WSL** with `source $HOME/.cargo/env` first. The pinned nightly is `nightly-2026-05-26`.
- **Adapt to resolved APIs, do not redesign.** Both `talc` and `limine` may differ from the canonical code in path/method names. Make the smallest change that compiles and boots, and report exactly what you changed.
- **The hello line is no longer the asserted string.** That is intentional: success is now "reached the alloc test." If the alloc line is absent, the failure mode is somewhere between Limine-loaded-kernel and `Vec::collect()` returning — read `build/serial.log` to localize.
- **Allocator before init.** Task 1 declares `#[global_allocator]` but never allocates. Any accidental `alloc` between Task 1 and Task 2 would deref a zero arena and likely fault. The plan therefore puts the only allocator users (Box/Vec in kmain) in Task 2, after `init_heap()`.
