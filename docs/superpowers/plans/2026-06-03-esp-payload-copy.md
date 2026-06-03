# M2b-1 ESP boot-payload copy (LFN file-write) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Let ruos write its full boot tree (kernel + BOOTX64.EFI + limine.conf +
all `.wasm` modules, with correct long names) onto an authored ESP, so the SSD
boots standalone.

**Architecture:** (1) ship kernel/BOOTX64.EFI/limine.conf as Limine modules read
via `m.data()`; (2) add FAT32 `write_file` + **LFN write** to M2a's `FatWriter`;
(3) `copy_boot_payload` replicates the ISO boot tree on the ESP. Design:
`docs/superpowers/specs/2026-06-03-esp-payload-copy-design.md`.

**Tech Stack:** Rust no_std, M2a's `FatWriter`/`PartBorrow`/`disk::author`,
limine modules. Build via WSL. Branch: `feature/esp-payload-copy`. Changelog 211.

**Conventions:** checked/saturating FAT math (M2a discipline), all on-disk fields
LE, sector 512. Match surrounding style. Host-test by `cp`-ing kernel sources into
a throwaway `/tmp` crate + `pub mod` (M2a pattern). Kill stray qemu via
`ps -eo pid,comm | awk '/qemu-system/{print $1}'` (NOT `pgrep -f qemu`).

---

### Task 1: FatWriter `write_file` + LFN write

**Files:** Modify `kernel/src/vfs/fat32.rs` (`FatWriter` impl + LFN helpers).

This is the core task. Read M2a's `FatWriter` first (`open`, `alloc_cluster`,
`add_dir_record`, `write_fat_entry`, `write_cluster`, `mkdir`, `find_subdir`,
`create_dirs`, the `Bpb` geometry, consts `DIR_ENTRY_SIZE=32`, `ATTR_DIRECTORY`,
`ATTR_ARCHIVE`, `SECTOR`, `EOC`). Reuse `create_dirs`' component-walk to make
parent dirs.

- [ ] **Step 1: short-name generation.** `fn short_name(long: &str, dir_cluster, &mut self) -> Result<[u8;11], VfsError>`:
  - Uppercase ASCII; map chars not in `[A-Z0-9_~!@#$%^&()-{}]` to `_`; drop spaces.
  - Split on the LAST `.`: stem + ext. Basis = first 8 of stem, ext field = first 3 of ext (space-padded to 8 and 3).
  - Our inputs are always lossy (4-char `.wasm` ext, `limine.conf`, or >8 stem), so ALWAYS use the numeric-tail form: basis truncated so `~<n>` fits in 8 (e.g. ≤6 chars + `~1`). Try `n=1,2,3,...`, building the 11-byte short name `BASIS~n` (name field, space-padded) + `EXT` (ext field), and scan the target dir's existing entries (read each cluster, decode the 8.3 of non-LFN/non-free records) for a collision; first non-colliding `n` wins. Return the 11-byte short name.
  - (8.3 examples: `ls.wasm`→`LS~1    WAS`; `readdirtest.wasm`→`READDI~1WAS`; `limine.conf`→`LIMINE~1CON`.)

- [ ] **Step 2: checksum.** `fn lfn_checksum(short: &[u8;11]) -> u8`:
```rust
let mut sum: u8 = 0;
for &c in short { sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(c); }
sum
```

- [ ] **Step 3: build the LFN entry run.** `fn build_lfn_run(long: &str, short: &[u8;11]) -> Vec<[u8;32]>`:
  - UTF-16LE encode `long` (ASCII → 1 unit each). `n = ceil(units.len()/13)` LFN entries.
  - For logical chunk `k` (0-based, 13 units each): place units into a 32-byte entry at byte offsets **1..11 (5 units), 14..26 (6 units), 28..32 (2 units)**; byte 11 = `0x0F` (attr LFN); byte 12 = 0; byte 13 = checksum; bytes 26..28 = 0. After the last real unit write `0x0000` then pad remaining slots with `0xFFFF`.
  - Sequence byte (byte 0) for chunk `k` = `k+1`; the LAST chunk (highest k) gets `| 0x40`.
  - Return the entries in **physical order = reverse logical**: highest sequence (0x40) FIRST, sequence 1 LAST. (So `out` = entries for k=n-1, n-2, …, 0.)

- [ ] **Step 4: contiguous-run dir insert.** Extend/add `fn add_dir_run(&mut self, dir_cluster: u32, run: &[[u8;32]]) -> Result<(), VfsError>`: find `run.len()` **consecutive** free slots (first byte `0x00` or `0xE5`) within ONE cluster of the dir's chain; if no cluster has a long-enough consecutive free run, `alloc_cluster` + link it and place the run at its start. Write all entries. (LFN entries + the short entry must be physically consecutive and not split across clusters.)

- [ ] **Step 5: `write_file`.**
```rust
pub fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), VfsError> {
    // split path → parent dir components + file name (last); create parent dirs
    //   via the same walk as create_dirs (mkdir missing components).
    // resolve parent_cluster.
    // build the file: if bytes empty → first_cluster 0; else alloc a chain:
    //   c0 = alloc_cluster(); write bytes cluster-by-cluster (zero-pad last);
    //   link each next cluster with write_fat_entry(prev, next); last = EOC
    //   (alloc_cluster already sets EOC + zeroes).
    // short = self.short_name(name, parent_cluster)?;  run = build_lfn_run(name, &short);
    // build the 8.3 short record: short[0..11], attr ATTR_ARCHIVE, cluster hi/lo
    //   (bytes 20..22 / 26..28), size = bytes.len() (bytes 28..32).
    // self.add_dir_run(parent_cluster, &[run.., short_record])?;
    Ok(())
}
```
  Note: the parent-dir walk must NOT recreate `/EFI`/`/EFI/BOOT` if they exist
  (reuse `find_subdir`); and component names like `boot`,`bin`,`etc`,`root` ARE
  valid 8.3 (short dirs) — `mkdir` handles them as today.

- [ ] **Step 6: build** — `cd kernel && cargo build --release` clean.

- [ ] **Step 7: host-verify LFN (critical).** cp `fat32.rs`+`blockdev.rs` into a `/tmp` host crate over a `MemDev`; `format` a 64 MiB MemDev, `FatWriter::open`, then:
  - `write_file("/bin/ls.wasm", b"\x00asm....")` (small binary), `write_file("/bin/readdirtest.wasm", &[0xAB;5000])` (multi-cluster + 11-char stem), `write_file("/boot/limine/limine.conf", b"timeout: 0\n...")` (nested dirs + 4-char ext).
  - dump → `/tmp/esp.img`; assert:
    - `fsck.fat -n /tmp/esp.img` → clean (no LFN/checksum/orphan errors).
    - `mdir -i /tmp/esp.img ::/bin` lists **`ls.wasm` and `readdirtest.wasm`** (long names — proves LFN read-back), `mdir ::/boot/limine` lists `limine.conf`.
    - `mcopy -i /tmp/esp.img ::/bin/readdirtest.wasm /tmp/out` + `cmp /tmp/out <(printf '\xAB%.0s' {1..5000})` → identical (proves content + size).
  Fix until all pass — fsck rejecting the LFN run = checksum/sequence/ordering bug; mdir showing a mangled name = the LFN entries are wrong. Clean up /tmp.

- [ ] **Step 8: regression** — `make run-test` → `TEST_PASS` (mounted `/mnt` path uses the async short-name driver, untouched).

- [ ] **Step 9: commit** — `git add kernel/src/vfs/fat32.rs && git commit -m "feat(fat32): FatWriter write_file + LFN long-name write"`

---

### Task 2: boot payload as Limine modules

**Files:** Modify `limine.conf`, `kernel/src/modules.rs`, (maybe `Makefile`).

- [ ] **Step 1: limine.conf** — add three module entries (near the existing ones, inside the `/ruos` boot entry):
```
    module_path: boot():/boot/kernel
    module_cmdline: /payload/kernel
    module_path: boot():/EFI/BOOT/BOOTX64.EFI
    module_cmdline: /payload/BOOTX64.EFI
    module_path: boot():/boot/limine/limine.conf
    module_cmdline: /payload/limine.conf
```
  Verify the Makefile already copies all three onto the ISO at those `module_path`
  locations: kernel→`/boot/kernel`, BOOTX64.EFI→`/EFI/BOOT/`, limine.conf→
  `/boot/limine/` (all confirmed present; no Makefile change expected — if a path
  differs, fix the `module_path` to match the actual ISO location).

- [ ] **Step 2: modules.rs — skip /payload/ in mount_all.** In `mount_all()`'s loop, `if path.starts_with("/payload/") { continue; }` (don't tmpfs-copy the multi-MB payload). Keep counting/logging sane.

- [ ] **Step 3: modules.rs — accessors.**
```rust
/// Bytes of a /payload/<name> Limine module (kernel / BOOTX64.EFI / limine.conf).
pub fn payload(name: &str) -> Option<&'static [u8]> {
    let resp = MODULES.response()?;
    let want = /* format "/payload/{name}" without alloc if possible, else use alloc::format */;
    resp.modules().iter().find(|m| m.cmdline() == want).map(|m| m.data())
}
/// Iterate every boot module as (cmdline, data) — for copy_boot_payload.
pub fn all() -> impl Iterator<Item = (&'static str, &'static [u8])> {
    MODULES.response().into_iter().flat_map(|r| r.modules().iter().map(|m| (m.cmdline(), m.data())))
}
```
  (Match the actual `limine` crate API for `cmdline()`/`data()` return types — `cmdline()` may be `&CStr`/`&str`; adapt the comparison. `data()` lifetime is `'static` for the kernel.)

- [ ] **Step 4: build + smoke** — `make iso` clean; `make run-test` → `TEST_PASS` AND the boot log should NOT error on the new modules (they load; `/payload/*` skipped from tmpfs). Optionally `binfo!` the three payload sizes once for visibility.

- [ ] **Step 5: commit** — `git add limine.conf kernel/src/modules.rs && git commit -m "feat(boot): ship kernel/BOOTX64.EFI/limine.conf as Limine modules (payload accessors)"`

---

### Task 3: copy_boot_payload + mkboot trigger

**Files:** Modify `kernel/src/disk.rs`, `kernel/src/wasm/host/proc.rs`; create `user/mkboot/`; modify `Makefile`, `limine.conf`.

- [ ] **Step 1: `copy_boot_payload` in disk.rs.**
```rust
/// Write the full boot tree onto a freshly-authored ESP (must already have
/// /EFI/BOOT from author). Reads the boot files from Limine modules.
pub fn copy_boot_payload(esp: &mut dyn crate::blockdev::BlockDevice) -> Result<(), DiskError> {
    let mut w = crate::vfs::fat32::FatWriter::open(esp).map_err(|_| DiskError::Io)?;
    // 3 payload files at their UEFI/Limine ESP locations:
    let k = crate::modules::payload("kernel").ok_or(DiskError::Io)?;
    w.write_file("/boot/kernel", k).map_err(|_| DiskError::Io)?;
    let b = crate::modules::payload("BOOTX64.EFI").ok_or(DiskError::Io)?;
    w.write_file("/EFI/BOOT/BOOTX64.EFI", b).map_err(|_| DiskError::Io)?;
    let c = crate::modules::payload("limine.conf").ok_or(DiskError::Io)?;
    w.write_file("/boot/limine/limine.conf", c).map_err(|_| DiskError::Io)?;
    // every non-payload module → its cmdline path on the ESP:
    for (cmdline, data) in crate::modules::all() {
        if cmdline.starts_with("/payload/") { continue; }
        w.write_file(cmdline, data).map_err(|_| DiskError::Io)?;
    }
    Ok(())
}
```
  Make `FatWriter::open` `pub` if it isn't. (`write_file` creates intermediate
  dirs, so `/boot`, `/boot/limine`, `/bin`, `/etc`, `/root` are made on demand.)

- [ ] **Step 2: `ruos_mkboot` host fn** in `proc.rs` (mirror `ruos_mkdisk`): acquire the first SATA port (same storage.rs pattern), `disk::author(&mut port, esp_mib)`, then `disk::copy_boot_payload(&mut port)` (author already partitioned; copy targets the ESP — wrap a `PartBorrow` over the ESP extent returned by author: `author` returns `Layout{esp,data}`, so `let mut e = PartBorrow::new(&mut port, layout.esp.first_lba, layout.esp.sectors); copy_boot_payload(&mut e)`). `binfo!("mkboot","ok ...")`; return 0/-1/-2. Register `.func_wrap("ruos","mkboot", ...)`.

- [ ] **Step 3: `user/mkboot/` tool** — mirror `user/mkdisk`: import `ruos::mkboot(esp_mib:i32)->i32`; default 64; print destructive warning + `mkboot: ok` on 0. Add `mkboot` to Makefile `BIN_TOOLS` + a `limine.conf` `/bin/mkboot.wasm` module entry.

- [ ] **Step 4: build** — `make iso` clean; `mkboot.wasm` in `/bin`.

- [ ] **Step 5: commit** — `git add kernel/src/disk.rs kernel/src/wasm user/mkboot Makefile limine.conf && git commit -m "feat(disk): copy_boot_payload + ruos_mkboot host fn + mkboot tool"`

---

### Task 4: m2b1 end-to-end test + changelog

**Files:** Create `tests/m2b1-test.sh`, `user-bin/m2b1-init.sh`; modify `Makefile`; create `CHANGELOG/211-26-06-03-esp-payload-copy.md`.

- [ ] **Step 1: init** `user-bin/m2b1-init.sh`:
```sh
echo ruos boot OK
mkboot 64
echo m2b1-done
```

- [ ] **Step 2: `tests/m2b1-test.sh`** — boot ruos (payload modules present) with a blank 512 MiB disk + `INIT_SCRIPT=user-bin/m2b1-init.sh`, wait for `mkboot: ok`, kill qemu. Then host-verify the authored+copied ESP:
```bash
#!/usr/bin/env bash
set -u; cd "$(dirname "$0")/.."
IMG=build/m2b1-disk.img; S=build/serial.log
killq(){ ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done; }
killq; sleep 1
dd if=/dev/zero of="$IMG" bs=1M count=512 status=none
make iso INIT_SCRIPT=user-bin/m2b1-init.sh > build/m2b1-iso.log 2>&1 || { echo TEST_FAIL_ISO; tail -20 build/m2b1-iso.log; exit 1; }
timeout 150 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom build/os.iso -serial stdio -display none -no-reboot -m 1024 -device qemu-xhci \
  -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci -device ide-hd,drive=d0,bus=ahci.0 > "$S" 2>&1 & QP=$!
for _ in $(seq 1 70); do grep -qF "mkboot: ok" "$S" && break; kill -0 $QP 2>/dev/null||break; sleep 2; done
killq
grep -qF "mkboot: ok" "$S" || { echo TEST_FAIL_MKBOOT; tail -30 "$S"; exit 1; }
# extract ESP (partition 1) + verify the boot tree
ELBA=$(sgdisk -i 1 "$IMG" | awk '/First sector/{print $3}')
ESEC=$(sgdisk -i 1 "$IMG" | awk '/Partition size/{print $3}')
dd if="$IMG" of=build/m2b1-esp.img bs=512 skip="$ELBA" count="$ESEC" status=none
fsck.fat -n build/m2b1-esp.img > build/m2b1-fsck.log 2>&1
grep -qiE "Dirty bit|orphan|Checksum|invalid|corrupt" build/m2b1-fsck.log && { echo TEST_FAIL_FSCK; cat build/m2b1-fsck.log; exit 1; }
mdir -i build/m2b1-esp.img ::/EFI/BOOT 2>&1 | grep -qi "BOOTX64" || { echo TEST_FAIL_BOOTX64; exit 1; }
mdir -i build/m2b1-esp.img ::/boot     2>&1 | grep -qi "kernel"  || { echo TEST_FAIL_KERNEL; exit 1; }
mdir -i build/m2b1-esp.img ::/boot/limine 2>&1 | grep -qi "limine" || { echo TEST_FAIL_CONF; exit 1; }
mdir -i build/m2b1-esp.img ::/bin 2>&1 | grep -qi "readdirtest.wasm" || { echo TEST_FAIL_LFN; mdir -i build/m2b1-esp.img ::/bin; exit 1; }
# byte-identity on the kernel + a wasm
mcopy -i build/m2b1-esp.img ::/boot/kernel build/m2b1-kernel.out 2>/dev/null
cmp build/m2b1-kernel.out kernel/target/x86_64-unknown-none/release/kernel || { echo TEST_FAIL_KERNEL_BYTES; exit 1; }
mcopy -i build/m2b1-esp.img ::/bin/ls.wasm build/m2b1-ls.out 2>/dev/null
cmp build/m2b1-ls.out user-bin/ls.wasm || { echo TEST_FAIL_LS_BYTES; exit 1; }
echo TEST_PASS_M2B1
```
  **Validate + fix against the real WSL tool output** (the `sgdisk -i` field
  parsing, `fsck.fat` benign-vs-hard messages, `mdir` long-name listing,
  `cmp` paths — the kernel ELF path is `kernel/target/x86_64-unknown-none/release/kernel`,
  the wasm sources are `user-bin/*.wasm`). The `readdirtest.wasm` mdir check is the
  LFN proof; the `cmp` checks are the byte-identity proof. Iterate until it
  passes on a good copy and would fail on a bad one.

- [ ] **Step 3: Makefile** `run-m2b1-test:` → `bash tests/m2b1-test.sh`.

- [ ] **Step 4: run ALL gates** (sequential, kill stray qemu between): `make run-m2b1-test`→`TEST_PASS_M2B1`; `make run-m2a-test`→`TEST_PASS_M2A`; `make run-gpt-test`→`TEST_PASS_GPT`; `make run-test`→`TEST_PASS`.

- [ ] **Step 5: CHANGELOG** `CHANGELOG/211-26-06-03-esp-payload-copy.md` (Cosa: payload come moduli + write_file/LFN + copy_boot_payload + mkboot; Perché: ESP avviabile = prerequisito install/boot-da-SSD M2b-2; File toccati; Verifica: i 4 gate).

- [ ] **Step 6: commit** — `git add tests/m2b1-test.sh Makefile user-bin/m2b1-init.sh CHANGELOG/ && git commit -m "test(disk): m2b1 ESP boot-tree copy (LFN names + byte-identity) + changelog"`

---

## Self-review (controller)

- Type consistency: `FatWriter::write_file(&str,&[u8])` (T1) used by `copy_boot_payload` (T3); `modules::payload`/`all` (T2) used by `copy_boot_payload` (T3); `disk::author`→`Layout{esp,data}` (M2a) feeds the `PartBorrow` in `ruos_mkboot` (T3).
- LFN is the risk: T1 host-verifies via fsck (well-formedness) + mdir (read-back) + cmp (content). Get the entry byte offsets (1..11, 14..26, 28..32), checksum, and reverse-physical ordering exactly right.
- `copy_boot_payload` host-testability is limited (needs Limine modules) → its real proof is T4 in-guest; T3 verifies compile + that `ruos_mkboot` wires author→copy correctly.
- ESP size: T4 uses a 512 MiB disk (64 MiB ESP holds kernel + 52 wasm ≈ tens of MB comfortably). `-m 1024` (the kernel ELF is loaded twice — running + as a module).
