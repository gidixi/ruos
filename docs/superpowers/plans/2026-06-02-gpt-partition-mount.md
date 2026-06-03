# M1 — GPT partition parse + partition-aware mount — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Boot ruos from a GPT-partitioned SATA SSD and mount its FAT32 data partition as `/mnt` (read/write, persistent), while still mounting a raw-FAT disk (no partition table) at LBA 0 as before.

**Architecture:** A `gpt` parser reads the GPT header + entries off any `BlockDevice`. A `PartitionDevice` wraps a `BlockDevice` with a base-LBA offset so the existing FAT32 driver mounts a partition unchanged. The storage phase parses the GPT, picks the Microsoft-Basic-Data partition, and mounts it via `PartitionDevice`; if there's no GPT it falls back to mounting the whole device at LBA 0.

**Tech Stack:** Rust `no_std` kernel; existing `blockdev::BlockDevice`, `vfs::fat32`, AHCI.

**Spec:** `docs/superpowers/specs/2026-06-02-gpt-partition-mount-design.md`

**Build/test (WSL via PowerShell tool; git-bash mangles /mnt):**
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && <cmd>'`. Kernel build: `cd kernel && cargo build --release 2>&1 | tail -20`. Smoke: `touch kernel/build.rs && make run-test`. Kill stray qemu if disk locked: `ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 $p; done` (NB: `pgrep -f qemu` matches the wsl shell itself — use the `comm` form).

**Verified facts:**
- `blockdev::BlockDevice` trait: `block_size()->u32`, `block_count()->u64`, `read_blocks(lba:u64,&mut[u8])->Result<(),BlockError>`, `write_blocks(lba:u64,&[u8])->Result<(),BlockError>`. `BlockError::{Io,OutOfRange,BadAlignment,Timeout}`.
- `vfs::fat32`: `Fat32Fs::from_ahci_port(port)` boxes the port (`Box::new(port) as Box<dyn BlockDevice+Send>`) and builds the fs reading the BPB from sector 0. `mount_from_ahci_port(port)` → `vfs::mount("/mnt", FsImpl::Fat32(fs))`.
- `boot/phases/storage.rs`: walks `hba.pi` bits, `AhciPort::bringup(hba.abar, idx)`, reads sector 0 (smoke), `mount_from_ahci_port(port)`, `break`.
- Host has `sgdisk` (1.0.10) + `mkfs.vfat` + `mcopy`.

GPT on-disk type-GUID bytes (mixed-endian, as stored):
- ESP `C12A7328-F81F-11D2-BA4B-00A0C93EC93B` → `28 73 2A C1 1F F8 D2 11 BA 4B 00 A0 C9 3E C9 3B`
- MS Basic Data `EBD0A0A2-B9E5-4433-87C0-68B6B72699C7` → `A2 A0 D0 EB E5 B9 33 44 87 C0 68 B6 B7 26 99 C7`

---

## File Structure

| File | Responsibility |
|------|----------------|
| `kernel/src/gpt.rs` | NEW — parse GPT header + entries off a BlockDevice; type-GUID helpers |
| `kernel/src/blockdev.rs` | add `PartitionDevice` (base-LBA offset wrapper) |
| `kernel/src/vfs/fat32.rs` | add `mount_from_blockdev(Box<dyn BlockDevice+Send>)`; `from_blockdev` |
| `kernel/src/boot/phases/storage.rs` | GPT-parse → pick data partition → mount via PartitionDevice; LBA-0 fallback |
| `kernel/src/main.rs` | `mod gpt;` |
| `tests/gpt-test.sh`, `Makefile` | GPT test disk + `run-gpt-test`; CHANGELOG |

---

## Task 1: GPT parser (`gpt.rs`) + host unit test

**Files:** Create `kernel/src/gpt.rs`; modify `kernel/src/main.rs`.

- [ ] **Step 1: Write `gpt.rs` with the parser + `#[cfg(test)]` tests**

```rust
//! Minimal GPT (GUID Partition Table) reader. Parses the primary header at
//! LBA 1 + the partition entry array. Read-only (M1); writing is M2.

use crate::blockdev::BlockDevice;
use alloc::vec::Vec;

/// On-disk type-GUID byte layout (mixed-endian, as GPT stores it).
pub const TYPE_ESP: [u8; 16] =
    [0x28,0x73,0x2A,0xC1, 0x1F,0xF8, 0xD2,0x11, 0xBA,0x4B, 0x00,0xA0,0xC9,0x3E,0xC9,0x3B];
pub const TYPE_MS_BASIC_DATA: [u8; 16] =
    [0xA2,0xA0,0xD0,0xEB, 0xE5,0xB9, 0x33,0x44, 0x87,0xC0, 0x68,0xB6,0xB7,0x26,0x99,0xC7];

#[derive(Clone)]
pub struct GptPartition {
    pub type_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
}
impl GptPartition {
    pub fn is_esp(&self) -> bool { self.type_guid == TYPE_ESP }
    pub fn is_basic_data(&self) -> bool { self.type_guid == TYPE_MS_BASIC_DATA }
    pub fn sectors(&self) -> u64 { self.last_lba.saturating_sub(self.first_lba) + 1 }
}

fn rd_u32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o],b[o+1],b[o+2],b[o+3]]) }
fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a=[0u8;8]; a.copy_from_slice(&b[o..o+8]); u64::from_le_bytes(a)
}

/// Parse the primary GPT. Returns the non-empty partitions, or None if LBA 1
/// is not a GPT header (caller falls back to a raw FAT at LBA 0).
pub fn parse(dev: &mut dyn BlockDevice) -> Option<Vec<GptPartition>> {
    if dev.block_size() != 512 { return None; }
    let mut hdr = [0u8; 512];
    dev.read_blocks(1, &mut hdr).ok()?;
    if &hdr[0..8] != b"EFI PART" { return None; }
    let entries_lba = rd_u64(&hdr, 72);
    let num = rd_u32(&hdr, 80).min(128) as usize;     // cap at 128 (sane GPT)
    let esize = rd_u32(&hdr, 84) as usize;
    if !(128..=512).contains(&esize) || num == 0 { return None; }
    // Read the entry array sector-by-sector (esize divides 512 for std 128).
    let per_sec = 512 / esize;
    if per_sec == 0 { return None; }
    let mut out = Vec::new();
    let mut sec = [0u8; 512];
    let sectors_needed = (num + per_sec - 1) / per_sec;
    for s in 0..sectors_needed {
        if dev.read_blocks(entries_lba + s as u64, &mut sec).is_err() { break; }
        for i in 0..per_sec {
            let idx = s * per_sec + i;
            if idx >= num { break; }
            let e = &sec[i*esize .. i*esize + esize];
            let mut tg = [0u8;16]; tg.copy_from_slice(&e[0..16]);
            if tg == [0u8;16] { continue; } // empty entry
            out.push(GptPartition { type_guid: tg, first_lba: rd_u64(e,32), last_lba: rd_u64(e,40) });
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// First Microsoft-Basic-Data partition (ruos data partition), if any.
pub fn find_data(parts: &[GptPartition]) -> Option<&GptPartition> {
    parts.iter().find(|p| p.is_basic_data())
}

#[cfg(test)]
mod tests {
    use super::*; extern crate std; use std::vec; use std::vec::Vec as SVec;
    use std::boxed::Box;

    // In-memory BlockDevice over a Vec of 512-byte sectors.
    struct MemDev(SVec<u8>);
    impl crate::blockdev::BlockDevice for MemDev {
        fn block_size(&self)->u32 {512}
        fn block_count(&self)->u64 {(self.0.len()/512) as u64}
        fn read_blocks(&mut self,lba:u64,buf:&mut[u8])->Result<(),crate::blockdev::BlockError>{
            let o=(lba as usize)*512;
            if o+buf.len()>self.0.len(){return Err(crate::blockdev::BlockError::OutOfRange);}
            buf.copy_from_slice(&self.0[o..o+buf.len()]); Ok(())
        }
        fn write_blocks(&mut self,lba:u64,buf:&[u8])->Result<(),crate::blockdev::BlockError>{
            let o=(lba as usize)*512; self.0[o..o+buf.len()].copy_from_slice(buf); Ok(())
        }
    }

    fn synth() -> MemDev {
        let mut d = vec![0u8; 512*40];
        // LBA1 header
        d[512..520].copy_from_slice(b"EFI PART");
        d[512+72..512+80].copy_from_slice(&2u64.to_le_bytes());   // entries at LBA 2
        d[512+80..512+84].copy_from_slice(&128u32.to_le_bytes()); // 128 entries
        d[512+84..512+88].copy_from_slice(&128u32.to_le_bytes()); // 128 bytes each
        // entry 0 @ LBA2 byte0: ESP, first=34 last=2047
        let e0 = 2*512;
        d[e0..e0+16].copy_from_slice(&TYPE_ESP);
        d[e0+32..e0+40].copy_from_slice(&34u64.to_le_bytes());
        d[e0+40..e0+48].copy_from_slice(&2047u64.to_le_bytes());
        // entry 1: MS basic data, first=2048 last=4095
        let e1 = e0+128;
        d[e1..e1+16].copy_from_slice(&TYPE_MS_BASIC_DATA);
        d[e1+32..e1+40].copy_from_slice(&2048u64.to_le_bytes());
        d[e1+40..e1+48].copy_from_slice(&4095u64.to_le_bytes());
        MemDev(d)
    }

    #[test] fn parses_two_parts() {
        let mut d = synth();
        let p = parse(&mut d).unwrap();
        assert_eq!(p.len(), 2);
        assert!(p[0].is_esp());
        assert!(p[1].is_basic_data());
        assert_eq!(p[1].first_lba, 2048);
        assert_eq!(p[1].sectors(), 2048);
    }
    #[test] fn no_gpt_is_none() {
        let mut d = MemDev(vec![0u8; 512*4]); // no "EFI PART"
        assert!(parse(&mut d).is_none());
    }
    #[test] fn find_data_picks_basic() {
        let mut d = synth();
        let p = parse(&mut d).unwrap();
        let data = find_data(&p).unwrap();
        assert_eq!(data.first_lba, 2048);
    }
}
```

- [ ] **Step 2: `mod gpt;`** — add to `kernel/src/main.rs` near the other `mod` lines.

- [ ] **Step 3: Host-test the parser** (the kernel can't `cargo test`; copy `gpt.rs` + a minimal `blockdev` stub into a host crate). Simplest: the test uses `crate::blockdev` types — so run the test by temporarily building a tiny host crate that `include!`s both. Pragmatic command:
```
wsl ... 'cd /tmp && rm -rf gt && mkdir -p gt/src && \
  printf "[package]\nname=\"gt\"\nversion=\"0.0.0\"\nedition=\"2021\"\n" > gt/Cargo.toml && \
  { echo "extern crate alloc;"; echo "pub mod blockdev { include!(\"/mnt/e/MinimalOS/BasicOperatingSystem/kernel/src/blockdev.rs\"); }"; echo "pub mod gpt { include!(\"/mnt/e/MinimalOS/BasicOperatingSystem/kernel/src/gpt.rs\"); }"; } > gt/src/lib.rs && \
  cd gt && cargo test 2>&1 | tail -12'
```
Expected: `test result: ok. 3 passed`. (If `blockdev.rs` pulls kernel-only imports that break the host build, instead copy just the `BlockDevice` trait + `BlockError` into the test module — but `blockdev.rs` is self-contained `use core::fmt`, so the include should compile on host. If it doesn't, report and we adjust.)

- [ ] **Step 4: Kernel build** — `cd kernel && cargo build --release 2>&1 | tail -5` → `Finished` (gpt fns may warn dead_code until Task 4 — acceptable).

- [ ] **Step 5: Commit** — `git add kernel/src/gpt.rs kernel/src/main.rs && git commit -m "feat(gpt): GPT header + partition entry parser (+ host tests)"`.

---

## Task 2: `PartitionDevice` offset wrapper + host test

**Files:** Modify `kernel/src/blockdev.rs`.

- [ ] **Step 1: Add `PartitionDevice` + a test** to `kernel/src/blockdev.rs` (after the trait):

```rust
extern crate alloc;
use alloc::boxed::Box;

/// A `BlockDevice` view of one partition: every LBA is offset by `base`, and
/// the device length is clamped to `count` sectors. Lets the FAT32 driver mount
/// a partition unchanged (it reads "LBA 0" = the partition's first sector).
pub struct PartitionDevice {
    inner: Box<dyn BlockDevice + Send>,
    base: u64,
    count: u64,
}

impl PartitionDevice {
    pub fn new(inner: Box<dyn BlockDevice + Send>, base: u64, count: u64) -> Self {
        Self { inner, base, count }
    }
}

impl BlockDevice for PartitionDevice {
    fn block_size(&self) -> u32 { self.inner.block_size() }
    fn block_count(&self) -> u64 { self.count }
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let n = (buf.len() as u64) / self.inner.block_size() as u64;
        if lba + n > self.count { return Err(BlockError::OutOfRange); }
        self.inner.read_blocks(self.base + lba, buf)
    }
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let n = (buf.len() as u64) / self.inner.block_size() as u64;
        if lba + n > self.count { return Err(BlockError::OutOfRange); }
        self.inner.write_blocks(self.base + lba, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*; extern crate std; use std::vec; use std::vec::Vec;
    struct Mem(Vec<u8>);
    impl BlockDevice for Mem {
        fn block_size(&self)->u32{512}
        fn block_count(&self)->u64{(self.0.len()/512) as u64}
        fn read_blocks(&mut self,lba:u64,buf:&mut[u8])->Result<(),BlockError>{
            let o=(lba as usize)*512; buf.copy_from_slice(&self.0[o..o+buf.len()]); Ok(())
        }
        fn write_blocks(&mut self,lba:u64,buf:&[u8])->Result<(),BlockError>{
            let o=(lba as usize)*512; self.0[o..o+buf.len()].copy_from_slice(buf); Ok(())
        }
    }
    #[test] fn offsets_and_clamps() {
        let mut backing = vec![0u8; 512*10];
        backing[5*512] = 0xAB; // sector 5 marker
        let mut pd = PartitionDevice::new(Box::new(Mem(backing)), 5, 3); // base 5, 3 sectors
        let mut buf = [0u8;512];
        pd.read_blocks(0, &mut buf).unwrap();     // → backing sector 5
        assert_eq!(buf[0], 0xAB);
        assert!(pd.read_blocks(3, &mut buf).is_err()); // past count=3 → OutOfRange
        assert_eq!(pd.block_count(), 3);
    }
}
```
(If `blockdev.rs` already has `extern crate alloc;` / `use alloc::boxed::Box;`, don't duplicate — add only what's missing.)

- [ ] **Step 2: Host-test** — reuse the Task-1 host-crate trick on `blockdev.rs` alone:
```
wsl ... 'cd /tmp && rm -rf bt && mkdir -p bt/src && printf "[package]\nname=\"bt\"\nversion=\"0.0.0\"\nedition=\"2021\"\n" > bt/Cargo.toml && { echo "extern crate alloc;"; echo "include!(\"/mnt/e/MinimalOS/BasicOperatingSystem/kernel/src/blockdev.rs\");"; } > bt/src/lib.rs && cd bt && cargo test 2>&1 | tail -8'
```
Expected: `test result: ok. 1 passed`.

- [ ] **Step 3: Kernel build** → `Finished` (PartitionDevice dead_code until Task 4 — ok).
- [ ] **Step 4: Commit** — `git add kernel/src/blockdev.rs && git commit -m "feat(blockdev): PartitionDevice base-LBA offset wrapper (+test)"`.

---

## Task 3: `fat32::mount_from_blockdev`

**Files:** Modify `kernel/src/vfs/fat32.rs`.

- [ ] **Step 1:** Read the existing `Fat32Fs::from_ahci_port` (search `from_ahci_port`) and `mount_from_ahci_port`. They box the port into `Box<dyn BlockDevice+Send>` then build the fs from the BPB at sector 0. Generalise:

```rust
/// Build + mount a FAT32 volume on any block device at /mnt.
pub fn mount_from_blockdev(dev: alloc::boxed::Box<dyn crate::blockdev::BlockDevice + Send>) -> Result<(), VfsError> {
    let fs = Fat32Fs::from_blockdev(dev)?;
    crate::vfs::mount("/mnt", crate::vfs::fs::FsImpl::Fat32(fs))?;
    Ok(())
}
```
And refactor `Fat32Fs::from_ahci_port(port)` to box the port and call a new `from_blockdev(dev)` that holds the existing BPB-reading logic:
```rust
pub fn from_ahci_port(port: AhciPort) -> Result<Self, VfsError> {
    Self::from_blockdev(alloc::boxed::Box::new(port))
}
pub fn from_blockdev(dev: alloc::boxed::Box<dyn crate::blockdev::BlockDevice + Send>) -> Result<Self, VfsError> {
    // ... the current from_ahci_port body, but starting from `dev` already boxed
    // (the line `dev: Box::new(port) as Box<...>` becomes just `dev`).
}
```
Keep `mount_from_ahci_port` working (used elsewhere? grep — storage.rs is the only caller; Task 4 switches it). If nothing else calls it, you may leave it or remove it; keep it for now to minimise churn.

- [ ] **Step 2: Kernel build** → `Finished`.
- [ ] **Step 3: Commit** — `git add kernel/src/vfs/fat32.rs && git commit -m "feat(fat32): mount_from_blockdev (generalise mount over any BlockDevice)"`.

---

## Task 4: GPT-aware storage phase + LBA-0 fallback

**Files:** Modify `kernel/src/boot/phases/storage.rs`.

- [ ] **Step 1:** Replace the mount block. For each populated port, after the sector-0 smoke read, parse the GPT and mount accordingly:

```rust
if let Some(mut port) = crate::ahci::AhciPort::bringup(hba.abar, idx as usize) {
    // (keep the existing sector-0 smoke read/log here)

    // Parse the GPT; if present, mount the data partition; else raw FAT at LBA 0.
    let mounted = match crate::gpt::parse(&mut port) {
        Some(parts) => match crate::gpt::find_data(&parts) {
            Some(d) => {
                let base = d.first_lba;
                let count = d.sectors();
                crate::binfo!("storage", "gpt: data part lba={} sectors={} -> /mnt", base, count);
                let pd = crate::blockdev::PartitionDevice::new(
                    alloc::boxed::Box::new(port), base, count);
                crate::vfs::fat32::mount_from_blockdev(alloc::boxed::Box::new(pd))
            }
            None => {
                crate::bwarn!("storage", "gpt present but no data partition; trying LBA 0");
                crate::vfs::fat32::mount_from_blockdev(alloc::boxed::Box::new(port))
            }
        },
        None => crate::vfs::fat32::mount_from_blockdev(alloc::boxed::Box::new(port)),
    };
    match mounted {
        Ok(())  => crate::binfo!("fat32", "mnt mounted FAT"),
        Err(e)  => crate::bwarn!("fat32", "mount /mnt failed: {}", e),
    }
    break;
}
```
NOTE: `port` is borrowed by `gpt::parse(&mut port)` then moved into `Box::new(port)` after the borrow ends — fine. If the borrow checker complains, bind the parse result to a `Vec`/`Option` first (parse returns owned data), drop the borrow, then move `port`.

- [ ] **Step 2: Build + smoke (raw-FAT regression)** — `touch kernel/build.rs && make run-test 2>&1 | tail -2`. Expected `TEST_PASS`: the existing raw-FAT `disk.img` (no GPT) hits the `None` fallback → `mnt mounted FAT` + `hello from disk` still pass. Confirm `grep -E "mnt mounted FAT|hello from disk" build/serial.log`.
- [ ] **Step 3: Commit** — `git add kernel/src/boot/phases/storage.rs && git commit -m "feat(storage): GPT-aware /mnt mount (data partition) + LBA-0 fallback"`.

---

## Task 5: GPT test disk + `run-gpt-test` + changelog

**Files:** Create `tests/gpt-test.sh`; modify `Makefile`; CHANGELOG.

- [ ] **Step 1: `tests/gpt-test.sh`** — build a GPT disk (ESP + MS-basic-data with a marker file), boot QEMU with it as the only AHCI disk, assert ruos mounts the data partition + reads the marker. No loop devices (format a FAT file, dd it into the partition offset):

```bash
#!/usr/bin/env bash
set -u
cd "$(dirname "$0")/.."
IMG=build/gpt-disk.img
ISO=build/os.iso
SERIAL=build/serial.log
ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done
sleep 1
# 64 MiB GPT disk: ESP (1 MiB) + MS basic data (rest).
dd if=/dev/zero of="$IMG" bs=1M count=64 status=none
sgdisk -n 1:2048:+1M -t 1:EF00 -c 1:EFI \
       -n 2:0:0      -t 2:0700 -c 2:ruos-data "$IMG" >/dev/null
# Data partition start LBA (sgdisk -i 2 → "First sector: N").
DLBA=$(sgdisk -i 2 "$IMG" | awk '/First sector/{print $3}')
DSECS=$(( ( $(stat -c%s "$IMG") / 512 ) - DLBA - 34 ))   # leave room for backup GPT
# Build a FAT in a standalone file sized to the data partition, add a marker.
KB=$(( DSECS / 2 ))
mkfs.vfat -C build/data.fat "$KB" >/dev/null
printf 'gpt-persist-ok\n' > build/marker.txt
mcopy -o -i build/data.fat build/marker.txt ::/GPTHELLO.TXT
# Splice the FAT into the data partition offset.
dd if=build/data.fat of="$IMG" bs=512 seek="$DLBA" conv=notrunc status=none
# Boot: GPT disk as the (only) AHCI disk; smoke.sh runs `cat /mnt/GPTHELLO.TXT`.
timeout 120 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -drive file="$IMG",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 > "$SERIAL" 2>&1 &
QP=$!; sleep 18
ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done
grep -qE "gpt: data part lba=" "$SERIAL" || { echo TEST_FAIL_GPT_PARSE; exit 1; }
grep -qF "gpt-persist-ok" "$SERIAL" || { echo TEST_FAIL_GPT_READ; exit 1; }
echo TEST_PASS_GPT
```

- [ ] **Step 2: smoke.sh marker** — add `cat /mnt/GPTHELLO.TXT` near the `/mnt` checks in `user-bin/smoke.sh` (so the GPT test's `gpt-persist-ok` reaches serial; the existing `cat /mnt/hello.txt` is for the raw-FAT run-test where `/mnt/GPTHELLO.TXT` is absent → harmless "not found"). Guard so run-test doesn't fail on its absence: use `cat /mnt/GPTHELLO.TXT 2>/dev/null; true` (or a line that only the gpt-test asserts).

- [ ] **Step 3: Makefile target**
```make
.PHONY: run-gpt-test
run-gpt-test: iso
	bash tests/gpt-test.sh
```

- [ ] **Step 4: Build iso + run both gates**
```
wsl ... 'touch kernel/build.rs && make iso >/tmp/iso.log 2>&1 && echo ISO_OK && \
  bash tests/gpt-test.sh 2>&1 | grep TEST_ && \
  make run-test 2>&1 | tail -1'
```
Expected: `TEST_PASS_GPT` (GPT data partition mounted + marker read) AND `TEST_PASS` (raw-FAT fallback regression).

- [ ] **Step 5: Changelog + commit** — `CHANGELOG/NNN-26-06-02-gpt-partition-mount.md` (next number via `ls CHANGELOG | grep -oE '^[0-9]+' | sort -n | tail -1`). `git add tests/gpt-test.sh Makefile user-bin/smoke.sh CHANGELOG/ && git commit -m "test(storage): GPT partition mount disk test + changelog"`.

---

## Final review

Dispatch a reviewer over the branch diff (focus: GPT header-field bounds/validation, PartitionDevice range checks, the storage-phase borrow-then-move of `port`, no regression on raw-FAT fallback). Then `superpowers:finishing-a-development-branch`. Do NOT merge without explicit user approval (CLAUDE.md). This is M1; M2 (the self-installer write-side) is the next spec.

## Self-review notes
- **Spec coverage:** GPT parse (T1), PartitionDevice (T2), mount_from_blockdev (T3), GPT-aware storage + LBA-0 fallback (T4), GPT test disk + raw-FAT regression (T5). Type GUIDs, "first Basic-Data partition", fallback — all covered.
- **Type consistency:** `GptPartition{type_guid,first_lba,last_lba}` + `is_esp/is_basic_data/sectors`; `gpt::parse`/`find_data`; `PartitionDevice::new(Box,base,count)`; `fat32::mount_from_blockdev(Box)`; consistent across T1-T4.
- **Risks flagged inline:** host-test include! of blockdev.rs/gpt.rs (T1/T2 — fallback to copying the trait if the include doesn't compile on host); borrow-then-move of `port` in storage.rs (T4 note); smoke.sh marker must not break run-test (T2/T5 — guard with `2>/dev/null; true`).
- **YAGNI:** read-only, single data partition, no ESP mount, no GPT write (all M2).
```