# USB-MSC Live-CD Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline). Steps use checkbox (`- [ ]`).

**Goal:** Leggere `/bin` off-boot dalla chiavetta USB di boot tramite un driver USB Mass-Storage (BOT/SCSI read-only), così la ISO boota a shell anche su HW reale (oggi: `shell not found`).

**Architecture:** Nuovo driver USB-MSC su xHCI (bulk transfer) → `BlockDevice` → montato in ISO9660 via adattatore di scala settore (2048↔512). L'overlay `/bin` si sposta dalla fase `storage` (gira prima dell'USB) a un nuovo step `media_bin` dopo l'enumerazione USB, che prova ATAPI poi USB-MSC. Set minimo (`shell.cwasm`+init) resta modulo Limine come rete di sicurezza.

**Tech Stack:** Rust no_std, xHCI (`xhci` crate), DMA `memory::dma`, VFS/iso9660, QEMU `usb-storage`.

Spec: `docs/superpowers/specs/2026-06-08-usb-msc-livecd-design.md`.

---

### Task 1: `SectorScale` adapter (pure, TDD)

**Files:**
- Modify: `kernel/src/blockdev.rs` (add `SectorScale` + tests)

- [ ] **Step 1: failing test** — in `blockdev.rs` `#[cfg(test)] mod tests`, add:
```rust
#[test] fn sector_scale_512_to_2048() {
    // 8 logical-512 sectors = 2 iso-2048 sectors; mark byte 0 of iso-sector 1.
    let mut backing = vec![0u8; 512*8];
    backing[4*512] = 0xCD;                  // iso-sector 1 (lba 4 @512) byte 0
    let mut ss = SectorScale::new(Box::new(Mem(backing))); // Mem is 512B
    assert_eq!(ss.block_size(), 2048);
    assert_eq!(ss.block_count(), 2);
    let mut buf = [0u8; 2048];
    ss.read_blocks(1, &mut buf).unwrap();   // iso lba 1 -> dev lba 4
    assert_eq!(buf[0], 0xCD);
    assert!(ss.read_blocks(2, &mut buf).is_err());   // out of range
}
#[test] fn sector_scale_passthrough_2048() {
    struct M2(Vec<u8>);
    impl BlockDevice for M2 { fn block_size(&self)->u32{2048} fn block_count(&self)->u64{(self.0.len()/2048)as u64}
        fn read_blocks(&mut self,l:u64,b:&mut[u8])->Result<(),BlockError>{let o=(l as usize)*2048;b.copy_from_slice(&self.0[o..o+b.len()]);Ok(())}
        fn write_blocks(&mut self,_:u64,_:&[u8])->Result<(),BlockError>{Err(BlockError::Io)} }
    let mut ss = SectorScale::new(Box::new(M2(vec![7u8;2048*3])));
    assert_eq!(ss.block_size(),2048); assert_eq!(ss.block_count(),3);
}
```
- [ ] **Step 2:** `cargo test -p kernel-lib` (host) → FAIL (no SectorScale).
- [ ] **Step 3: implement** in `blockdev.rs`:
```rust
/// Presents a 2048-byte logical block on top of any smaller-sector device,
/// so iso9660 (which addresses 2048-byte ISO sectors directly) can read a
/// 512-byte USB Mass-Storage LUN. ratio = 2048 / inner.block_size().
pub struct SectorScale { inner: Box<dyn BlockDevice + Send>, ratio: u64 }
impl SectorScale {
    pub fn new(inner: Box<dyn BlockDevice + Send>) -> Self {
        let bs = inner.block_size().max(1) as u64;
        let ratio = (2048 / bs).max(1);
        Self { inner, ratio }
    }
}
impl BlockDevice for SectorScale {
    fn block_size(&self) -> u32 { 2048 }
    fn block_count(&self) -> u64 { self.inner.block_count() / self.ratio }
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() % 2048 != 0 { return Err(BlockError::BadAlignment); }
        let inner_lba = lba.checked_mul(self.ratio).ok_or(BlockError::OutOfRange)?;
        self.inner.read_blocks(inner_lba, buf)
    }
    fn write_blocks(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), BlockError> { Err(BlockError::Io) }
}
```
(`Mem` test helper already exists in the module and is 512B.)
- [ ] **Step 4:** `cargo test` → PASS.
- [ ] **Step 5: commit** `feat(blockdev): SectorScale 2048<-512 adapter for iso9660 on USB`.

### Task 2: MSC wire-format helpers (pure, TDD)

**Files:**
- Create: `kernel/src/usb/msc.rs` (CBW/CSW build+parse + tests). Add `pub mod msc;` to `usb/mod.rs`.

- [ ] **Step 1: failing test** — CBW build (31B) + CSW parse (13B):
```rust
#[cfg(test)] mod tests { use super::*;
  #[test] fn cbw_read10() {
    let cdb = scsi_read10(5, 2);                 // lba=5, 2 blocks
    let cbw = build_cbw(0xDEADBEEF, 2*512, true, 0, &cdb);
    assert_eq!(&cbw[0..4], b"USBC");
    assert_eq!(u32::from_le_bytes([cbw[4],cbw[5],cbw[6],cbw[7]]), 0xDEADBEEF);
    assert_eq!(u32::from_le_bytes([cbw[8],cbw[9],cbw[10],cbw[11]]), 2*512);
    assert_eq!(cbw[12], 0x80);                    // data-IN
    assert_eq!(cbw[14], 10);                      // CDB len
    assert_eq!(cbw[15], 0x28);                    // READ(10) opcode
    assert_eq!(u32::from_be_bytes([cbw[15+2],cbw[15+3],cbw[15+4],cbw[15+5]]), 5);
  }
  #[test] fn csw_ok() {
    let mut c = [0u8;13]; c[0..4].copy_from_slice(b"USBS");
    c[4..8].copy_from_slice(&0xDEADBEEFu32.to_le_bytes()); c[12]=0;
    let p = parse_csw(&c, 0xDEADBEEF).unwrap();
    assert_eq!(p.status, 0); assert_eq!(p.residue, 0);
  }
  #[test] fn csw_bad_sig() { let c=[0u8;13]; assert!(parse_csw(&c,0).is_none()); }
}
```
- [ ] **Step 2:** `cargo test` → FAIL.
- [ ] **Step 3: implement** pure helpers in `msc.rs` (no hardware deps):
```rust
pub const CBW_LEN: usize = 31; pub const CSW_LEN: usize = 13;
pub struct Csw { pub tag: u32, pub residue: u32, pub status: u8 }
pub fn build_cbw(tag: u32, data_len: u32, data_in: bool, lun: u8, cdb: &[u8]) -> [u8; CBW_LEN] {
    let mut b = [0u8; CBW_LEN];
    b[0..4].copy_from_slice(b"USBC");
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&data_len.to_le_bytes());
    b[12] = if data_in { 0x80 } else { 0x00 };
    b[13] = lun & 0x0F;
    b[14] = cdb.len() as u8;
    b[15..15+cdb.len()].copy_from_slice(cdb);
    b
}
pub fn parse_csw(b: &[u8], tag: u32) -> Option<Csw> {
    if b.len() < CSW_LEN || &b[0..4] != b"USBS" { return None; }
    let got = u32::from_le_bytes([b[4],b[5],b[6],b[7]]);
    if got != tag { return None; }
    Some(Csw { tag: got, residue: u32::from_le_bytes([b[8],b[9],b[10],b[11]]), status: b[12] })
}
pub fn scsi_read10(lba: u32, blocks: u16) -> [u8;10] {
    let mut c=[0u8;10]; c[0]=0x28; c[2..6].copy_from_slice(&lba.to_be_bytes());
    c[7..9].copy_from_slice(&blocks.to_be_bytes()); c
}
pub fn scsi_read_capacity10() -> [u8;10] { let mut c=[0u8;10]; c[0]=0x25; c }
pub fn scsi_test_unit_ready() -> [u8;6] { [0u8;6] }
pub fn scsi_inquiry(len: u8) -> [u8;6] { let mut c=[0u8;6]; c[0]=0x12; c[4]=len; c }
```
- [ ] **Step 4:** `cargo test` → PASS.
- [ ] **Step 5: commit** `feat(usb-msc): BOT CBW/CSW + SCSI CDB builders (pure)`.

### Task 3: xHCI bulk transfer infra

**Files:**
- Create: `kernel/src/usb/xhci/bulk.rs`; add `pub mod bulk;` to `usb/xhci/mod.rs`.

Hardware path (no host test; verified in QEMU at Task 7). Provide:
```rust
//! Synchronous bulk IN/OUT transfer over an xHCI endpoint transfer ring.
use super::Xhci; use crate::memory::dma::DmaRegion;
/// Push one Normal TRB (buf_phys, len, IOC), ring slot doorbell(dci), wait
/// for THIS endpoint's Transfer Event (type 32, matching slot+dci). Returns
/// (completion_code, residual_bytes). Mirrors control.rs's wait_for pattern.
pub fn bulk_xfer(
    x: &mut Xhci, slot: u8, dci: u8,
    ring: &DmaRegion, enq: &mut usize, cyc: &mut bool,
    buf_phys: u64, len: u32,
) -> Option<(u8, u32)> {
    super::ring::enqueue_xfer(ring, enq, cyc, [
        (buf_phys & 0xFFFF_FFFF) as u32, (buf_phys >> 32) as u32,
        len, (1 << 10) | (1 << 5),    // type=1 Normal | IOC
    ]);
    x.regs.doorbell.update_volatile_at(slot as usize, |d| { d.set_doorbell_target(dci); });
    let ev = super::event::wait_for(x, 1000, |w|
        super::ring::trb_type(w) == 32
        && ((w[3] >> 24) & 0xFF) as u8 == slot
        && ((w[3] >> 16) & 0x1F) as u8 == dci)?;
    Some((super::ring::completion_code(&ev) as u8, ev[2] & 0x00FF_FFFF))
}
```
- [ ] Implement + `pub mod bulk;`. Builds clean (`make iso` later). Commit `feat(usb-xhci): synchronous bulk transfer`.

### Task 4: MSC enumeration + endpoint config + `MscBlock`

**Files:**
- Modify: `kernel/src/usb/msc.rs` (add hardware side: detect, configure bulk EPs, MscState, MscBlock).
- Modify: `kernel/src/usb/device.rs` (detect MSC interface in a new `configure_msc`, dispatch in `enumerate`).
- Modify: `kernel/src/usb/registry.rs` (`SlotKind::Msc(MscState)` + probe_dump arm; teardown arm; dispatch_transfer leaves MSC alone — it's polled synchronously, not via worklist).
- Modify: `kernel/src/usb/mod.rs` (`pub fn first_msc_block() -> Option<MscBlock>`).

Key pieces:
1. `device.rs`: after hub/HID checks, add MSC. Walk config descriptor (reuse the walk in `configure`) for interface class 0x08 / sub 0x06 / proto 0x50, capture bulk-IN + bulk-OUT endpoint addresses. New `MscIface { iface, ep_in, ep_out }`. If found → `msc::configure_endpoints(x, dev, &mi)?` → `SlotKind::Msc(state)`.
2. `msc.rs` `configure_endpoints`: Configure Endpoint command enabling BOTH bulk DCIs (in one input ctx, like hid but two add-context flags; `set_context_entries(max(dci_in,dci_out))`, EndpointType::BulkOut/BulkIn, max_packet from ep desc, error_count 3, separate transfer ring per ep). Then `SET_CONFIGURATION` (cfg_val). Then SCSI bring-up: `TEST UNIT READY` (retry ≤ ~1.5s), `READ CAPACITY(10)` → block_size/block_count. Returns `MscState`.
3. `MscState { slot_id, dci_in, dci_out, ring_in, enq_in, cyc_in, ring_out, enq_out, cyc_out, data: DmaRegion, tag: u32, block_size: u32, block_count: u64, max_lun: u8 }`.
4. `MscBlock { slot: u8 }` implements `BlockDevice`. `read_blocks`: lock `usb::CTRL` → `with_slot` MSC state → for each chunk (≤ data page / block_size) run `bot_read(x, st, lba, n, out)`: build CBW(READ10) into `data` page, `bulk_xfer` OUT(cbw,31), `bulk_xfer` IN(data, n*bs) into `data` page, `bulk_xfer` IN(csw,13); verify CSW; copy `data`→buf. `block_size`/`block_count` cached on `MscBlock` (read once at construction from MscState). `write_blocks`→`Err(Io)`.

> Lock note: `MscBlock::read_blocks` takes `usb::CTRL` then `registry::with_slot`. Never call it while already holding either (VFS read path on executor does not). Single-threaded during `media_bin`.

5. `usb::first_msc_block()`: scan registry slots for `SlotKind::Msc`, return `MscBlock{slot}` with cached geometry; `None` if no MSC.

- [ ] Implement, then build. Commit `feat(usb-msc): BOT/SCSI driver + MscBlock BlockDevice`.

### Task 5: `media_bin` boot phase + move overlay out of storage

**Files:**
- Create: `kernel/src/boot/phases/media_bin.rs`.
- Modify: `kernel/src/boot/phases/mod.rs` (`pub mod media_bin;`).
- Modify: `kernel/src/boot/mod.rs` (call `phases::media_bin::init()?` after `phases::usb::init()?`).
- Modify: `kernel/src/boot/phases/storage.rs` (remove the `/bin` ATAPI overlay block from `mount_userspace`; keep `mount_all` + `/mnt`).

`media_bin.rs`:
```rust
//! Phase — media_bin: mount /bin off-boot from removable media, AFTER USB enum.
use crate::boot::BootError;
pub fn init() -> Result<(), BootError> {
    pump_usb();                                   // drive enumeration to quiet
    if atapi_overlay() { return Ok(()); }         // CD-ROM (VM / real CD)
    if usb_overlay()   { return Ok(()); }         // USB stick (real HW)
    crate::bwarn!("media_bin", "no removable /bin medium; Limine fallback set in use");
    Ok(())
}
fn pump_usb() {
    let end = crate::boot::clock::elapsed_ms() + 3000;
    let mut idle = 0;
    while crate::boot::clock::elapsed_ms() < end && idle < 50 {
        crate::usb::poll();
        if crate::usb::pending_work() { idle = 0; } else { idle += 1; }
        core::hint::spin_loop();
    }
}
fn atapi_overlay() -> bool {
    if let Some(cd) = crate::ahci::acquire_atapi_port() {
        match crate::vfs::iso9660::mount_from_blockdev(alloc::boxed::Box::new(cd), "/bin", "/bin") {
            Ok(()) => { crate::binfo!("media_bin", "/bin overlaid from ISO9660 (ATAPI)"); return true; }
            Err(e) => crate::bwarn!("media_bin", "ATAPI ISO9660 mount failed: {}", e),
        }
    }
    false
}
fn usb_overlay() -> bool {
    let blk = match crate::usb::first_msc_block() { Some(b) => b, None => return false };
    crate::binfo!("media_bin", "usb-msc: bsize={} blocks={}", blk.block_size(), blk.block_count());
    let dev = alloc::boxed::Box::new(crate::blockdev::SectorScale::new(alloc::boxed::Box::new(blk)));
    match crate::vfs::iso9660::mount_from_blockdev(dev, "/bin", "/bin") {
        Ok(()) => { crate::binfo!("media_bin", "/bin overlaid from USB-MSC (ISO9660)"); true }
        Err(e) => { crate::bwarn!("media_bin", "USB-MSC ISO9660 mount failed: {}", e); false }
    }
}
```
- Add `usb::pending_work()` (true if event ring non-empty OR worklist non-empty) to `usb/mod.rs` for the quiescence check. Simpler fallback: just pump a fixed ~1.5s. (Use fixed-time pump if pending_work is awkward.)
- `storage.rs`: delete lines 24-31 (the `acquire_atapi_port`→iso9660 block) and the explanatory comment; `mount_userspace` becomes just `mount_all` + log. Keep the `boot_cd_port` skip in the SATA loop (harmless; now set by media_bin's `acquire_atapi_port` which runs first).

> Order check: `media_bin` runs after `storage`. `acquire_atapi_port` records `BOOT_CD_PORT`, but the SATA `/mnt` loop already ran in `storage` BEFORE that. On VBox (CD on boot HBA port 0) the old code relied on storage skipping the CD port. NEW risk: storage's SATA loop now runs before the CD port is acquired, so it may bring up the CD port as if SATA. Mitigation: in storage's SATA loop keep the `if port.is_atapi { continue; }` guard (already present, line 58) — that skips ATAPI ports regardless of `boot_cd_port`. So VBox stays safe. Verify in code.

- [ ] Implement, build. Commit `feat(boot): media_bin phase — mount /bin from ATAPI or USB-MSC after USB enum`.

### Task 6: Limine fallback set + Makefile

**Files:**
- Modify: `limine.conf` (re-add `/bin/shell.cwasm` as a boot module; keep init.wasm + init.sh).
- Modify: `Makefile` (ensure shell.cwasm staged as a module path; rest of /bin stays ISO-only).

`limine.conf` add under the existing module list:
```
    module_path: boot():/bin/shell.cwasm
    module_cmdline: /bin/shell.cwasm
```
(The file already exists in ISO `/bin/shell.cwasm`. As a module it is ALSO loaded into RAM and `mount_all` places it in tmpfs `/bin/shell.cwasm` → fallback if USB-MSC overlay fails. The USB/ATAPI ISO9660 overlay then shadows `/bin`.)
- Verify `modules::mount_all` maps a module whose cmdline is `/bin/shell.cwasm` to tmpfs path `/bin/shell.cwasm`. If `mount_all` strips dirs, adjust cmdline accordingly (check `kernel/src/modules.rs`).
- [ ] Commit `feat(livecd): shell.cwasm fallback module (boot to shell even if USB-MSC fails)`.

### Task 7: QEMU integration test `run-test-usb`

**Files:**
- Modify: `Makefile` (new `run-test-usb` target).
- Create: `tests/usb-msc-test.sh` (optional; or inline in target).

New target: build ISO with smoke.sh init, boot from USB storage holding the ISO, NO `-cdrom`:
```make
run-test-usb:
	@$(MAKE) iso INIT_SCRIPT=user-bin/smoke.sh
	@echo "--- usb-msc serial (timeout 240s) ---"
	@timeout 240 qemu-system-x86_64 -machine q35 -cpu max -m 512 -no-reboot -display none -serial stdio \
		-drive if=none,id=stick,file=$(ISO),format=raw \
		-device qemu-xhci,id=xhci -device usb-storage,bus=xhci.0,drive=stick,bootindex=0 \
		-device usb-kbd,bus=xhci.0 \
		-netdev user,id=net0 -device $(NIC),netdev=net0 \
		| tee build/serial-usb.log; \
	grep -qF "$(HELLO)" build/serial-usb.log || { echo TEST_FAIL_SHELL; exit 1; }; \
	grep -qiE "MSC .*slot=" build/serial-usb.log || { echo TEST_FAIL_MSC_ENUM; exit 1; }; \
	grep -qF "/bin overlaid from USB-MSC" build/serial-usb.log || { echo TEST_FAIL_USB_BIN; exit 1; }; \
	echo TEST_PASS_USB
```
- [ ] Run `make run-test-usb` in WSL → iterate until `TEST_PASS_USB`.
- [ ] Run `make run-test` (ATAPI) → still green (media_bin chooses ATAPI first).
- [ ] Commit `test(livecd): run-test-usb — boot /bin from USB-MSC`.

### Task 8: Changelog + wrap

- [ ] CHANGELOG entries (one per logical change, next NN after 352): implementation entry(ies).
- [ ] Final `make run-test` + `make run-test-usb` both green (evidence pasted).
- [ ] Verify on real hardware (user-driven): boot USB stick, confirm desktop/shell. `usb-probe` if it fails.

---

## Self-review notes
- Spec coverage: bulk infra(T3), MSC driver(T2,T4), SectorScale(T1), media_bin+ordering(T5), Limine fallback(T6), tests(T1,T2,T7). ✓
- Ordering risk (storage SATA loop vs CD port) mitigated by existing `is_atapi` guard — verify in code at T5.
- `pending_work()` optional; fixed-time pump acceptable fallback.
- Real-HW SuperSpeed bulk: max_burst 0 / no streams assumed (BOT). If a real stick is SuperSpeed and needs streams, follow-up.
