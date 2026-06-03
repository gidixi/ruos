# M2a Disk-authoring (GPT write + FAT32 mkfs + mkdir) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Let ruos author a disk from scratch — write a valid GPT and format
FAT32 partitions with a directory tree — verified by host tools (`sgdisk -v`,
`fsck.fat`, `mtools`) and ruos's own M1 reader.

**Architecture:** Four responsibility-aligned units (mirrors M1): `crc32.rs`
(CRC32), `gpt.rs` write side, `fat32.rs` `format`+`mkdir`, `disk.rs` orchestrator
driven by a thin `mkdisk` wasm tool + `ruos_mkdisk` host fn. Design detail:
`docs/superpowers/specs/2026-06-03-disk-authoring-design.md`.

**Tech Stack:** Rust `no_std`, `BlockDevice`/`PartitionDevice` traits, limine,
wasm32-wasip1 tool. Build via WSL (`make iso`, `make run-test`). Branch:
`feature/disk-authoring`.

**Conventions:** untrusted-disk-safe arithmetic (checked/saturating, no panic on
device-reported sizes — like M1's hardened read path). All multi-byte on-disk
fields little-endian. Sector = 512 B. Match surrounding code style. One CHANGELOG
entry at the end (next number 210).

**Host-test harness note:** kernel files start with `//!` inner-doc comments, so
`include!()` fails (E0753). To host-test a kernel source, `cp` it into a throwaway
host crate's `src/` and load via `pub mod X;` (module system), as M1 did. Do NOT
add `#[cfg(test)]` that breaks the `x86_64-unknown-none` build.

---

### Task 1: CRC32

**Files:**
- Create: `kernel/src/crc32.rs`
- Modify: `kernel/src/main.rs` (add `mod crc32;`)

- [ ] **Step 1: Write `crc32.rs`** — reflected CRC32, poly `0xEDB88320`.

```rust
//! Reflected CRC-32 (IEEE 802.3 / zlib / GPT). Init 0xFFFFFFFF, input+output
//! reflected, final XOR 0xFFFFFFFF.

/// CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn known_vector() { assert_eq!(crc32(b"123456789"), 0xCBF4_3926); }
    #[test] fn empty() { assert_eq!(crc32(b""), 0x0000_0000); }
    #[test] fn one_byte() { assert_eq!(crc32(&[0x00]), 0xD202_EF8D); }
}
```

(Bitwise is fine — GPT buffers are ≤ a few sectors; no perf concern. A 256-entry
table is an acceptable alternative if the implementer prefers; keep the same API.)

- [ ] **Step 2: `mod crc32;` in main.rs** next to `mod gpt;`.

- [ ] **Step 3: Host-test the three vectors** (cp into a host crate, `cargo test`).
Expected: 3 pass. `0xCBF43926` for `"123456789"` is the canonical check value.

- [ ] **Step 4: Build the kernel** — `make iso` compiles clean (no_std).

- [ ] **Step 5: Commit** — `git add kernel/src/crc32.rs kernel/src/main.rs && git commit -m "feat(crc32): reflected CRC-32 (IEEE 802.3 / GPT)"`

---

### Task 2: GPT write side + CRC validation on read

**Files:**
- Modify: `kernel/src/gpt.rs` (add `Extent`, `GptError`, `write_layout`; CRC check in `parse`)

Reuses existing `TYPE_ESP`, `TYPE_MS_BASIC_DATA`, `rd_u32/rd_u64`, `BlockDevice`.
GPT field offsets (header, 92 bytes): sig[0..8]="EFI PART", rev[8..12]=`00000100`,
hdr_size[12..16]=92, hdr_crc[16..20], reserved[20..24]=0, my_lba[24..32],
alt_lba[32..40], first_usable[40..48], last_usable[48..56], disk_guid[56..72],
part_entry_lba[72..80], num_entries[80..84]=128, entry_size[84..88]=128,
array_crc[88..92]. Entry (128 B): type_guid[0..16], unique_guid[16..32],
first_lba[32..40], last_lba[40..48], attrs[48..56], name[56..128] UTF-16LE.

- [ ] **Step 1: Add `Extent` + `GptError`.**
```rust
pub struct Extent { pub first_lba: u64, pub sectors: u64 }
#[derive(Debug)] pub enum GptError { TooSmall, Io }
```

- [ ] **Step 2: Write `write_layout`.** Pseudocode (implement fully):
```
fn write_layout(dev, esp_sectors) -> Result<(Extent, Extent), GptError> {
    if dev.block_size() != 512 { return Err(Io); }
    let total = dev.block_count();                         // sectors
    let entries_sectors = 32;                              // 128*128/512
    let first_usable = 2 + entries_sectors;                // LBA 34
    let backup_hdr_lba = total - 1;
    let backup_arr_lba = backup_hdr_lba - entries_sectors; // 32 sectors before
    let last_usable = backup_arr_lba - 1;
    let esp_first = 2048;                                  // 1 MiB align
    let esp_last  = esp_first + esp_sectors - 1;
    let data_first = align_up(esp_last + 1, 2048);
    let data_last  = last_usable;
    if esp_first < first_usable || data_first > data_last || esp_last >= data_first {
        return Err(TooSmall);
    }
    // 1. protective MBR (LBA0): zero, one 0xEE entry @0x1BE:
    //    boot=0, chs=0x000200, type=0xEE, chs_end=max, start_lba=1,
    //    size=min(total-1, 0xFFFFFFFF); sig 55AA @510.
    // 2. build 128-entry array (32 sectors) in a buffer:
    //    entry0=ESP(esp_first..esp_last, TYPE_ESP, name "EFI System"),
    //    entry1=DATA(data_first..data_last, TYPE_MS_BASIC_DATA, name "ruos-data"),
    //    each unique_guid = fresh random (see Step 3); rest zero.
    //    array_crc = crc32(whole 128*128 buffer).
    // 3. build primary header (92B, crc field zeroed) with my_lba=1,
    //    alt_lba=backup_hdr_lba, first/last_usable, disk_guid=random,
    //    part_entry_lba=2, num=128, esize=128, array_crc; then hdr_crc=
    //    crc32(first 92 bytes with hdr_crc=0); write hdr_crc into [16..20].
    // 4. write: array @LBA2 (32 sec), primary header @LBA1, protective MBR @LBA0,
    //    backup array @backup_arr_lba, backup header @backup_hdr_lba
    //    (same fields but my_lba=backup_hdr_lba, alt_lba=1, part_entry_lba=
    //    backup_arr_lba, recompute hdr_crc).
    Ok((Extent{first_lba:esp_first, sectors:esp_sectors},
        Extent{first_lba:data_first, sectors:data_last-data_first+1}))
}
```
Use `crate::crc32::crc32`. `align_up(x,a)= (x + a-1)/a*a` with checked math.
Write sectors via `dev.write_blocks`. Map block errors → `GptError::Io`.

- [ ] **Step 3: Unique GUIDs.** Fill `disk_guid` + each `unique_guid` with 16
random bytes from the in-tree CSPRNG (find how M1/SSH seeds it — e.g.
`crate::random`/`getrandom` shim). If no easy kernel RNG is reachable here,
derive from the TSC + a per-entry counter (document it); uniqueness is
nice-to-have for v1, validity (CRCs) is the hard requirement.

- [ ] **Step 4: Add CRC validation to `parse`.** After the "EFI PART" check:
read `hdr_size` (clamp ≤512), compute `crc32` of the header with bytes[16..20]
zeroed, compare to stored hdr_crc → mismatch ⇒ `return None`. After reading the
entry array, compute `crc32` over `num*esize` bytes, compare to array_crc[88..92]
→ mismatch ⇒ `None`. Keep all the M1 bounds/`esize==128`/`.ok()?` guards. Result:
malformed/corrupt GPT → None → LBA-0 fallback (unchanged contract).

- [ ] **Step 5: Host test** (cp gpt.rs + crc32.rs + blockdev.rs into a host
crate). Test: `write_layout` on a `MemDev` of N sectors, then `parse` the same
MemDev → returns 2 partitions, `is_esp()`/`is_basic_data()`, extents match the
returned `Extent`s, and the CRCs validate (parse succeeds *because* CRCs are
right — proves the writer + the new read-side check agree). Add a negative test:
flip one header byte → `parse` returns None (CRC catches it).

- [ ] **Step 6: Build** — `make iso` clean.

- [ ] **Step 7: Regression** — `make run-gpt-test` → `TEST_PASS_GPT` (the M1
test disk is a real sgdisk GPT with valid CRCs; the new `parse` CRC check must
still accept it). If it fails: the CRC computation disagrees with sgdisk —
debug the header byte range / array length used for the CRC.

- [ ] **Step 8: Commit** — `git add kernel/src/gpt.rs && git commit -m "feat(gpt): write_layout (GPT+protective MBR+backup, CRC32) + validate CRC on parse"`

---

### Task 3: FAT32 format (mkfs)

**Files:**
- Modify: `kernel/src/vfs/fat32.rs` (add `pub fn format`)

Existing: `Bpb` (parse-only), `SECTOR`, `Inner`, `from_blockdev`. Add a writer.

- [ ] **Step 1: Geometry.** From `dev.block_count()` (= partition sectors), pick
`sec_per_cluster` by volume size (FAT32 standard table):
```
<= 260 MiB(532480 sec): not FAT32 (reject TooSmall for our use, or 1)
<= 8 GiB:   8
<= 16 GiB: 16
<= 32 GiB: 32
>  32 GiB: 64
```
reserved=32, num_fats=2, root_cluster=2. Compute `fat_sz32` so the FATs cover all
data clusters:
```
tot = block_count (as u32, clamp)
data_after_reserved = tot - reserved
// each FAT entry = 4 bytes; sec_per_fat such that:
//   2*sec_per_fat (FATs) + clusters*sec_per_cluster = data_after_reserved
//   clusters = (data_after_reserved - 2*sec_per_fat) / sec_per_cluster
// solve: sec_per_fat = ceil( (data_after_reserved + 2*sec_per_cluster*? ) ... )
// Use the standard FAT spec formula (fatgen103) — TmpVal1/TmpVal2:
let tmp1 = data_after_reserved - reserved? ...   // follow fatgen103 exactly
```
Implement the **fatgen103** `sec_per_fat` formula precisely (the canonical
mkfs.fat math) so `fsck.fat` accepts the result. Keep all math checked.

- [ ] **Step 2: Write boot sector (BPB).** Offsets: jmp[0..3]=`EB 58 90`,
oem[3..11]=`MSWIN4.1`, byts_per_sec[11..13]=512, sec_per_clus[13]=…,
rsvd[14..16]=32, num_fats[16]=2, root_ent[17..19]=0, tot16[19..21]=0,
media[21]=0xF8, fatsz16[22..24]=0, …, tot32[32..36]=tot, fatsz32[36..40]=…,
extflags[40..42]=0, fsver[42..44]=0, root_clus[44..48]=2, fsinfo[48..50]=1,
bkboot[50..52]=6, drvnum[64]=0x80, bootsig[66]=0x29, volid[67..71]=random,
vollab[71..82]="RUOS       ", filsystype[82..90]="FAT32   ", sig[510..512]=55AA.

- [ ] **Step 3: FSInfo (sector 1) + backup boot (sector 6).** FSInfo:
lead_sig[0..4]=0x41615252, struc_sig[484..488]=0x61417272, free_count[488..492]=
(unknown=0xFFFFFFFF or computed), next_free[492..496]=2 or 3, trail_sig[508..512]=
0xAA550000. Backup boot = copy of the boot sector at sector 6; FSInfo copy at 7.

- [ ] **Step 4: FATs + root.** Zero both FATs (`fat_sz32` sectors each, starting
at LBA `reserved`). Then set FAT[0]=`0x0FFFFFF8` (media|EOC), FAT[1]=`0x0FFFFFFF`,
FAT[2]=`0x0FFFFFFF` (root cluster EOC) in BOTH FAT copies. Zero the root-dir
cluster (cluster 2 → its sectors). Write efficiently (zero a sector buffer, loop).

- [ ] **Step 5:** `pub fn format(dev: &mut dyn BlockDevice) -> Result<(), VfsError>`
ties Steps 1-4. Returns `VfsError::IoError` on write failure / `TooSmall`-ish.

- [ ] **Step 6: Build** — `make iso` clean.

- [ ] **Step 7: Commit** — `git add kernel/src/vfs/fat32.rs && git commit -m "feat(fat32): format (mkfs.fat32 — BPB+FSInfo+FATs+root, fatgen103 geometry)"`

---

### Task 4: FAT32 mkdir + parent-dir-chain extend

**Files:**
- Modify: `kernel/src/vfs/fat32.rs` (implement `mkdir`; add dir-chain-extend helper)

- [ ] **Step 1: Dir-chain-extend helper** on `Inner`: given a dir's last cluster
that is full, `alloc_cluster()`, link it (set the old last cluster's FAT entry to
the new cluster, new cluster's entry = EOC — `alloc_cluster` already sets EOC),
zero the new cluster. Used when adding a record to a full directory. Refactor
`create_file`'s "parent dir full → NoSpace" path to use it too.

- [ ] **Step 2: Implement `mkdir`** (replace the `Unsupported` stub in the
`FileSystem` impl). Steps: resolve parent dir cluster (walk path); reject if the
name exists; `alloc_cluster()` for the new dir; zero it; write `.` (8.3 name
`.          `, ATTR_DIRECTORY=0x10, cluster=new) and `..` (`..         `, cluster=
parent, **0 if parent is root** per FAT spec) as the first two 32-byte records;
add an 8.3 record (name, ATTR_DIRECTORY, cluster=new, size=0) to the parent dir
(using Step 1's extend if the parent is full). Mirror the existing `create_file`
record-writing logic.

- [ ] **Step 3: Build** — `make iso` clean.

- [ ] **Step 4: Smoke (interim).** Until the m2a test exists (Task 6), verify via
a quick boot: temporarily, after mounting `/mnt`, the kernel can `mkdir` a subdir
on the existing FAT and the existing `readdirtest`/`ls` shows it — OR defer the
real proof to Task 6's round-trip. Don't add throwaway test code that ships; rely
on Task 6. At minimum: `make run-test` stays green (mkdir is additive; existing
file create/write/read unchanged).

- [ ] **Step 5: Commit** — `git add kernel/src/vfs/fat32.rs && git commit -m "feat(fat32): mkdir + parent-dir-chain extend (. / .. entries)"`

---

### Task 5: disk::author orchestrator + ruos_mkdisk host fn + mkdisk wasm tool

**Files:**
- Create: `kernel/src/disk.rs`
- Modify: `kernel/src/main.rs` (`mod disk;`)
- Modify: `kernel/src/wasm/host/…` (add `ruos_mkdisk`; register it — find where other `ruos_*` host fns are `func_wrap`'d, e.g. `wasm/host/proc.rs`)
- Create: `user/mkdisk/` (Cargo crate, wasm32-wasip1, thin)
- Modify: `Makefile` (`BIN_TOOLS += mkdisk`), `limine.conf` (module entry for `/bin/mkdisk.wasm`)

- [ ] **Step 1: `disk.rs`**:
```rust
//! Disk authoring orchestrator (M2a): GPT + format + dir tree on a raw disk.
pub enum DiskError { TooSmall, Io, NoPort }
pub struct Layout { pub esp: crate::gpt::Extent, pub data: crate::gpt::Extent }

pub fn author(dev: &mut dyn crate::blockdev::BlockDevice, esp_mib: u32)
        -> Result<Layout, DiskError> {
    let esp_sectors = (esp_mib as u64) * 1024 * 1024 / 512;
    let (esp, data) = crate::gpt::write_layout(dev, esp_sectors)
        .map_err(|_| DiskError::TooSmall)?;
    // format ESP, make /EFI/BOOT
    { let mut pd = crate::blockdev::PartitionDevice::new(/* see note */, esp.first_lba, esp.sectors);
      crate::vfs::fat32::format(&mut pd).map_err(|_| DiskError::Io)?;
      // mount the ESP at a temp prefix to mkdir, OR add a direct mkdir-on-blockdev.
    }
    // format data
    Ok(Layout { esp, data })
}
```
**Design note for the implementer:** `PartitionDevice::new` takes a
`Box<dyn BlockDevice+Send>` (owns the inner). `author` borrows `dev: &mut`.
Resolve this cleanly: EITHER (a) `author` takes the owned device
(`Box<dyn BlockDevice+Send>`) and threads it through PartitionDevices (re-wrap
per partition — but PartitionDevice consumes the box, so you need the device
back; give PartitionDevice an `into_inner()` or make `author` take ownership and
build two PartitionDevices by cloning a cheap handle), OR (b) the simpler path:
have `format` and the mkdir operate on a `&mut dyn BlockDevice` and add a
`PartitionDevice` variant that borrows (`&mut dyn`) instead of owning — a
`PartBorrow<'a>{ inner: &'a mut dyn BlockDevice, base, count }`. **Recommended:
(b)** — add a borrowing partition view (small, avoids ownership churn); the
mount-time `PartitionDevice` (owning) stays for M1. Implement whichever is
cleanest; keep the partition isolation (checked base+lba, clamp to count).
For **mkdir on the ESP**: mounting at `/mnt` conflicts with the data mount;
instead add `fat32::format_and_mkdirs(dev, &["/EFI","/EFI/BOOT"])` or expose a
`Fat32Fs::from_blockdev` + direct `mkdir` calls on that fs object WITHOUT going
through the global VFS mount table. Pick the path that avoids touching the
`/mnt` mount.

- [ ] **Step 2: `ruos_mkdisk` host fn.** Find the host-fn registration site
(`grep func_wrap kernel/src/wasm`). Add `ruos_mkdisk(esp_mib: i32) -> i32`
(0=ok, negative=error code). It acquires the first SATA `AhciPort`
(`crate::ahci::AhciPort::bringup` / the global PORT0 path used by storage.rs),
calls `disk::author(&mut port, esp_mib as u32)`, logs the layout via `binfo!`,
returns status. **Destructive** — it wipes the disk. (Target selection / "is this
the boot disk" is M2b; for M2a it operates on the first SATA port, which in the
test is the blank target.)

- [ ] **Step 3: `user/mkdisk/` wasm tool.** Mirror an existing thin tool
(`user/mkdir` or similar). `main()`: parse optional `esp_mib` arg (default 64),
print a destructive-warning line, call the `ruos_mkdisk` import, print result.
Declare the import:
```rust
#[link(wasm_import_module = "ruos")]
extern "C" { fn mkdisk(esp_mib: i32) -> i32; }   // matches func_wrap name
```
(Match the exact import module/name used by other `ruos_*` host fns.)

- [ ] **Step 4: Wire build.** `Makefile`: add `mkdisk` to `BIN_TOOLS`.
`limine.conf`: add a module entry `module_path: boot():/bin/mkdisk.wasm` +
`module_cmdline: /bin/mkdisk.wasm` (copy an existing tool's two lines).

- [ ] **Step 5: Build** — `make iso` clean (kernel + the new wasm tool both
compile; the tool lands in `/bin`).

- [ ] **Step 6: Commit** — `git add kernel/src/disk.rs kernel/src/main.rs kernel/src/wasm user/mkdisk Makefile limine.conf && git commit -m "feat(disk): author orchestrator + ruos_mkdisk host fn + mkdisk tool"`

---

### Task 6: m2a test (host-verify + ruos round-trip) + changelog

**Files:**
- Create: `tests/m2a-test.sh`
- Modify: `Makefile` (`run-m2a-test` target), `user-bin/smoke.sh` (or a dedicated init) to run `mkdisk` + a round-trip
- Create: `CHANGELOG/210-26-06-03-disk-authoring.md`

- [ ] **Step 1: Decide the in-guest sequence.** The booted ruos must, on a blank
second disk: run `mkdisk 64`, then mount the new **data** partition and write+read
a marker. Two sub-options: (i) `mkdisk` already triggers `disk::author`; for the
round-trip, after `mkdisk` the disk now has a GPT → on the NEXT boot, M1's
`storage.rs` would auto-mount the data partition. So the test can be **two-phase**:
boot 1 runs `mkdisk` (authors the disk); boot 2 (same disk, no re-author) lets M1
auto-mount `/mnt` from the new data partition and `cat`s a marker written by a
prior `cp`. OR (ii) single boot: `mkdisk` then a `mountdata`-style step. **Use
(i) two-phase** — it also proves persistence + that M1 mounts ruos-authored GPTs.
Concretely: boot1 init runs `mkdisk 64` then `cp /etc/init.sh /mnt-after?`… —
simplest: boot1 just `mkdisk 64`; boot2 (M1 auto-mounts data part as /mnt) runs
`echo gpt-self-mk > /mnt/MK.TXT; cat /mnt/MK.TXT`. If writing on boot2 then
reading same boot is enough, one round-trip boot after authoring suffices.

- [ ] **Step 2: `tests/m2a-test.sh`** (no loop devices; QEMU AHCI):
```bash
#!/usr/bin/env bash
set -u
cd "$(dirname "$0")/.."
IMG=build/m2a-disk.img; ISO=build/os.iso; S=build/serial.log
killq(){ ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done; }
killq; sleep 1
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none      # blank target
boot(){ timeout "$1" qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci \
  -device ide-hd,drive=d0,bus=ahci.0 > "$S" 2>&1 & QP=$!; }
# phase 1: author
boot 90
for _ in $(seq 1 45); do grep -qE "mkdisk: (ok|done|authored)" "$S" && break; kill -0 $QP 2>/dev/null||break; sleep 2; done
killq; cp "$S" build/serial.m2a1.log
grep -qE "mkdisk: (ok|done|authored)" build/serial.m2a1.log || { echo TEST_FAIL_AUTHOR; tail -25 build/serial.m2a1.log; exit 1; }
# host verify GPT + FATs (read-only; never write the img)
sgdisk -v "$IMG" 2>&1 | grep -qiE "No problems|found" || { echo TEST_FAIL_SGDISK; sgdisk -v "$IMG"; exit 1; }
# (extract ESP+data offsets via sgdisk -i; fsck.fat -n each; mtools list /EFI/BOOT on ESP)
# ... compute DLBA/ELBA with sgdisk -i 1 / -i 2, dd each partition out, fsck.fat -n, mdir
# phase 2: round-trip (M1 auto-mounts the authored data partition)
boot 120
for _ in $(seq 1 55); do grep -qF "m2a-roundtrip-ok" "$S" && break; kill -0 $QP 2>/dev/null||break; sleep 2; done
killq
grep -qF "m2a-roundtrip-ok" "$S" || { echo TEST_FAIL_ROUNDTRIP; tail -25 "$S"; exit 1; }
echo TEST_PASS_M2A
```
Fill in the ESP/data extraction (sgdisk `-i 1`/`-i 2` → First sector; `dd` the
partition to a temp file; `fsck.fat -n`; `mcopy -i`/`mdir` to confirm `/EFI/BOOT`).
The init for boot1 must run `mkdisk 64`; for boot2, run the write+read of
`m2a-roundtrip-ok` against the auto-mounted `/mnt`. Use a dedicated init script
(e.g. `user-bin/m2a-init.sh`) selected via `INIT_SCRIPT=` for this target, so
`make run-test`/`run-gpt-test` are unaffected.

- [ ] **Step 2b: init script(s).** Create `user-bin/m2a-init.sh`:
```sh
echo ruos boot OK
mkdisk 64
echo --- m2a roundtrip ---
echo m2a-roundtrip-ok > /mnt/MK.TXT 2>/dev/null
cat /mnt/MK.TXT 2>/dev/null
```
(On boot1 `/mnt` may be absent before authoring → the write is silent; the author
line `mkdisk: ok` is what phase 1 greps. On boot2 M1 mounts the authored data
partition → the write+read emits `m2a-roundtrip-ok`.) Confirm `mkdisk` prints a
stable success token (`mkdisk: ok` or similar) — align the grep with the tool's
actual output.

- [ ] **Step 3: Makefile target:**
```make
.PHONY: run-m2a-test
run-m2a-test:
	@$(MAKE) iso INIT_SCRIPT=user-bin/m2a-init.sh
	bash tests/m2a-test.sh
```

- [ ] **Step 4: Run all gates.**
  - `make run-m2a-test` → `TEST_PASS_M2A` (author + sgdisk/fsck/mtools + round-trip).
  - `make run-gpt-test` → `TEST_PASS_GPT` (M1 read, incl. new CRC check).
  - `make run-test` → `TEST_PASS` (raw-FAT regression).
  Kill stray qemu (comm form) between gates; run sequentially.

- [ ] **Step 5: CHANGELOG** `CHANGELOG/210-26-06-03-disk-authoring.md` (Cosa/
Perché/File toccati/Verifica — list crc32.rs, gpt.rs, fat32.rs, disk.rs,
ruos_mkdisk, user/mkdisk, tests/m2a-test.sh; verification = the three gates).

- [ ] **Step 6: Commit** — `git add tests/m2a-test.sh Makefile user-bin/m2a-init.sh CHANGELOG/ && git commit -m "test(disk): m2a authoring test (sgdisk+fsck+mtools+round-trip) + changelog"`

---

## Self-review notes (controller)

- Type consistency: `Extent{first_lba,sectors}` used in gpt.rs (T2), disk.rs (T5).
  `format(&mut dyn BlockDevice)` (T3) called by author (T5) via the borrowing
  partition view. `crc32(&[u8])->u32` (T1) used in gpt.rs (T2).
- The PartitionDevice ownership wrinkle (T5 Step 1) is the main integration risk
  — flagged with a recommended fix (borrowing partition view). The implementer
  should resolve it before wiring author.
- mkdir-on-ESP must avoid the global `/mnt` mount table (T5 note) — use a
  `Fat32Fs` object directly or a `format_and_mkdirs` helper.
- Host-tool deps for the test: `sgdisk`, `fsck.fat` (dosfstools), `mtools` — all
  present in the WSL env (M1 used sgdisk + mkfs.vfat + mcopy).
```
