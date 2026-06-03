# M2b-2 SSD installer (`install` + guard + boot-from-SSD) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** A safe `install` command (author + copy boot tree, guarded against
wiping the boot medium / a live `/mnt`) that produces an SSD which boots ruos
standalone, proven end-to-end under UEFI (OVMF).

**Architecture:** (1) reset-free AHCI port acquisition (fix the carry-forward);
(2) `/mnt` guard + `install` host fn/tool; (3) OVMF boot-from-SSD test. Design:
`docs/superpowers/specs/2026-06-03-ssd-installer-design.md`.

**Tech Stack:** Rust no_std; M2a `author` + M2b-1 `copy_boot_payload`/`mkboot`;
AHCI; OVMF (`/usr/share/OVMF/OVMF_CODE_4M.fd` + `OVMF_VARS_4M.fd`). Build via WSL.
Branch `feature/ssd-installer`. Changelog 212. End commit messages with
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Kill
stray qemu via `ps -eo pid,comm | awk '/qemu-system/{print $1}'` (NOT pgrep).

---

### Task 1: reset-free AHCI port acquisition + refactor mkdisk/mkboot

**Files:** `kernel/src/ahci/mod.rs` (+ `hba.rs`/`port.rs` if needed), `kernel/src/boot/phases/storage.rs`, `kernel/src/wasm/host/proc.rs`.

Read first: `kernel/src/ahci/mod.rs` (`init()` → `Hba::find_and_init()`, `static PORT0`), `kernel/src/ahci/hba.rs` (`Hba{abar:VirtAddr, pi:u32, cap, vs}`, the `GHC_HR` reset in `find_and_init`), `kernel/src/ahci/port.rs` (`AhciPort::bringup(abar, idx) -> Option<Self>`, `pub sectors`, `pub model`, the `Drop`/stop behavior if any), `kernel/src/boot/phases/storage.rs` (calls `ahci::init()` then `AhciPort::bringup(hba.abar, idx)`), and how `ruos_mkdisk`/`ruos_mkboot` (in `proc.rs`) currently acquire their port (they call `ahci::init()` → full HBA reset → bringup).

- [ ] **Step 1: stash the boot HBA.** In `kernel/src/ahci/mod.rs` add
```rust
use spin::Mutex; use x86_64::VirtAddr;
static BOOT_HBA: Mutex<Option<(VirtAddr, u32)>> = Mutex::new(None); // (abar, pi)
```
In `ahci::init()`, after `find_and_init()` succeeds, `*BOOT_HBA.lock() = Some((hba.abar, hba.pi));` before returning (so the already-reset, already-mapped HBA is reachable post-boot). (`init()` is one-shot at boot via storage.rs.)

- [ ] **Step 2: `acquire_port` + `sata_ports`** in `ahci/mod.rs`:
```rust
/// Bring up SATA port `idx` using the boot-time HBA — NO HBA reset (so a live
/// /mnt on another port is not orphaned). None if no HBA or no device on `idx`.
pub fn acquire_port(idx: usize) -> Option<AhciPort> {
    let (abar, _pi) = (*BOOT_HBA.lock())?;
    AhciPort::bringup(abar, idx)
}
/// Populated SATA port indices (from the boot HBA's Ports-Implemented).
pub fn sata_ports() -> alloc::vec::Vec<usize> {
    let mut v = alloc::vec::Vec::new();
    if let Some((_abar, pi)) = *BOOT_HBA.lock() {
        for idx in 0..32 { if pi & (1 << idx) != 0 { v.push(idx); } }
    }
    v
}
```

- [ ] **Step 3: refactor `ruos_mkdisk` + `ruos_mkboot`** (proc.rs) to acquire their port via `crate::ahci::acquire_port(idx)` over `crate::ahci::sata_ports()` (first port), instead of `ahci::init()`+bringup. This removes their full-HBA-reset (the orphan risk). Keep the same 0/-1/-2 returns.

- [ ] **Step 4: idempotent bringup IF needed.** `acquire_port` brings up a port the boot already brought up + dropped. If the existing mkdisk/mkboot tests (Step 5) regress (port doesn't come up cleanly — stale PxCMD.ST/FRE), make `AhciPort::bringup` stop the port first (clear `PxCMD.ST` then `PxCMD.FRE`, wait for `PxCMD.CR`/`FR` to clear) before reconfiguring. Only do this if a test actually fails — verify first.

- [ ] **Step 5: build + regression (proves reset-free acquisition works).**
```
cd kernel && cargo build --release   # clean
make run-m2a-test    → TEST_PASS_M2A    # mkdisk via acquire_port still authors
make run-m2b1-test   → TEST_PASS_M2B1   # mkboot via acquire_port still authors+copies
make run-test        → TEST_PASS
make run-gpt-test    → TEST_PASS_GPT
```
(Kill stray qemu between, comm form.) If a SATA test fails after the refactor, apply Step 4 (idempotent bringup) and re-run. These passing = the reset-free path works.

- [ ] **Step 6: commit** — `git add kernel/src/ahci kernel/src/boot/phases/storage.rs kernel/src/wasm/host/proc.rs && git commit` (msg: `feat(ahci): reset-free acquire_port (cache boot HBA) — mkdisk/mkboot no longer reset the HBA` + the Co-Authored-By trailer).

---

### Task 2: `/mnt` guard + `install` command

**Files:** `kernel/src/vfs/mod.rs`, `kernel/src/wasm/host/proc.rs`, `user/install/` (new), `Makefile`, `limine.conf`.

- [ ] **Step 1: `vfs::is_mounted`** in `kernel/src/vfs/mod.rs` (the `MOUNTS: Mutex<Vec<(String, FsImpl)>>` table):
```rust
/// True if a filesystem is mounted at exactly `prefix` (e.g. "/mnt").
pub fn is_mounted(prefix: &str) -> bool {
    MOUNTS.lock().iter().any(|(p, _)| p == prefix)
}
```

- [ ] **Step 2: `ruos_install` host fn** (proc.rs), mirror `ruos_mkboot` but guarded:
```rust
fn ruos_install(_c: Caller<'_, RuntimeState>, esp_mib: i32) -> Result<i32, Error> {
    if crate::vfs::is_mounted("/mnt") {
        crate::bwarn!("install", "refusing: /mnt is mounted — boot the installer medium to install");
        return Ok(-3);
    }
    let esp = if esp_mib <= 0 { 64 } else if esp_mib > 4096 { 4096 } else { esp_mib } as u32;
    let idx = match crate::ahci::sata_ports().first().copied() { Some(i)=>i, None=>{ crate::bwarn!("install","no SATA disk"); return Ok(-1); } };
    let mut port = match crate::ahci::acquire_port(idx) { Some(p)=>p, None=>return Ok(-1) };
    crate::binfo!("install", "target: port {} model={:?} sectors={} ({} MiB) — WIPING",
        idx, port.model, port.sectors, port.sectors / 2048);
    let layout = match crate::disk::author(&mut port, esp) { Ok(l)=>l, Err(_)=>return Ok(-2) };
    let mut e = crate::blockdev::PartBorrow::new(&mut port, layout.esp.first_lba, layout.esp.sectors);
    if crate::disk::copy_boot_payload(&mut e).is_err() { return Ok(-2); }
    crate::binfo!("install", "ok — ruos installed to port {}, reboot from the SSD", idx);
    Ok(0)
}
```
Register `.func_wrap("ruos","install", ruos_install)?`.

- [ ] **Step 3: `user/install/` tool** (mirror `user/mkboot/`): import `ruos::install(esp_mib:i32)->i32`; default 64; print `install: installing ruos to the first SATA disk (ESP {n} MiB)...`; call; on `0` → `install: ok — remove the installer medium and reboot` (token `install: ok`); on `-3` → `install: /mnt is mounted, refusing`; on `-1` → `install: no SATA disk`; else `install: failed (code N)` + exit 1. Add `install` to Makefile `BIN_TOOLS` + a `/bin/install.wasm` `limine.conf` module entry.

- [ ] **Step 4: build + in-guest smoke** — `make iso` clean; `install.wasm` in `/bin`. Quick check: boot with a blank SATA disk + an init that runs `install`, grep serial for `install: ok` AND `install: target: port` (confirms author+copy ran via the guard+acquire_port path). Reuse the m2b1 verification shape (extract ESP, mtools sees `/EFI/BOOT/BOOTX64.EFI` + `/boot/kernel`). (The full UEFI boot proof is Task 3.) `make run-test` → `TEST_PASS`.

- [ ] **Step 5: commit** — `git add kernel/src/vfs/mod.rs kernel/src/wasm/host/proc.rs user/install Makefile limine.conf && git commit` (msg: `feat(install): ruos_install host fn + install tool (/mnt guard, auto-target, author+copy)` + trailer).

---

### Task 3: boot-from-SSD OVMF test + changelog

**Files:** `tests/m2b2-test.sh` (new), `user-bin/m2b2-init.sh` (new), `Makefile`, `CHANGELOG/212-26-06-03-ssd-installer.md`.

- [ ] **Step 1: init** `user-bin/m2b2-init.sh`:
```sh
echo ruos boot OK
install
echo m2b2-installed
```
(On Phase 1 — blank disk, no `/mnt` — `install` authors+copies. On Phase 2 — booted from the SSD, M1 has mounted `/mnt` from the SSD's data partition before init — `install` prints `install: /mnt is mounted, refusing` and boot continues. Same init, both phases; the guard prevents the loop.)

- [ ] **Step 2: `tests/m2b2-test.sh`** — two phases on one disk image. Validate every parse/marker against the real tool output; iterate until it passes on a good install and fails on a bad one.
```bash
#!/usr/bin/env bash
set -u; cd "$(dirname "$0")/.."
IMG=build/m2b2-disk.img; S=build/serial.log
killq(){ ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done; }
killq; sleep 1
dd if=/dev/zero of="$IMG" bs=1M count=512 status=none
make iso INIT_SCRIPT=user-bin/m2b2-init.sh > build/m2b2-iso.log 2>&1 || { echo TEST_FAIL_ISO; tail -20 build/m2b2-iso.log; exit 1; }
# --- Phase 1: install (BIOS/ISO boot + blank SATA disk) ---
timeout 200 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom build/os.iso -serial stdio -display none -no-reboot -m 1024 -device qemu-xhci \
  -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci -device ide-hd,drive=d0,bus=ahci.0 > "$S" 2>&1 & QP=$!
for _ in $(seq 1 90); do grep -qF "install: ok" "$S" && break; kill -0 $QP 2>/dev/null||break; sleep 2; done
killq; cp "$S" build/serial.m2b2p1.log
grep -qF "install: ok" build/serial.m2b2p1.log || { echo TEST_FAIL_INSTALL; tail -30 build/serial.m2b2p1.log; exit 1; }
# --- Phase 2: boot FROM the SSD under UEFI (OVMF), NO cdrom ---
cp /usr/share/OVMF/OVMF_VARS_4M.fd build/ovmf_vars.fd
timeout 120 qemu-system-x86_64 -machine q35 -cpu max \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive if=pflash,format=raw,file=build/ovmf_vars.fd \
  -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci -device ide-hd,drive=d0,bus=ahci.0 \
  -serial stdio -display none -no-reboot -m 1024 -device qemu-xhci > "$S" 2>&1 & QP=$!
for _ in $(seq 1 55); do grep -qF "mnt mounted FAT" "$S" && break; kill -0 $QP 2>/dev/null||break; sleep 2; done
killq; cp "$S" build/serial.m2b2p2.log
grep -qF "ruos boot OK" build/serial.m2b2p2.log || { echo TEST_FAIL_SSD_BOOT; tail -40 build/serial.m2b2p2.log; exit 1; }
grep -qF "mnt mounted FAT" build/serial.m2b2p2.log || { echo TEST_FAIL_SSD_MNT; tail -40 build/serial.m2b2p2.log; exit 1; }
echo TEST_PASS_M2B2
```
**Phase 2 is the novel/risky bit.** If UEFI doesn't boot the SSD: (a) check OVMF found `/EFI/BOOT/BOOTX64.EFI` (it auto-runs the removable-media path — confirm the ESP partition type is EF00/ESP GUID, which M2a sets); (b) check Limine found `limine.conf` (it searches `/boot/limine/`, where M2b-1 wrote it) and `/boot/kernel`; (c) dump the phase-2 serial — OVMF + Limine print to it. If Limine can't find the config, the path/location may differ from what Limine's BOOTX64.EFI searches — report the serial; this may need a `limine.conf` copy at the ESP root or `/EFI/BOOT/` too (try adding it in copy_boot_payload — but that's an escalation to a kernel change, report DONE_WITH_CONCERNS). If it boots but `/mnt` doesn't mount, M1/the data partition has an issue — report.

- [ ] **Step 3: Makefile** `run-m2b2-test:` → `bash tests/m2b2-test.sh`.

- [ ] **Step 4: run ALL gates** (sequential, kill stray qemu between): `make run-m2b2-test`→`TEST_PASS_M2B2`; `run-m2b1-test`→`TEST_PASS_M2B1`; `run-m2a-test`→`TEST_PASS_M2A`; `run-gpt-test`→`TEST_PASS_GPT`; `run-test`→`TEST_PASS`.

- [ ] **Step 5: CHANGELOG** `CHANGELOG/212-26-06-03-ssd-installer.md` (Cosa: comando install + guardia /mnt + acquire_port reset-free + boot-da-SSD; Perché: capstone installer SSD persistente; File toccati; Verifica: run-m2b2-test = install in QEMU poi boot UEFI/OVMF dal SSD, ruos parte + monta /mnt; + gli altri 4 gate).

- [ ] **Step 6: commit** — `git add tests/m2b2-test.sh Makefile user-bin/m2b2-init.sh CHANGELOG/212-26-06-03-ssd-installer.md && git commit` (msg: `test(install): boot-from-SSD OVMF e2e (install -> UEFI reboot -> ruos mounts /mnt) + changelog` + trailer).

---

## Self-review (controller)

- Type consistency: `ahci::acquire_port(idx)->Option<AhciPort>` + `sata_ports()->Vec<usize>` (T1) used by `ruos_install`/`ruos_mkdisk`/`ruos_mkboot` (T1/T2); `vfs::is_mounted` (T2) used by `ruos_install`; `disk::author`/`copy_boot_payload`/`PartBorrow` (M2a/M2b-1) reused.
- T1 risk = bringup on a boot-acquired port without reset; gated by the existing mkdisk/mkboot tests still passing (Step 5). Idempotent-bringup fallback (Step 4) if needed.
- T3 risk = UEFI/OVMF actually booting the installed ESP; the debugging hooks in Step 2 cover the likely failure (Limine config location). The guard-prevents-loop design means the same init serves both phases.
