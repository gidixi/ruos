# On-demand tools from the data partition (slim initramfs on the installed SSD) — Design

**Date:** 2026-06-03
**Status:** approved (design); bootstrap = kernel+shell+init+network/SSH, tools on-demand

## Goal

Stop cramming all ~50 `.wasm` tools onto the installed SSD's ESP as Limine
modules (loaded into RAM at every boot). On the **installed SSD**, ship a **slim
initramfs** (kernel + shell + init + the network/SSH service) on the ESP, put the
**command-line tools on the data partition** (`/mnt/bin`), and have the shell
load a tool **on-demand from the FAT** the first time it's run. The **live
ISO/USB stays unchanged** (all tools as Limine modules) — it has no readable
filesystem after boot (no USB-mass-storage driver), so its userland MUST come
from the bootloader.

## Why asymmetric (live vs installed)

- **Live (ISO/USB):** the kernel can't mount the boot medium's filesystem after
  boot (ruos has USB **HID** + hub, no USB-mass-storage driver; the SATA disk is
  the install *target*, not a tool source). So the live environment is
  necessarily an initramfs — all tools via Limine. Unchanged.
- **Installed SSD:** M1 mounts the SSD's **data partition** at `/mnt` early
  (before userland). So the installed system CAN read tools from `/mnt/bin`
  on-demand. This is where the win is: the ESP carries only the bootstrap, and
  tools load lazily (RAM only for what you actually run; the tool set lives on
  the disk, outside `limine.conf`).

## The split (approved)

- **Bootstrap (ESP, Limine modules) — needed before/independent of `/mnt`:**
  kernel, `/init.wasm`, `/etc/init.sh`, `/bin/shell.wasm`, `/root/server.wasm`,
  `/root/client.wasm` (the init chain + shell + the **network/SSH service**, so
  networking/SSH work even if `/mnt` is flaky). Plus `BOOTX64.EFI` + a **slim
  `limine.conf`** declaring only these.
- **Data partition (`/mnt/bin`) — on-demand:** all 50 `/bin/*.wasm` tools
  (`ls`, `cat`, `grep`, `rtop`, `mkdisk`/`mkboot`/`install`, …).

## Background (verified)

- `limine.conf` declares 58 module entries (`/init.wasm`, `/root/{server,client}.wasm`,
  `/etc/init.sh`, `/bin/shell.wasm`, 50 `/bin/*.wasm`, + 3 `/payload/*`).
  `modules::mount_all()` copies each (except `/payload/*`) into tmpfs at its
  cmdline path. The first userspace process is `/bin/shell.wasm` (argv), which
  replays `/etc/init.sh`.
- `disk::copy_boot_payload(esp)` (M2b-1) writes BOOTX64.EFI + kernel +
  limine.conf + EVERY non-payload module to the **ESP** via `FatWriter`.
  `disk::author` returns `Layout{esp, data}` (both `Extent`s). `install`/`mkboot`
  do `author` → `PartBorrow(esp)` → `copy_boot_payload`.
- `FatWriter::write_file` (LFN, O(1) alloc) writes files to any FAT partition.
  The data partition is FAT32-formatted by `author` (empty).
- The shell `resolve_path(cmd)` (user/shell/src/main.rs:475) returns
  `/bin/{cmd}.wasm` — a SINGLE path. `exec` reads the wasm via the VFS and
  instantiates it (works on any VFS path, incl. `/mnt/...` → the FAT driver).

## Architecture — three units

### 1. Slim SSD `limine.conf` — `limine-ssd.conf` (NEW) + Makefile + payload module

A second config declaring only the bootstrap: the kernel `path` + module entries
for `/init.wasm`, `/etc/init.sh`, `/bin/shell.wasm`, `/root/server.wasm`,
`/root/client.wasm` (NO `/bin/*` tools). Ship it as a payload module
(`/payload/limine-ssd.conf`). The live `limine.conf` is unchanged.

### 2. Split the copy — `kernel/src/disk.rs` (`copy_boot_payload`)

Rewrite to write the **bootstrap to the ESP** and the **tools to the data
partition**:
```
const BOOTSTRAP: &[&str] = &["/init.wasm", "/etc/init.sh", "/bin/shell.wasm",
                             "/root/server.wasm", "/root/client.wasm"];
pub fn copy_boot_payload(dev, layout) -> Result<(), DiskError> {
    // ESP: BOOTX64.EFI + kernel + slim limine.conf + bootstrap modules
    { let mut esp = PartBorrow::new(dev, layout.esp.first_lba, layout.esp.sectors);
      let mut w = FatWriter::open(&mut esp)?;
      w.write_file("/EFI/BOOT/BOOTX64.EFI", payload("BOOTX64.EFI"))?;
      w.write_file("/boot/kernel",          payload("kernel"))?;
      w.write_file("/boot/limine/limine.conf", payload("limine-ssd.conf"))?; // the SLIM one
      for (cmdline, data) in modules::all() {
          if BOOTSTRAP.contains(&cmdline) { w.write_file(cmdline, data)?; }
      }
    } // esp borrow dropped
    // DATA: the /bin/*.wasm tools at /bin (mounts at /mnt/bin)
    { let mut d = PartBorrow::new(dev, layout.data.first_lba, layout.data.sectors);
      let mut w = FatWriter::open(&mut d)?;
      for (cmdline, data) in modules::all() {
          if cmdline.starts_with("/payload/") || BOOTSTRAP.contains(&cmdline) { continue; }
          w.write_file(cmdline, data)?;   // e.g. /bin/ls.wasm → data:/bin/ls.wasm → /mnt/bin/ls.wasm
      }
    }
    Ok(())
}
```
Signature changes from `(esp)` to `(dev, layout)` (it needs both partitions). The
two `PartBorrow`s are sequential (dropped between) — no aliasing. `install` +
`mkboot` updated to call `copy_boot_payload(&mut port, &layout)` after `author`.
(`modules::payload`/`all` already exist; add `payload("limine-ssd.conf")` via the
new module.)

### 3. Shell PATH `/bin` → `/mnt/bin` fallback — `user/shell/src/main.rs`

`resolve_path(cmd)`: return `/bin/{cmd}.wasm` if it exists, else
`/mnt/bin/{cmd}.wasm` if it exists, else None. Use `std::fs::metadata(path).is_ok()`
to test existence (WASI). Tab-completion (the `/bin` listing at main.rs:121-134)
should also union `/bin` + `/mnt/bin` (best-effort; `/mnt/bin` may be absent on
the live system → ignore the error).
- **Live ISO/USB:** `/bin/ls.wasm` exists (tmpfs) → first hit. `/mnt/bin` absent
  → second lookup harmlessly fails. Unchanged behavior.
- **Installed SSD:** `/bin/` has only `shell` (bootstrap) → `/bin/ls.wasm` misses
  → `/mnt/bin/ls.wasm` hits → `exec` reads it from the FAT on-demand.

## Data flow (installed SSD boot)

```
UEFI → BOOTX64.EFI → slim limine.conf → load kernel + 5 bootstrap modules (RAM)
kernel boot → storage::init mounts the data partition at /mnt (/mnt/bin = the tools)
shell (bootstrap) runs /etc/init.sh; network/SSH service (bootstrap) up
user runs `ls` → resolve /bin/ls.wasm (miss) → /mnt/bin/ls.wasm (hit) → read FAT → wasmi run
```

## Error handling / degradation

- **`/mnt` fails to mount on the SSD** → `/mnt/bin` absent → only the bootstrap
  tools (shell) work; `ls`/etc. → "command not found". **But the system boots**
  (shell + network/SSH up, M1 mount is non-fatal). Documented graceful
  degradation (vs. today's "all tools always resident").
- A tool present in BOOTSTRAP is on the ESP (always available); the rest depend
  on `/mnt`.
- Live ISO unaffected (full toolset, no `/mnt` dependency).

## Testing

- **m2b2 (boot-from-SSD)** gains the real proof: after the SSD boots (Phase 2),
  the init runs a tool that lives ONLY on the data partition (e.g. `uname -a` or
  `ls /mnt/bin`) and the test asserts its output — proving on-demand exec from
  `/mnt/bin` works. (If the tool ran, it was loaded from the FAT, not the ESP.)
- **m2b1 (mkboot copy)**: update the ESP checks — the ESP now has BOOTX64.EFI +
  kernel + (slim) limine.conf + the 5 bootstrap modules (NOT the 50 tools);
  assert a tool (`ls.wasm`) is on the **data** partition, not the ESP. Byte-identity
  still on the kernel.
- Regression: `run-test`, `run-gpt-test`, `run-m2a-test` green (live path
  unchanged — the live ISO still has all tools; the shell fallback is additive).

## Out of scope

- Changing the live ISO/USB (stays full initramfs — necessary, no USB-storage driver).
- A USB-mass-storage driver (would let the live medium also load on-demand) — a
  separate large driver milestone.
- A real PATH env var / arbitrary search dirs (just `/bin` then `/mnt/bin` for now).

## Files touched

- `limine-ssd.conf` — NEW (slim config). `Makefile` — copy it onto the ISO +
  declare it as a payload module. `limine.conf` — add the `/payload/limine-ssd.conf`
  module entry (the live config itself unchanged otherwise).
- `kernel/src/disk.rs` — `copy_boot_payload(dev, layout)` split (ESP bootstrap /
  data tools). `kernel/src/wasm/host/proc.rs` — `ruos_install`/`ruos_mkboot` call
  the new signature.
- `user/shell/src/main.rs` — `resolve_path` `/bin`→`/mnt/bin` fallback (+ completion union).
- `tests/m2b1-test.sh`, `tests/m2b2-test.sh`, `user-bin/m2b2-init.sh` — updated
  asserts. CHANGELOG 218.
