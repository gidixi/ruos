# 143 — Task 3+4: AHCI port bring-up + IDENTIFY DEVICE + READ/WRITE DMA EXT

**Data:** 2026-05-29

## Cosa

`kernel/src/ahci/port.rs` (~360 righe):

- **Port bring-up** (`AhciPort::bringup`):
  1. Check `PxSSTS.DET == 3` (device + PHY ready)
  2. Stop engine: clear `PxCMD.ST` → wait CR=0; clear `PxCMD.FRE` → wait FR=0
  3. Alloc 2 pagine DMA, layout: CL@0x000 + FIS@0x400 + CT0@0x500 + scratch@0x600
  4. Program `PxCLB`/`PxCLBU` (CL phys), `PxFB`/`PxFBU` (FIS phys)
  5. Clear `PxSERR` + `PxIS`, mask `PxIE` (polling)
  6. Set `PxCMD.FRE = 1` (FIS Receive on) — **necessario PRIMA di leggere PxSIG**
     perché device latch sig solo dopo aver inviato D2H Register FIS
  7. Wait TFD.BSY|DRQ clear + valid sig (~1 s timeout)
  8. Check sig == 0x101 (SATA disk)
  9. Set `PxCMD.ST = 1` (command engine on)

- **`issue()` polling-mode command** (slot 0 only):
  - Build H2D Register FIS in CT0.cfis (command + LBA48 + sectors)
  - Build PRDT[0] (buf_phys, byte count, DBC = bytes-1)
  - CmdHeader[0]: CFL=5 dwords, W bit per write, PRDTL=1, CTBA=CT0 phys
  - Wait TFD.BSY|DRQ clear, set `PxCI |= 1`, poll until clear
  - Check TFD.ERR

- **IDENTIFY DEVICE** (cmd 0xEC): parsed 512-byte response:
  - word 83 bit 10 → LBA48 supported flag
  - words 100..104 → u64 sector count
  - words 27..47 → model string (byte-swapped, 40 chars, trimmed)
  - LBA28 fallback (words 60..62)

- **READ DMA EXT** (0x25) + **WRITE DMA EXT** (0x35):
  - `impl BlockDevice for AhciPort`
  - PRDT-entry cap 4 MiB → chunking loop split arbitrary buffer in
    blocchi ≤ 8192 sectors
  - HHDM `phys = virt - hhdm_offset()` per buffer caller heap

- Helper: `port_addr`, `read32`/`write32`, `wait_clear(addr, bits, timeout)`.

`kernel/src/ahci/mod.rs`:
- Global `PORT0: Mutex<Option<AhciPort>>` + `set_port0` / `take_port0`
  (consumato dal mount FAT in Task 7).

`kernel/src/boot/phases/storage.rs`:
- Itera `PI` bits, chiama `AhciPort::bringup`, stash first OK in PORT0.

### Makefile gate

`run-test` ora asserisce `ahci port [0-9]+ sata sectors=`.

## Test

`make run-test` → TEST_PASS. Serial:
```
[T+3.759s] INFO ahci HBA up cap=0xc0141f05 vs=0x00010000 ports=6 pi=0x0000003f
[T+3.785s] INFO ahci port 0 sata sectors=131072 model="QEMU HARDDISK"
```

131072 sectors × 512 B = 64 MiB = matches `DISK_MB`. IDENTIFY OK.

## Note di sequence importanti

`PxSIG` invalido (0xFFFFFFFF) finché engine FIS Receive non parte. Bug
classico di porting AHCI: leggere sig prima di abilitare FRE → "no
device". Fix: FRE-then-wait-sig come fa Linux libahci.

## File toccati

- kernel/src/ahci/port.rs (full impl)
- kernel/src/ahci/mod.rs (PORT0 stash + set/take_port0)
- kernel/src/ahci/hba.rs (cleanup diagnostic)
- kernel/src/boot/phases/storage.rs (iterate PI, bring up first)
- Makefile (run-test gate IDENTIFY)
- CHANGELOG/143-26-05-29-ahci-port-identify.md (questo)
