# On-demand tools (slim SSD initramfs) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Installed SSD = slim ESP (kernel + shell + init + network/SSH service) +
the ~50 command tools on the data partition (`/mnt/bin`), loaded on-demand by the
shell. Live ISO/USB unchanged. Design:
`docs/superpowers/specs/2026-06-03-ondemand-tools-design.md`.

**Tech Stack:** Rust no_std (kernel) + wasm32-wasip1 (shell), Limine, M2a/M2b
`FatWriter`/`author`/`PartBorrow`/`modules`. Build via WSL. Branch
`feature/ondemand-tools`. Changelog 218. End commits with `Co-Authored-By: Claude
Opus 4.8 (1M context) <noreply@anthropic.com>`. Kill stray qemu via `ps -eo
pid,comm | awk '/qemu-system/{print $1}'` (NOT pgrep). mtools on the 9p `/mnt/e`
mount is slow → stage ESP/data images on ext4 `/tmp` for host-side mtools.

The bootstrap set (on the SSD ESP) = `["/init.wasm", "/etc/init.sh",
"/bin/shell.wasm", "/root/server.wasm", "/root/client.wasm"]`. Everything else
(`/bin/*.wasm`) → the data partition.

---

### Task 1: slim `limine-ssd.conf` + ship as payload module

**Files:** create `limine-ssd.conf`; modify `Makefile`, `limine.conf`.

- [ ] **Step 1: create `limine-ssd.conf`** — copy the structure of `limine.conf` but with ONLY the bootstrap modules. Read `limine.conf` first to copy the header/boot-entry syntax (timeout, the `/ruos` entry, `path: boot():/boot/kernel`, the comment style). Include exactly these module pairs (drop all `/bin/*.wasm` except shell, and all `/payload/*`):
```
    module_path: boot():/init.wasm
    module_cmdline: /init.wasm
    module_path: boot():/root/server.wasm
    module_cmdline: /root/server.wasm
    module_path: boot():/root/client.wasm
    module_cmdline: /root/client.wasm
    module_path: boot():/etc/init.sh
    module_cmdline: /etc/init.sh
    module_path: boot():/bin/shell.wasm
    module_cmdline: /bin/shell.wasm
```
(Match the exact `module_path: boot():<path>` form the live config uses — the paths must exist on the SSD ESP, where Task 2 writes shell/init/server/client.)

- [ ] **Step 2: Makefile** — copy `limine-ssd.conf` onto the ISO so it can be a module source, e.g. into `$(ISO_ROOT)/boot/limine/limine-ssd.conf` (next to where `limine.conf` is cp'd, ~Makefile:90). (It just needs to be a file Limine loads as a module on the LIVE ISO so the kernel can read its bytes — it's never used to boot the live ISO.)

- [ ] **Step 3: limine.conf** — add a payload module entry (next to the other `/payload/*`):
```
    module_path: boot():/boot/limine/limine-ssd.conf
    module_cmdline: /payload/limine-ssd.conf
```
(So `modules::payload("limine-ssd.conf")` returns its bytes at runtime; it's skipped from tmpfs by the existing `/payload/` filter.)

- [ ] **Step 4: build + verify** — `make iso` clean. `make run-test` → `TEST_PASS` (the extra module loads, skipped from tmpfs). Optionally extend the existing `binfo!("mod","payload: …")` log to include `limine-ssd.conf`'s size, and confirm it appears in the run-test serial (proves `payload("limine-ssd.conf")` resolves — Task 2 needs it).

- [ ] **Step 5: commit** — `git add limine-ssd.conf Makefile limine.conf && git commit -m "build(install): slim limine-ssd.conf (bootstrap-only) shipped as payload module ..."`

---

### Task 2: split `copy_boot_payload` (ESP bootstrap / data tools)

**Files:** `kernel/src/disk.rs`, `kernel/src/wasm/host/proc.rs`.

Read the current `copy_boot_payload(esp)` in disk.rs + how `ruos_install`/`ruos_mkboot` call it (proc.rs: `author` → `PartBorrow(layout.esp)` → `copy_boot_payload`).

- [ ] **Step 1: rewrite `copy_boot_payload` to `(dev, layout)`** in disk.rs:
```rust
const BOOTSTRAP: &[&str] = &["/init.wasm", "/etc/init.sh", "/bin/shell.wasm",
                             "/root/server.wasm", "/root/client.wasm"];

pub fn copy_boot_payload(dev: &mut dyn crate::blockdev::BlockDevice,
                         layout: &Layout) -> Result<(), DiskError> {
    // --- ESP: BOOTX64.EFI + kernel + slim limine.conf + bootstrap modules ---
    {
        let mut esp = crate::blockdev::PartBorrow::new(dev, layout.esp.first_lba, layout.esp.sectors);
        let mut w = crate::vfs::fat32::FatWriter::open(&mut esp).map_err(|_| DiskError::Io)?;
        w.write_file("/EFI/BOOT/BOOTX64.EFI", crate::modules::payload("BOOTX64.EFI").ok_or(DiskError::Io)?).map_err(|_| DiskError::Io)?;
        w.write_file("/boot/kernel", crate::modules::payload("kernel").ok_or(DiskError::Io)?).map_err(|_| DiskError::Io)?;
        w.write_file("/boot/limine/limine.conf", crate::modules::payload("limine-ssd.conf").ok_or(DiskError::Io)?).map_err(|_| DiskError::Io)?;
        for (cmdline, data) in crate::modules::all() {
            if BOOTSTRAP.contains(&cmdline) { w.write_file(cmdline, data).map_err(|_| DiskError::Io)?; }
        }
    } // esp borrow dropped here
    // --- DATA: the /bin/*.wasm tools (mounts at /mnt/bin) ---
    {
        let mut d = crate::blockdev::PartBorrow::new(dev, layout.data.first_lba, layout.data.sectors);
        let mut w = crate::vfs::fat32::FatWriter::open(&mut d).map_err(|_| DiskError::Io)?;
        for (cmdline, data) in crate::modules::all() {
            if cmdline.starts_with("/payload/") || BOOTSTRAP.contains(&cmdline) { continue; }
            w.write_file(cmdline, data).map_err(|_| DiskError::Io)?;  // /bin/ls.wasm → data:/bin/ls.wasm
        }
    }
    Ok(())
}
```
Note: writes the **slim** `limine-ssd.conf` as the SSD's `/boot/limine/limine.conf`. The data partition is already FAT32-formatted (empty) by `author`; `write_file` creates `/bin` on it on demand.

- [ ] **Step 2: update callers (proc.rs)** — `ruos_install` + `ruos_mkboot`: after `let layout = author(&mut port, esp)?;`, replace the `PartBorrow(esp)` + `copy_boot_payload(esp)` with a single `crate::disk::copy_boot_payload(&mut port, &layout)`. (No PartBorrow at the call site anymore — copy_boot_payload makes both internally.)

- [ ] **Step 3: build** — `cd kernel && cargo build --release` clean.

- [ ] **Step 4: host-verify the split** (de-risks T4). cp `disk.rs`+`fat32.rs`+`blockdev.rs`+`gpt.rs`+`crc32.rs` into a `/tmp` host crate over a `MemDev` (stub `modules::payload`/`all` to return a fake kernel + bootstrap set + ~10 fake `/bin/*.wasm` tools + a fake limine-ssd.conf). Run `author(&mut mem, 64)` + `copy_boot_payload(&mut mem, &layout)`. Dump → `/tmp/disk.img`, extract BOTH partitions (sgdisk -i 1 / -i 2 → dd, onto ext4 `/tmp`), assert with mtools:
  - **ESP**: has `/EFI/BOOT/BOOTX64.EFI`, `/boot/kernel`, `/boot/limine/limine.conf` (the slim one), `/bin/shell.wasm`, `/root/server.wasm`, `/init.wasm` — and does NOT have a non-bootstrap tool like `/bin/ls.wasm`.
  - **DATA**: has `/bin/ls.wasm` (and the other fake tools), NOT the bootstrap ones.
  - `fsck.fat -n` clean on both.
  Clean up /tmp.

- [ ] **Step 5: commit** — `git add kernel/src/disk.rs kernel/src/wasm/host/proc.rs && git commit -m "feat(install): split copy — bootstrap to ESP, tools to the data partition ..."`

---

### Task 3: shell `/bin` → `/mnt/bin` fallback

**Files:** `user/shell/src/main.rs`.

- [ ] **Step 1: `resolve_path` fallback** (around main.rs:475). Currently returns `/bin/{cmd}.wasm`. Change to: build `/bin/{cmd}.wasm`; if `std::fs::metadata(&p).is_ok()` return it; else build `/mnt/bin/{cmd}.wasm`; if it exists return it; else `None`. (Keep any existing builtins/`.wasm`-suffix handling.) Match the fn's current return type (`Option<String>`).

- [ ] **Step 2: completion union (optional, best-effort)** — the tab-completion `/bin` listing (main.rs:121-134) may also read `/mnt/bin` and union the names; ignore a read error on `/mnt/bin` (absent on the live system). Don't break completion if `/mnt/bin` is missing.

- [ ] **Step 3: build** — the shell tool builds (`make iso` rebuilds `shell.wasm`). Clean.

- [ ] **Step 4: regression** — `make run-test` → `TEST_PASS`. **Critical:** the live ISO has all tools in `/bin` (tmpfs), so `resolve_path` hits `/bin` first → unchanged behavior; the smoke battery (ls, cat, grep, pipes, rtop, …) must all still run. `/mnt/bin` is absent on the run-test disk → the fallback lookup harmlessly misses. Confirm no tool broke.

- [ ] **Step 5: commit** — `git add user/shell/src/main.rs user-bin/shell.wasm && git commit -m "feat(shell): resolve commands from /bin then /mnt/bin (on-demand tools) ..."` (commit the rebuilt `user-bin/shell.wasm` blob per repo convention).

---

### Task 4: tests (slim ESP + on-demand exec) + changelog

**Files:** `tests/m2b1-test.sh`, `tests/m2b2-test.sh`, `user-bin/m2b2-init.sh`, `Makefile` (no new target), `CHANGELOG/218-26-06-03-ondemand-tools.md`.

- [ ] **Step 1: m2b1-test.sh** — mkboot now writes a slim ESP + tools on the data partition. Update the host checks (stage images on ext4 `/tmp`):
  - ESP (`sgdisk -i 1` → dd): mtools sees `/EFI/BOOT/BOOTX64.EFI`, `/boot/kernel`, `/boot/limine/limine.conf`, `/bin/shell.wasm`, `/root/server.wasm` — and assert a tool like `/bin/ls.wasm` is NOT on the ESP.
  - DATA (`sgdisk -i 2` → dd): mtools sees `/bin/ls.wasm` + `/bin/readdirtest.wasm` (LFN proof now on the data partition). Byte-identity `cmp` still on `/boot/kernel` (ESP) + on `/bin/ls.wasm` (now from the DATA partition vs `user-bin/ls.wasm`).
  - `fsck.fat -n` clean on both partitions.
  Token stays `TEST_PASS_M2B1`.

- [ ] **Step 2: m2b2 — prove on-demand exec.** `user-bin/m2b2-init.sh`: after `install 0`, add a line that runs a tool living ONLY on the data partition, e.g.:
```sh
echo ruos boot OK
install 0
uname -a
echo m2b2-installed
```
On Phase 1 (ISO/live), `uname` resolves from `/bin` (tmpfs) → prints. On **Phase 2 (booted from SSD)**, `/bin/uname.wasm` is absent (slim ESP) → resolves from `/mnt/bin/uname.wasm` → **loaded on-demand from the FAT** → prints. In `tests/m2b2-test.sh` Phase 2, ADD an assert that `uname`'s output (a stable substring it prints — check what `uname -a` emits, e.g. `ruos` or the version string) appears in the phase-2 serial AFTER `ruos boot OK`. That line proves on-demand exec from `/mnt/bin`. Keep the existing `ruos boot OK` + `mnt mounted FAT` asserts. (Phase 2 already waits for `m2b2-installed` — the last marker — so `uname`'s output is captured.)

- [ ] **Step 3: run ALL gates** (sequential, kill stray qemu between, stage mtools on /tmp): `make run-m2b2-test`→`TEST_PASS_M2B2` (incl. the on-demand `uname` from /mnt/bin); `run-m2b1-test`→`TEST_PASS_M2B1` (slim ESP + tools on data); `run-m2a-test`→`TEST_PASS_M2A`; `run-gpt-test`→`TEST_PASS_GPT`; `run-test`→`TEST_PASS`.
  If Phase 2 `uname` prints nothing: the shell fallback or the data-partition tools are wrong — check the phase-2 serial (does `shell:` say command not found?) + that `/mnt/bin/uname.wasm` exists (it's a kernel/copy bug → report). If a tool fails on the LIVE path (run-test): the `resolve_path` change broke `/bin` resolution — fix.

- [ ] **Step 4: CHANGELOG** `CHANGELOG/218-26-06-03-ondemand-tools.md` (Cosa: SSD slim — ESP solo bootstrap [kernel+shell+init+rete/SSH], i ~50 tool sulla partizione dati caricati on-demand dalla shell [/bin→/mnt/bin]; live ISO invariato; Perché: meno RAM/spazio al boot installato, tool fuori da limine.conf; File toccati; Verifica: i 5 gate, + boot-da-SSD esegue `uname` da /mnt/bin on-demand).

- [ ] **Step 5: commit** — `git add tests/m2b1-test.sh tests/m2b2-test.sh user-bin/m2b2-init.sh CHANGELOG/218-26-06-03-ondemand-tools.md && git commit -m "test(install): slim-ESP + on-demand tool exec from /mnt/bin (boot-from-SSD runs uname off the data partition) + changelog ..."`

---

## Self-review (controller)

- `copy_boot_payload(dev, layout)` (T2) is called by install/mkboot (T2 Step 2); `payload("limine-ssd.conf")` needs T1's module. `resolve_path` (T3) makes the SSD's `/mnt/bin` tools runnable; T4 proves it via `uname` on the SSD boot.
- Live ISO unchanged: T1 only ADDS a module; T3's fallback is additive (`/bin` first); T2/T4 only affect the SSD `install`/`mkboot` path. run-test (live) must stay green throughout.
- The on-demand proof (T4 Step 2) is the crux: a tool that exists ONLY on `/mnt/bin` running on the SSD boot = on-demand exec works.
- Degradation: if `/mnt` doesn't mount on the SSD, only bootstrap tools work — documented, non-fatal.
