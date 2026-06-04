---
name: ruos-wasm-app
description: Use when creating a new ruos userspace tool/app (a wasm32-wasip1 binary under user/). Scaffolds the crate + wires the workspace, Makefile BIN_TOOLS, and limine.conf, and documents the available `ruos::*` host functions — so you don't re-read the codebase each time.
---

# Scaffold a ruos userspace wasm app

ruos userspace = `wasm32-wasip1` modules. Each tool is a tiny crate under `user/`.
The kernel is the host: plain `std` (files, args, stdout, process) works via WASI
preview 1; kernel-specific features (disk, net, proc, smp, term) are `ruos::*`
host-function imports. This skill scaffolds a new tool end-to-end.

**Announce:** "Using ruos-wasm-app to scaffold the `<name>` tool."

## One command to scaffold

From the repo root, run the bundled script (it creates the crate from a template
and wires all three integration points, idempotently):

```bash
bash ${CLAUDE_PLUGIN_ROOT}/skills/ruos-wasm-app/scaffold.sh <name> "one-line description"
```

It does:
1. `user/<name>/Cargo.toml` + `user/<name>/src/main.rs` (a working template).
2. Adds `"<name>"` to the `user/Cargo.toml` workspace `members`.
3. Prepends `<name>` to `BIN_TOOLS` in the root `Makefile` (so it builds + ships to `/bin`).
4. Inserts the `/bin/<name>.wasm` module pair into `limine.conf` (so Limine loads it).

`<name>` must be `[a-z0-9_]` and match the shell command you'll type in ruos.

## Then

1. **Edit `user/<name>/src/main.rs`** — implement the tool. The template is a
   plain-`std` skeleton. Use `std::env::args()`, `println!`/`eprintln!`,
   `std::fs`, `std::process::exit(code)` freely. For a kernel capability, import a
   `ruos::*` host fn — see **`reference.md`** (in this skill dir) for the full list,
   signatures, and the buffer-passing convention.
2. **Build:** `make iso` (from the repo root, via WSL) — compiles every tool and
   assembles the ISO. The new binary lands at `user-bin/<name>.wasm` and on the
   ISO at `/bin/<name>.wasm`. (Single-tool compile check: `cd user && cargo build
   --target wasm32-wasip1 --release -p <name>`.)
3. **Run it:** boot ruos (`make run` interactive, or `make run-test`) and type
   `<name>` at the shell. The shell resolves `/bin/<name>.wasm` (or `/mnt/bin` on
   an installed SSD) and execs it.
4. **Commit the blob:** `user-bin/<name>.wasm` is a tracked artifact (repo
   convention — every tool's built `.wasm` is committed). `git add user/<name>
   user-bin/<name>.wasm user/Cargo.toml Makefile limine.conf` + the lock if it changed.

## What a tool looks like

The simplest (`user/echo`):
```rust
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("{}", args.join(" "));
}
```

Using a kernel host fn (`user/disks` — note the import + `unsafe` call):
```rust
#[link(wasm_import_module = "ruos")]
extern "C" { fn sata_list(ptr: i32, cap: i32) -> i32; }

fn main() {
    let mut buf = [0u8; 1024];
    let n = unsafe { sata_list(buf.as_mut_ptr() as i32, buf.len() as i32) };
    // ... parse the kernel-formatted bytes ...
}
```

## Host functions (`ruos::*`) — quick map

Import as `#[link(wasm_import_module = "ruos")] extern "C" { fn <name>(...) -> ...; }`
and call inside `unsafe`. The kernel registers these (grep `\.func_wrap("ruos"` in
`kernel/src/wasm/`). Common ones — **full signatures + the kernel↔guest buffer
convention are in `reference.md`**:

- **Process/exec:** `exec`, `exec_pipeline`, `proc_list`, `proc_stat`, `proc_kill`, `poll_stdin`
- **System info:** `uname`, `uptime`, `meminfo`, `cpuinfo`, `cpustat`, `time_get`, `dmesg`
- **Disk/FS:** `sata_list`, `mkdisk`, `mkboot`, `install`, `umount`, `readdir`
- **Network:** `net_iface`, `net_set_static`, `net_dhcp_renew`, `tcp_dial`, `ping`, `pci_list`
- **Terminal:** `tcgetattr`, `tcsetattr`, `isatty`
- **Services / SMP / power:** `service_list/start/status`, `smp_bench`, `poweroff`, `reboot`

**Most tools need NONE of these** — file I/O, args, env, and stdout are plain WASI
`std`. Reach for a host fn only for a kernel capability `std` can't express.

## Gotchas

- **Target:** the build always passes `--target wasm32-wasip1` (no `.cargo/config`
  needed). Ensure it's installed once: `rustup target add wasm32-wasip1`.
- **The model/string fields** returned by some host fns (e.g. `sata_list`) are
  `{:?}`-quoted debug strings — `trim_matches('"')` when parsing.
- **Build via WSL** (`wsl -d Ubuntu -u root -e bash -lc '... make iso'`), login
  shell so cargo is on PATH.
- **Adding a NEW host fn** (the tool needs a kernel capability that doesn't exist
  yet): that's a kernel change in `kernel/src/wasm/host/` (add the fn + `.func_wrap`
  registration) — see `reference.md`'s "Adding a host fn" section. The scaffold
  script only handles the userspace tool side.
- **`profile.release`** (in `user/Cargo.toml`) already sets `opt-level="s"`, `lto`,
  `panic="abort"`, `strip` for all tools — don't re-add per-crate.

## Files this skill touches (per new tool)

`user/<name>/Cargo.toml` (new), `user/<name>/src/main.rs` (new), `user/Cargo.toml`
(members), `Makefile` (BIN_TOOLS), `limine.conf` (module entry), and after build
`user-bin/<name>.wasm` (committed blob).
