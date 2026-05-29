# Step 15 — AHCI / SATA + FAT persistente: Design Spec + Implementation Plan

**Date:** 2026-05-29
**Milestone:** Roadmap Step 15. End state: kernel can read **and** write a SATA
disk over AHCI (polling), mounts a FAT filesystem on it, surfaces it under
`/mnt` in the existing VFS alongside tmpfs.
**Status:** Spec + plan combined, ready for execution.

> **For agentic workers:** the "Implementation Plan" half uses checkbox
> (`- [ ]`) task tracking. Use superpowers:subagent-driven-development or
> superpowers:executing-plans to execute task-by-task.

---

## Context

Step 7 gave the VFS + tmpfs (RAM only). Step 13 (PCI/ECAM) + Step 14 (DMA
allocator, `map_io_range`, `frames::allocate_contiguous`) give us the building
blocks AHCI needs. Step 15 adds the first persistent storage.

The kernel currently boots from Limine modules (initrd-style): `init.wasm`,
`shell.wasm`, `/etc/init.sh`, `/bin/*.wasm`. Tmpfs at `/` holds them at runtime.
Nothing survives reboot.

After Step 15:
- A SATA disk attached over AHCI is detected, IDENTIFY-DEVICE'd, exposed as a
  `BlockDevice` (512-byte sectors, LBA48).
- `fatfs` mounts a FAT16/32 filesystem on it.
- The VFS gets a second mount at `/mnt` backed by the FAT volume. Writes
  persist across reboots.
- A QEMU disk image (`build/disk.img`) is created by `make iso` (or a separate
  target), formatted FAT, populated with a known marker file. `make run-test`
  asserts the kernel reads the marker after mounting.

Initrd → disk swap (loading `/bin/*.wasm` from FAT instead of Limine modules)
is **out of scope** here — covered by a follow-up step.

---

## Goals

- `kernel/src/blockdev.rs`: a `BlockDevice` trait abstracting any 512-byte
  sector storage (AHCI port, future NVMe/virtio-blk).
- `kernel/src/ahci/`: HBA discovery (PCI class 0x01/0x06/0x01), ABAR bring-up
  (`CAP`, `GHC`, `PI`, `IS`), per-port engine setup (Command List + FIS
  Receive + Command Tables in contiguous DMA), `IDENTIFY DEVICE`, `READ DMA
  EXT` + `WRITE DMA EXT` (LBA48), polled completion on `PxCI`/`PxIS`.
- `kernel/src/fs/fatmount.rs`: a `fatfs::OemCpConverter`-free, `no_std`
  bridge from `BlockDevice` to `fatfs::ReadWriteSeek` + mount the result at
  `/mnt` via the existing `vfs::FileSystem` trait (Step 7).
- `Makefile`: create + format `build/disk.img` (64 MiB raw, FAT32), prepopulate
  with `hello.txt`, attach to QEMU as `-drive if=none,id=disk0 -device ahci
  -device ide-hd,drive=disk0,bus=ahci.0`.
- `make run-test` gains a per-step gate: `disk read OK` and `mnt mounted FAT`
  serial lines + a `cat /mnt/hello.txt` smoke in `init.sh`.

## Non-goals (YAGNI)

- No NCQ (Native Command Queuing) — one outstanding command at a time per port.
  Single FIS slot used; spec supports 32, but polling-mode hides the win.
- No IRQ-driven completion initially — IMS masked, poll `PxCI`/`PxIS` clear.
  IRQ support deferred (MSI-X follow-up, post-Step 16).
- No hot-plug, no port multiplier, no ATAPI (CD/DVD), no PIO mode fallback
  (we require DMA-capable SATA — AHCI HBA guarantees it).
- No TRIM / SMART / write-cache management. Default-on write cache is fine.
- No FAT directory creation from kernel — fatfs handles it via its API, but
  the boot-time formatting is done by `mkfs.vfat` in the Makefile.
- No swap from initrd to disk for executable loading (separate step).
- No partition table parsing — disk image is "superfloppy" (whole-disk FAT).

---

## Architecture

```
                       PCI enumeration  (Step 13)
                                │
              find_class(0x01,0x06,0x01) → AHCI HBA PciDevice
                                │
   ┌────────────────────────────▼─────────────────────────────┐
   │ kernel/src/ahci/hba.rs   — HBA = whole BAR5 (ABAR)        │
   │   reset (GHC.HR), enable AHCI (GHC.AE), enumerate PI bits │
   └────────────────────────────┬─────────────────────────────┘
                                │
                  per-implemented-port  (PI bit set)
                                │
   ┌────────────────────────────▼─────────────────────────────┐
   │ kernel/src/ahci/port.rs  — one struct AhciPort per port  │
   │   stop engine, alloc CL+FIS+CT in DMA, restart engine,   │
   │   IDENTIFY DEVICE → sector count / LBA48 / model string  │
   └────────────────────────────┬─────────────────────────────┘
                                │ impl BlockDevice
                                ▼
   ┌──────────────────────────────────────────────────────────┐
   │ kernel/src/blockdev.rs   — trait BlockDevice             │
   │   { read_blocks, write_blocks, block_size, block_count } │
   └────────────────────────────┬─────────────────────────────┘
                                │
   ┌────────────────────────────▼─────────────────────────────┐
   │ kernel/src/fs/fatmount.rs — bridge BlockDevice ↔ fatfs   │
   │   impl fatfs::IoBase + Read + Write + Seek               │
   │   FileSystem<BlockDeviceWrapper> at mount time           │
   └────────────────────────────┬─────────────────────────────┘
                                │ impl vfs::FileSystem
                                ▼
                       vfs::mount("/mnt", ...)
                                │
                        WASM user via path_open("/mnt/...")
```

### Memory layout for one AHCI port

```
Port DMA region (one DmaRegion, contiguous, ~5 KiB):
  +0x000  Command List (1024 B = 32 × 32-B Command Headers)
  +0x400  Received FIS area (256 B)
  +0x500  Command Table 0  (Command FIS + PRDT[0..1])
            └─ small, since we use one PRDT entry per command
```

x86 is DMA-coherent → cacheable RAM. ABAR (MMIO) is uncached via
`map_io_range`. Per-port struct holds the ABAR pointer + the DMA region's
virt/phys; raw volatile accesses on register offsets.

---

## Components

### 0. `kernel/src/blockdev.rs` — `BlockDevice` trait

```rust
pub trait BlockDevice {
    fn block_size(&self) -> u32;        // 512 for SATA
    fn block_count(&self) -> u64;       // total LBA48 sectors
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8])  -> Result<(), BlockError>;
    fn write_blocks(&mut self, lba: u64, buf: &[u8])     -> Result<(), BlockError>;
}
#[derive(Debug, Clone, Copy)]
pub enum BlockError { Io, OutOfRange, BadAlignment, Timeout }
```

`buf` length must be a multiple of `block_size()`; `lba` must be < `block_count()`.

### 1. `kernel/src/ahci/hba.rs` — HBA discovery + reset

Registers (Intel/AHCI 1.3 spec §3.1):
- `CAP` @ 0x00 — capabilities (NP = port count - 1, S64A = 64-bit DMA)
- `GHC` @ 0x04 — global control (HR = HBA reset, AE = AHCI Enable, IE = IRQ enable)
- `IS`  @ 0x08 — interrupt status
- `PI`  @ 0x0C — ports implemented bitmap
- `VS`  @ 0x10 — version (≥ 0x00010300 for AHCI 1.3)

Init flow:
1. `find_class(0x01, 0x06, 0x01)` → discover HBA (skip if absent → no-op).
2. `enable_mmio()`, `enable_bus_master()`.
3. Read BAR5 (`Memory32`/`Memory64`), `map_io_range(phys, size)` → ABAR virt.
4. `GHC.AE = 1` (host-controlled mode).
5. `GHC.HR = 1`, poll until clears (HBA reset, bounded loop).
6. Re-`GHC.AE = 1` after reset.
7. Read `PI`, iterate set bits 0..31 → `AhciPort::new(abar, port_idx)`.

Log: `binfo!("ahci", "HBA up cap=… vs=… ports={}", popcount(PI))`.

### 2. `kernel/src/ahci/port.rs` — Port bring-up + IDENTIFY

Per-port registers offset from ABAR + `0x100 + port_idx * 0x80`:
- `PxCLB`  @ 0x00 — Command List Base (phys, low 32)
- `PxCLBU` @ 0x04 — Command List Base (high 32, if S64A)
- `PxFB`   @ 0x08 — FIS Base (phys, low 32)
- `PxFBU`  @ 0x0C — FIS Base (high 32)
- `PxIS`   @ 0x10 — interrupt status
- `PxIE`   @ 0x14 — interrupt enable
- `PxCMD`  @ 0x18 — command/status (`ST = 1<<0`, `FRE = 1<<4`, `FR = 1<<14`, `CR = 1<<15`)
- `PxTFD`  @ 0x20 — task file data (BSY = 1<<7, DRQ = 1<<3)
- `PxSIG`  @ 0x24 — device signature (SATA = 0x00000101)
- `PxSSTS` @ 0x28 — SATA status (DET=1 device, DET=3 present + PHY OK)
- `PxSCTL` @ 0x2C — SATA control
- `PxSERR` @ 0x30 — SATA error
- `PxCI`   @ 0x38 — command issue (1 bit per slot)

Init flow:
1. `PxSSTS.DET` must be 3 + sig = 0x00000101 → SATA disk (else skip).
2. Stop engine: clear `PxCMD.ST`, wait `CR == 0`; clear `PxCMD.FRE`, wait `FR == 0`.
3. Allocate DMA region (2 pages = 8 KiB, plenty): Command List @ +0,
   Received FIS @ +0x400, Command Table 0 @ +0x500.
4. Program `PxCLB`/`PxCLBU` to DMA phys; `PxFB`/`PxFBU` to DMA phys + 0x400.
5. Set `PxCMD.FRE = 1` and `PxCMD.ST = 1` (restart engine).
6. Issue `IDENTIFY DEVICE` (0xEC) on slot 0:
   - Command Header[0]: CFL=5 dword, W=0, PRDTL=1, CTBA=CT phys.
   - Command FIS: H2D, command=0xEC.
   - PRDT[0]: data_base = scratch_phys (one 512-B scratch page), DBC=511.
   - Wait `PxTFD.BSY = 0`, set `PxCI |= 1<<0`, poll `PxCI & 1 == 0`.
   - Parse 512 bytes: word 100..104 = LBA48 sector count, word 27..47 = model.

Logging: `binfo!("ahci", "port {} sata sectors={} model={}", idx, lba48, model)`.

### 3. `kernel/src/ahci/port.rs` — READ / WRITE DMA EXT

`READ DMA EXT` = 0x25, `WRITE DMA EXT` = 0x35. Both use LBA48 via the H2D FIS
LBA0..5 fields. PRDT[0] points to caller-supplied DMA buffer (must be 512-byte
aligned). Max one PRDT entry per command for simplicity → up to 8 MiB per cmd
(PRDT DBC limit 4 MiB, so cap at 4 MiB = 8192 sectors).

`impl BlockDevice for AhciPort` does:
```rust
fn read_blocks(&mut self, lba, buf) {
    assert buf.len() % 512 == 0 && lba + sectors < block_count;
    // Caller allocates a DMA scratch we copy in/out of — kernel heap buf
    // is HHDM-mapped so its phys = virt - hhdm_offset, eligible for PRDT.
    issue(ATA_READ_DMA_EXT, lba, sectors, buf);
}
```

`write_blocks` mirrors. Issue helper: builds CT (Command FIS + PRDT), starts
slot 0, polls `PxCI & 1 == 0` with timeout against `timer::ticks()`.

### 4. `kernel/src/fs/fatmount.rs` — fatfs ↔ BlockDevice bridge

Add `fatfs = { version = "0.4", default-features = false, features = ["alloc"] }`.

Wrap a `&mut dyn BlockDevice` in a struct `BlockIo { dev, pos: u64,
size: u64, sector_buf: [u8; 512] }` implementing:
- `fatfs::IoBase` — `type Error = ();`
- `fatfs::Read` — read across sector boundary into a 512-byte staging buffer.
- `fatfs::Write` — read-modify-write per sector for partial writes; full
  sector writes pass through.
- `fatfs::Seek` — adjust `pos`, bound by `size`.

Then `fatfs::FileSystem::new(io, FsOptions::new())` opens the volume.

Expose:
```rust
pub fn mount_fat(dev: AhciPort) -> Result<(), FatMountError>;
```

It stores the `FileSystem<BlockIo>` behind a `spin::Mutex<Option<...>>`,
registers a `FatVfs` impl of `vfs::FileSystem` at `/mnt`. Each VFS op
(`open`, `read`, `write`, `readdir`, `stat`) translates to fatfs `File` ops
under the mutex.

Concurrency: kernel is single-CPU; the mutex protects against multiple WASM
fibers racing on the same file. Each `open` returns a per-handle position
cursor; the fatfs `File` lives inside the mutex.

### 5. `kernel/src/vfs/mount.rs` — second mount point

VFS already has a single root mount (tmpfs). Extend the existing mount table
to support multiple mount points keyed by path prefix:
- Lookup: longest-prefix match. `/mnt/foo` → FatVfs (strip `/mnt`).
- `/mnt` itself resolves to the FAT root dir.
- Existing tmpfs at `/` keeps handling everything outside `/mnt`.

This is the smallest VFS change consistent with adding more mounts later
(SquashFS for read-only initrd swap, future `/proc`/`/sys` virtual fs).

### 6. `Makefile` — disk image + QEMU drive

```makefile
DISK_IMG := build/disk.img
DISK_MB  := 64

$(DISK_IMG):
	mkdir -p build
	dd if=/dev/zero of=$@.tmp bs=1M count=$(DISK_MB)
	mkfs.vfat -F 32 -n RUOS $@.tmp
	mkdir -p build/diskmnt
	# Populate via mtools (works without sudo loop-mount).
	echo 'hello from disk' | mcopy -i $@.tmp - ::/hello.txt
	mv $@.tmp $@

iso: ... $(DISK_IMG)    # ensure disk exists for run/run-test

run-test: iso
	... -drive file=$(DISK_IMG),if=none,id=disk0 \
	    -device ahci,id=ahci \
	    -device ide-hd,drive=disk0,bus=ahci.0
	# add gates:
	grep -qF "ahci HBA up" build/serial.log || fail
	grep -qE "ahci port \d+ sata sectors=" build/serial.log || fail
	grep -qF "mnt mounted FAT" build/serial.log || fail
```

`init.sh` smoke append: `cat /mnt/hello.txt` (expect `hello from disk` in
serial).

### Error handling

`BlockError`: `Io` (TFD ERR set), `OutOfRange` (lba past end), `BadAlignment`
(buf size or offset not 512-mul), `Timeout` (PxCI didn't clear). Display.
AHCI surface: log + return error, never panic the kernel. fatfs errors map
through `FatMountError` similarly.

### Testing strategy

`no_std` kernel — tests = QEMU + serial grep. New gates land in `run-test`:
- `ahci HBA up` (HBA discovered + reset OK)
- `ahci port N sata sectors=<n>` (IDENTIFY succeeded)
- `mnt mounted FAT` (fatmount + VFS register)
- `init.sh` end: `hello from disk` in serial (cat from disk works)

End-to-end persistence test (manual, not in CI): boot, `cp /etc/init.sh
/mnt/saved.sh`, poweroff, boot again, `cat /mnt/saved.sh` returns file.

---

# Implementation Plan

> All commands via WSL (per CLAUDE.md):
> ```bash
> wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
> ```
> Build `make build`; gate `make run-test`. Logging `binfo!("ahci", ...)` /
> `bwarn!`. **Branch:** `feature/step15-ahci-fat` (already created). Commit
> per task; do NOT push. **CHANGELOG:** one `CHANGELOG/NN-26-05-29-slug.md`
> per task; the `NN` is the next free integer at execution time.

## Task 1 — Branch, spec, `BlockDevice` trait skeleton

**Files:** Create `docs/superpowers/specs/2026-05-29-rust-step15-ahci-fat-design.md`
(this file), `kernel/src/blockdev.rs`; modify `kernel/src/main.rs`
(`mod blockdev;`).

- [ ] **Step 1:** Branch already created; commit this spec.
- [ ] **Step 2:** Create `kernel/src/blockdev.rs` with the trait + `BlockError`
  enum + `Display`. Add `mod blockdev;` to `main.rs`.
- [ ] **Step 3 (test):** `make build` → `Finished`.
- [ ] **Step 4:** CHANGELOG + commit (`feat(blockdev): trait skeleton`).

## Task 2 — `ahci/mod.rs` skeleton + HBA discovery + reset

**Files:** Create `kernel/src/ahci/{mod.rs, hba.rs}`; modify `main.rs`.

- [ ] **Step 1:** `ahci/mod.rs` with `pub mod hba; pub mod port;` (port empty
  for now). `hba.rs`: PCI find_class(0x01,0x06,0x01), enable_mmio,
  enable_bus_master, map ABAR. Reset via GHC.HR + bounded poll. Re-enable AE.
  Log `ahci HBA up cap=0x… vs=0x… ports=N`. Public `pub fn init() ->
  Option<Hba>` that returns the Hba snapshot (ABAR virt + PI bitmap + port
  count).
- [ ] **Step 2:** Wire `mod ahci;` in main and call `ahci::init()` from a new
  `boot/phases/storage.rs` (between PCI and userland phases).
- [ ] **Step 3 (test):** make build → Finished. `make run-test` with new
  `-drive`/`-device ahci` flags in Makefile (Task 7's gates not added yet).
  Serial must contain `ahci HBA up`.
- [ ] **Step 4:** CHANGELOG + commit (`feat(ahci): HBA discovery + reset`).

## Task 3 — Port bring-up

**Files:** Modify `kernel/src/ahci/port.rs`.

- [ ] **Step 1:** `struct AhciPort { abar: VirtAddr, port: usize, dma:
  DmaRegion, sectors: u64, lba48: bool, model: String }`. Per-port reset:
  detect `PxSSTS.DET == 3` and sig 0x101 (SATA), stop engine (`PxCMD.ST = 0`
  → wait CR=0; `PxCMD.FRE = 0` → wait FR=0), allocate 2-page DMA, program
  `PxCLB`/`PxCLBU` and `PxFB`/`PxFBU`, set `PxCMD.FRE` + `PxCMD.ST`.
- [ ] **Step 2 (test):** `make build`. No new gate yet — port up but no
  IDENTIFY logged.
- [ ] **Step 3:** CHANGELOG + commit (`feat(ahci): port bring-up`).

## Task 4 — IDENTIFY DEVICE + ATA polling helper

**Files:** Modify `port.rs`.

- [ ] **Step 1:** Add `fn issue(&mut self, cmd: u8, lba: u64, sectors: u16,
  buf_phys: u64) -> Result<(), BlockError>` that builds CT0 (Command FIS H2D
  with the command + LBA fields + sector count + PRDT[0] pointing at
  `buf_phys` for `sectors*512`), sets `PxCI |= 1`, polls `PxCI & 1 == 0`
  with `timer::ticks()` timeout (5 s), checks `PxTFD.ERR == 0`.
- [ ] **Step 2:** Add `pub fn identify(&mut self)` that issues 0xEC into a
  one-page scratch DMA, parses words 100..104 → sectors, words 27..47 →
  model. Log `ahci port N sata sectors=… model="…"`.
- [ ] **Step 3 (test):** make run-test. Expect `ahci port 0 sata sectors=131072`
  for the 64 MiB disk image.
- [ ] **Step 4:** CHANGELOG + commit (`feat(ahci): IDENTIFY + polled I/O`).

## Task 5 — READ DMA EXT + WRITE DMA EXT

**Files:** Modify `port.rs`.

- [ ] **Step 1:** `read_dma(&mut self, lba: u64, sectors: u16, buf_phys:
  u64)` → `issue(0x25, lba, sectors, buf_phys)`. `write_dma` → `issue(0x35,
  ...)` with PRDT direction bit.
- [ ] **Step 2:** `impl BlockDevice for AhciPort`: chunk arbitrary-sized
  buffers into ≤8 MiB DMA windows, copy in/out of caller buf via HHDM (caller
  buf is heap → its phys = virt - hhdm_offset, already DMA-eligible since x86
  is coherent).
- [ ] **Step 3 (test):** make build. No serial gate yet; tested via fatmount
  in Task 7.
- [ ] **Step 4:** CHANGELOG + commit (`feat(ahci): READ/WRITE DMA EXT`).

## Task 6 — `fatfs` dep + `BlockIo` bridge

**Files:** `kernel/Cargo.toml`, create `kernel/src/fs/fatmount.rs`.

- [ ] **Step 1:** Add `fatfs = { version = "0.4", default-features = false,
  features = ["alloc"] }` to `kernel/Cargo.toml`. Create `kernel/src/fs/mod.rs`
  if not present (else extend), `pub mod fatmount;`.
- [ ] **Step 2:** `BlockIo<'a> { dev: &'a mut dyn BlockDevice, pos: u64, size:
  u64, sector_buf: [u8; 512] }` impls `fatfs::IoBase`/`Read`/`Write`/`Seek`.
  Cross-sector read = staging-buffer copy; partial-sector write = RMW.
- [ ] **Step 3 (test):** make build → Finished.
- [ ] **Step 4:** CHANGELOG + commit (`feat(fs): fatfs ↔ BlockDevice bridge`).

## Task 7 — VFS mount point + `mount_fat`

**Files:** Modify `kernel/src/vfs/mod.rs` (and where the mount table lives),
extend `fatmount.rs` with `FatVfs` impl of `vfs::FileSystem`, wire in
`boot/phases/storage.rs`.

- [ ] **Step 1:** Extend vfs mount table to longest-prefix lookup. Tmpfs at
  `/`, FAT at `/mnt`. Path resolution strips the mount prefix.
- [ ] **Step 2:** `FatVfs { fs: spin::Mutex<FileSystem<BlockIo>> }` impl
  `vfs::FileSystem` (open/read/write/seek/stat/readdir/mkdir/unlink/rename).
- [ ] **Step 3:** `mount_fat(port: AhciPort)` → `Box<AhciPort>` static, BlockIo
  over it, FileSystem::new, `vfs::mount("/mnt", FatVfs::new(fs))`. Log
  `mnt mounted FAT` on success.
- [ ] **Step 4 (test):** `make run-test`. Serial must include `mnt mounted FAT`.
  `init.sh` cats `/mnt/hello.txt` and the boot log shows `hello from disk`.
- [ ] **Step 5:** CHANGELOG + commit (`feat(fs): mount FAT at /mnt via VFS`).

## Task 8 — Makefile disk image + run-test gates

**Files:** Modify `Makefile`, `user-bin/init.sh`.

- [ ] **Step 1:** `DISK_IMG := build/disk.img`. Recipe creates a 64 MiB raw
  image via `dd`, formats FAT32 with `mkfs.vfat -F 32 -n RUOS`, populates
  `hello.txt` via `mcopy -i ... ::/hello.txt`. Add as prereq to `iso`.
- [ ] **Step 2:** `run` / `run-test` QEMU args gain `-drive file=$(DISK_IMG),
  if=none,id=disk0 -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0`.
  `run-test` adds gates: `ahci HBA up`, `ahci port \d+ sata sectors=`,
  `mnt mounted FAT`, `hello from disk`.
- [ ] **Step 3:** `init.sh`: `cat /mnt/hello.txt` near the end.
- [ ] **Step 4 (test):** `make run-test` → `TEST_PASS`.
- [ ] **Step 5:** CHANGELOG + commit (`feat(makefile): disk.img + FAT gates`).

## Task 9 — Docs + roadmap mark done

**Files:** `docs/superpowers/roadmap-rust-os.md`, `README.md`.

- [ ] **Step 1:** Roadmap Step 15 → ✅ DONE, link to spec + plan.
- [ ] **Step 2:** README new section `/mnt` (FAT-on-AHCI), how to inspect
  `build/disk.img` host-side with `mtools`.
- [ ] **Step 3:** CHANGELOG + commit (`docs(roadmap): Step 15 AHCI/FAT done`).

---

## Done criteria

- `make run-test` (default virtio NIC, AHCI disk) → `TEST_PASS` with serial
  containing `ahci HBA up`, `ahci port 0 sata sectors=131072`, `mnt mounted
  FAT`, and `hello from disk`.
- `make run-test NIC=e1000` still passes (no regression on NIC path).
- A WASM tool can `path_open("/mnt/...", O_RDWR | O_CREAT)`, write, close,
  and read back — verified manually from the shell.

## Notes for the implementer

- **DMA-coherent**: do NOT mark Command List / FIS / scratch buffers
  uncached. The ABAR (MMIO registers) is uncached via `map_io_range`; the
  ring/scratch memory stays normal cacheable RAM (matches the conventions
  in `memory/dma.rs` and `net/nic/ring.rs`).
- **HHDM phys ↔ virt**: any heap buffer the caller passes to
  `BlockDevice::read_blocks/write_blocks` is HHDM-mapped, so
  `phys = virt - hhdm_offset()`. Make this explicit in `impl BlockDevice
  for AhciPort` so the PRDT addresses are obviously correct.
- **Polling timeouts**: every wait loop must terminate against
  `timer::ticks()`. PxCMD reset waits: 1 s; PxCI command: 5 s. Log + return
  `BlockError::Timeout` rather than spinning forever.
- **Sector size**: assume 512 B. ATA drives can advertise 4 KiB physical
  but we read in 512-B logical sectors via LBA48; fatfs is fine with that.
- **DMA buffer phys-range**: contiguous frame allocator (Step 14) caps
  contiguous regions to one allocation request. 2 pages (8 KiB) per port is
  comfortably within typical free runs.
- **Init order**: `phases/storage.rs` after `phases::pci::init()` and
  `phases::devices::init()` but before `phases::userland::init()` (which
  calls `net::init()` and starts the executor). Ensures FAT is mounted
  before any WASM can access `/mnt`.
