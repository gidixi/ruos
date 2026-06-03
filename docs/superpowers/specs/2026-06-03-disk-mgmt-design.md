# Disk management — `disks` (list) + `umount` — Design

**Date:** 2026-06-03
**Status:** approved (design)

## Goal

Two small disk-management capabilities surfaced by real-HW (VBox) testing:
1. **`umount <path>`** — unmount a filesystem (esp. `/mnt`). This is the
   important one: the `install` guard refuses while `/mnt` is mounted, so on a
   disk that already has ruos data (M1 auto-mounts it at boot) you currently
   can't (re)install. `umount /mnt` releases the FAT + its SATA port → the guard
   passes and `install <n>` proceeds.
2. **`disks`** — a clean read-only listing of the SATA disks (index, model,
   size), instead of today's messy `install`-no-arg output (the disk lines are
   kernel `binfo` logs interleaved with the tool's hints).

## Background (verified)

- `vfs::MOUNTS: Mutex<Vec<(String, FsImpl)>>`; `mount()` pushes, `is_mounted()`
  checks. No `unmount` exists. `FsImpl::Fat32(Fat32Fs)` holds the FAT over its
  SATA port (`Arc<Mutex<Inner{dev: Box<PartitionDevice<AhciPort>>}>>`). Removing
  the MOUNTS entry drops the `Fat32Fs`; if no open `Fat32File` still holds an
  `Arc` clone of the inner, that drops the `Inner` → `PartitionDevice` →
  `AhciPort` → **the port is released**. So `install`'s `acquire_port` can then
  re-grab it.
- Host-fn patterns to mirror: `ruos_chdir(path_ptr, path_len)` (proc.rs:51) reads
  a path from wasm via `crate::wasm::host::mem::guest_read`; `ruos_pci_list`
  (proc.rs:233) formats a listing into a wasm buffer (the write-to-guest pattern
  — find its `guest_write`/memory-export use). `ahci::sata_ports()` +
  `acquire_port(idx)` (giving `AhciPort.model`/`.sectors`) enumerate disks.

## Architecture

### 1. `vfs::unmount` — `kernel/src/vfs/mod.rs`
```
pub fn unmount(prefix: &str) -> Result<(), VfsError> {
    if prefix == "/" { return Err(VfsError::InvalidPath); }   // never unmount root
    let mut m = MOUNTS.lock();
    let before = m.len();
    m.retain(|(p, _)| p != prefix);
    if m.len() == before { Err(VfsError::NotFound) } else { Ok(()) }
}
```
Removing the entry drops the `FsImpl` (and releases the port when no file keeps a
ref). v1: no busy-check — if a file is open on the mount it stays alive until
that fd closes; in the install flow nothing has `/mnt` open right after boot
(the shell's cwd is tmpfs `/`). Document this.

### 2. `ruos_umount` host fn + `umount` tool
- `ruos_umount(path_ptr, path_len) -> i32` (proc.rs): `guest_read` the path
  (mirror `ruos_chdir`), `vfs::unmount(path)` → `0` ok / `-1` not mounted / `-2`
  invalid (root). Register `func_wrap("ruos","umount", …)`.
- `user/umount/` tool: `umount <path>` (require an arg; no default — explicit).
  `0` → `umount: <path> unmounted`; `-1` → `umount: <path> not mounted`; `-2` →
  `umount: cannot unmount that`. Add to `BIN_TOOLS` + `limine.conf`.

### 3. `ruos_sata_list` host fn + `disks` tool
- `ruos_sata_list(buf_ptr, buf_cap: i32) -> i32` (proc.rs): for each
  `ahci::sata_ports()` idx, `acquire_port(idx)` → format one line
  `"{idx}\t{model}\t{mib} MiB\n"` (`mib = sectors/2048`); concatenate; write into
  the guest buffer at `buf_ptr` (≤ `buf_cap`, mirror `ruos_pci_list`'s
  guest-write); return the byte count written (or `-1` if it wouldn't fit). No
  disk → return 0.
- `user/disks/` tool: alloc a `[u8; 1024]` buffer, call `ruos_sata_list(ptr,
  1024)`, print a header `IDX  MODEL                SIZE` then the returned text
  (the lines are already `idx\tmodel\tsize`; print with simple column spacing or
  just tab-separated). Add to `BIN_TOOLS` + `limine.conf`.

### 4. `install` (no arg) cleanup — `ruos_install` + `user/install`
- `ruos_install` LIST mode (`target < 0`): STOP `binfo`-logging the disks (drop
  that loop); just `return Ok(-10)`.
- `install` tool, no disk arg: print `install: run \`disks\` to list the SATA
  disks, then \`install <n>\` to install onto disk <n>` (no kernel log spam).
  Keep `install <n>` install behavior unchanged.

## Error handling / limits

- `unmount("/")` refused. Unmounting a non-existent prefix → `-1`.
- v1 has no open-file busy check: unmounting while a file is open leaves the port
  held until the fd closes (the new install's `acquire_port` could then conflict
  on the same physical port). Acceptable for the boot-then-umount-then-install
  flow; a busy-check (refuse if any fd references the mount) is a future hardening.

## Testing

- In-guest: boot with a disk that M1 auto-mounts at `/mnt` (a GPT disk, e.g. the
  one `mkboot` or `run-gpt-test` builds). Run `umount /mnt` → assert
  `umount: /mnt unmounted` AND that a following `install 0` is NOT refused (no
  `/mnt is mounted` — it proceeds to author, proving the port was freed). And
  `disks` lists the disk(s) cleanly (a line with the model + size).
- Regression: `run-test`, `run-m2b2-test`, `run-gpt-test`, `run-m2a-test` green.

## Files touched

- `kernel/src/vfs/mod.rs` — `unmount`.
- `kernel/src/wasm/host/proc.rs` — `ruos_umount`, `ruos_sata_list`; `ruos_install`
  LIST-mode no longer binfo-logs.
- `user/umount/`, `user/disks/` — NEW tools; `user/install/src/main.rs` (no-arg
  message); `Makefile` (`BIN_TOOLS`), `limine.conf` (2 module entries).
- A test + CHANGELOG 221.
