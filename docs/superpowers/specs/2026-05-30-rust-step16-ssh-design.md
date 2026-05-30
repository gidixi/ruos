# Step 16 — SSH server: Design Spec + Implementation Plan

**Date:** 2026-05-30
**Milestone:** Roadmap Step 16. End state: a remote machine running OpenSSH can
`ssh root@<ruos-ip>` and either run a one-shot wasm program (phase 1) or land
in a fully interactive `shell.wasm` session over a PTY (phase 2). Public-key
auth only, Ed25519 host key persisted to `/mnt/etc/ssh/host_key` (FAT, Step 15).

**Status:** Spec + plan combined, ready for execution.

> **For agentic workers:** the "Implementation Plan" half uses checkbox
> (`- [ ]`) task tracking. Use superpowers:subagent-driven-development or
> superpowers:executing-plans to execute task-by-task.

---

## Context

After Step 14 we have:
- `smoltcp` TCP sockets (`net::sockets::POOL`), kernel-side `connect`,
  `recv`, `send`, `accept` async helpers driven by `net_poll_task`.
- ChaCha20 CSPRNG seeded from RDRAND (`crate::rng`).
- Ethernet over virtio + e1000 (Step 14 + NIC MVP). DHCP-bound IP.

After Step 15:
- FAT32 persistent storage at `/mnt`. Lets us store the host key + an
  `authorized_keys` file across reboots.

After Step 12:
- PTY pseudo-terminals (master/slave + line discipline + termios). Lets us
  re-use the existing `shell.wasm` interactive program for phase 2.

What's missing is the SSH protocol layer + a server task listening on port 22
that mediates between the wire side (smoltcp socket) and the kernel side
(exec queue / PTY allocator).

---

## Goals

- **Phase 1 — non-interactive exec.**
  `ssh -i key root@<ip> /bin/echo.wasm hello world` runs `echo.wasm` inside
  the existing wasm executor, pipes its stdout back over the SSH channel,
  exits with the program's status. No PTY allocation. Closes the channel
  on `proc_exit`.

- **Phase 2 — interactive PTY session.**
  `ssh -i key root@<ip>` (no command) allocates a PTY pair, spawns
  `shell.wasm` on the slave, bridges the master to the SSH channel
  bidirectionally. Resize via `pty-req` window_change. Termios cooked mode
  by default; `tcsetattr` from `shell.wasm` still works.

- **Public-key auth.** Only `ssh-ed25519` accepted. `authorized_keys` lives
  at `/mnt/etc/ssh/authorized_keys` (FAT). `password` and `none` rejected.

- **Persistent host key.** Generated at first boot if absent, written to
  `/mnt/etc/ssh/host_key`. Subsequent boots reuse the same key so the
  client doesn't see a host-key-changed warning.

- **One concurrent session.** Multi-session deferred (one fiber per
  connection wires up easily later — the kernel is single-CPU + cooperative).

## Non-goals (YAGNI)

- No password auth, no keyboard-interactive, no host-based auth.
- No SFTP / SCP subsystems.
- No X11 forwarding, no agent forwarding, no TCP forwarding.
- No multiplexing (`ControlMaster` from the client is opaque to us; we just
  refuse extra channels).
- No HostKeyAlgorithms negotiation beyond ssh-ed25519.
- No rekeying (smoltcp socket lives for the lifetime of the connection).
- No compression (we don't advertise `zlib`).
- No IPv6.

---

## Crate choice — `sunset` vs `russh`

| | `sunset` | `russh` |
|---|---|---|
| no_std | ✅ native | ❌ std-only (tokio) |
| alloc required | optional | required |
| Async runtime | embedded-friendly | tokio |
| Server impl | ✅ | ✅ |
| Pubkey ed25519 | ✅ | ✅ |
| Pty/exec subsystems | ✅ | ✅ |
| Cipher / kex defaults | ChaCha20-Poly1305 + curve25519 | broader |
| Footprint | small | larger |

**Pick: `sunset`.** Embedded-first, no_std + alloc fits ruos perfectly. The
kernel already has `embassy-executor` cooperative async; `sunset` integrates
via its `BlockingTransport` or async traits over our smoltcp socket helpers.

If `sunset` integration turns out impossibly awkward during the spike in
Task 4, fall back to porting just the chacha20poly1305 + ed25519 crypto +
hand-rolling a minimal SSH transport layer (~1500 LoC; expensive). Prefer
to make `sunset` work.

---

## Architecture

```
              SSH client (OpenSSH on host)
                       │ TCP/22
                       ▼
   ┌────────────────────────────────────────────────────┐
   │ kernel/src/net/sockets POOL (smoltcp)               │
   │   listen 22 → accept → per-connection SocketHandle  │
   └────────────────────────────┬───────────────────────┘
                                │ async recv/send
                                ▼
   ┌────────────────────────────────────────────────────┐
   │ kernel/src/ssh/server.rs                            │
   │   sunset::Server<SshHandler>                        │
   │   • host key load/save (via crate::vfs FAT mount)   │
   │   • authorized_keys lookup                          │
   │   • channel dispatch                                │
   └─────────┬─────────────────────────┬─────────────────┘
             │ phase 1                 │ phase 2
             ▼                         ▼
   ┌──────────────────────┐   ┌──────────────────────────┐
   │ exec channel         │   │ pty + shell channel      │
   │ - exec_queue.post    │   │ - pty::allocate (Step 12)│
   │ - read wasm stdout   │   │ - spawn shell.wasm on    │
   │   from a Vec buffer  │   │   pts/N (Step 11)        │
   │ - close on proc_exit │   │ - bridge master <-> ssh  │
   └──────────────────────┘   └──────────────────────────┘
```

### Boot flow

1. `phases::userland::init` (after `net::init`) calls
   `crate::ssh::server::spawn()`, which:
   - Loads or generates the host key (creating
     `/mnt/etc/ssh/host_key` on first boot).
   - Loads the `authorized_keys` file (one ed25519 pubkey per line).
   - Allocates a TCP socket bound to `*:22`, calls `listen`.
   - Spawns an embassy task: `accept` → `serve_one`.
2. `serve_one` runs the full SSH handshake + auth + channel loop until the
   client disconnects, then returns to the accept loop.

---

## Components

### 0. `kernel/src/ssh/mod.rs` — module skeleton + `Config`

```rust
pub mod server;
pub mod hostkey;
pub mod authkeys;
pub mod channel;

pub struct Config {
    pub port: u16,            // default 22
    pub host_key_path:   &'static str,  // "/mnt/etc/ssh/host_key"
    pub authkeys_path:   &'static str,  // "/mnt/etc/ssh/authorized_keys"
}

pub static CONFIG: Config = Config {
    port: 22,
    host_key_path: "/mnt/etc/ssh/host_key",
    authkeys_path: "/mnt/etc/ssh/authorized_keys",
};
```

### 1. `kernel/src/ssh/hostkey.rs` — load/save Ed25519 host key

- On boot, try `vfs::open(CONFIG.host_key_path, READ)`.
- If exists: read 32-byte raw seed → reconstruct `ed25519::SigningKey`.
- If not: derive 32 bytes from `crate::rng::random` → write the seed →
  log `ssh: host key generated`.
- Returns `SigningKey` to the server.

Storage format: **raw 32-byte seed** (not OpenSSH PEM). Trivial to load,
zero parsing surface. The matching public key (32 bytes) is regenerated
in memory each boot.

### 2. `kernel/src/ssh/authkeys.rs` — load `authorized_keys`

- One key per line, OpenSSH format:
  `ssh-ed25519 <base64-encoded-32-byte-key> [comment]`.
- Returns `Vec<[u8; 32]>` — just the raw pub key bytes, comments stripped.
- Lookup is linear scan (single-user, never many keys).

Base64 decoder lives inline (~50 lines), avoids a dep.

### 3. `kernel/src/ssh/server.rs` — accept loop + connection task

```rust
pub fn spawn() -> Result<(), SshError> {
    let hk = hostkey::load_or_generate()?;
    let keys = authkeys::load()?;
    let socket_idx = net::sockets::POOL.alloc_tcp();
    let handle = net::sockets::POOL.handle(socket_idx).unwrap();
    net::sockets::listen(handle, CONFIG.port)?;
    executor::spawn(serve_loop(handle, hk, keys));
    Ok(())
}

async fn serve_loop(...) {
    loop {
        sockets::accept(handle).await.ok();
        if let Err(e) = serve_one(handle, &hk, &keys).await {
            kprintln!("ssh: session error: {}", e);
        }
        // Re-bind a fresh socket for the next accept (smoltcp half-close).
        // … alloc + listen again
    }
}
```

### 4. `kernel/src/ssh/channel.rs` — channel-side glue

Two flavours:

- `ExecChannel { argv, exit_code }` — calls `exec_queue::post_and_wait` for
  the wasm program named by `argv[0]`. Reads its stdout via a per-channel
  buffer the wasm side writes into (existing `proc::stdout_tee` style — we
  expose a `Vec<u8>` ring through a new kernel mechanism). Sends the bytes
  to the SSH channel data window, then closes with the captured exit code.

  **Note:** the simplest path is to allocate a PTY for the exec channel too
  and treat all output as the slave-side write stream. The "real" SSH spec
  uses separate stdout/stderr extended-data channels, but OpenSSH is fine
  with all stdout merged onto the data stream when `pty-req` is absent.
  Adopt this. Phase 1 is *implemented identically to phase 2* internally;
  only the user-visible UX differs (no shell prompt, exit on `proc_exit`).

- `PtyChannel { pair_idx }` — allocates one of the four `pty::PAIRS`,
  spawns `shell.wasm` with `stdin=stdout=stderr` bound to `/dev/pts/N`.
  Bridges:
    - SSH `data` from client → write to pty master input.
    - pty master output → SSH `data` to client.

Both channels' "I/O bridge" is two halves of an async loop polled by the
existing executor.

### 5. `kernel/src/ssh/sunset_io.rs` — `sunset` ↔ smoltcp socket adapter

`sunset` exposes traits like `embedded_io::Read/Write` or an async
equivalent. We provide a wrapper around our `net::sockets::recv/send`
helpers that implements those traits so `sunset::Server::process()` can
drive transport without knowing it's smoltcp.

### 6. `kernel/Cargo.toml` deps

```toml
sunset            = { version = "x.y", default-features = false, features = ["alloc"] }
ed25519-dalek     = { version = "2",   default-features = false, features = ["alloc", "rand_core"] }
sha2              = { version = "0.10",default-features = false }
chacha20poly1305  = { version = "0.10",default-features = false, features = ["alloc"] }
```

(`sunset` itself depends on most of these; explicit pins help when we need
to share crypto with our existing code.)

### 7. Boot wiring

- `boot::phases::userland::init` ⇒ `ssh::server::spawn()` after `net::init`.
- Log: `ssh: listening on 0.0.0.0:22`.
- If `/mnt` is unavailable (no SATA disk): bwarn `ssh: no /mnt — disabled`
  and skip. SSH requires persistence for the host key.

---

## Error handling

`SshError { VfsIo, NoNetwork, BadAuthKey, SunsetTransport(u32), ... }` with
`Display`. Connection-level errors are logged + the connection dropped; the
accept loop continues. Server-spawn errors at boot are non-fatal: log and
continue without SSH (matches the "kernel still boots, ssh just absent"
policy from Step 15's `/mnt`).

---

## Testing strategy

`no_std` kernel — tests = QEMU + grep, augmented by a host-side `ssh` smoke.

- **Unit gate (run-test):** `ssh listening on 0.0.0.0:22` in serial confirms
  spawn + listen.
- **Integration:** the Makefile adds a `run-ssh-test` target that:
  1. Starts QEMU with `-netdev user,hostfwd=tcp:127.0.0.1:2222-:22`.
  2. Waits for the listening line.
  3. Generates a temporary ed25519 key, writes its pubkey to
     `/mnt/etc/ssh/authorized_keys` via `mtools` *before* boot.
  4. Runs `ssh -p 2222 -o StrictHostKeyChecking=no -i key root@127.0.0.1
     /bin/echo.wasm hello` from the host, asserts exit 0 and `hello\n`
     stdout.

Phase 2 PTY testing is manual (interactive) initially; a scripted
expect-style test belongs in a follow-up if needed.

---

# Implementation Plan

> All commands via WSL (per CLAUDE.md). Build `make build`; gate
> `make run-test`. Logging `binfo!("ssh", ...)` / `bwarn!`. **Branch:**
> `feature/step16-ssh` (already created). Commit per task; do NOT push.
> CHANGELOG: one `CHANGELOG/NN-26-05-30-slug.md` per task.

## Task 1 — Branch + spec + module skeleton

**Files:** Create this spec (this file); `kernel/src/ssh/{mod,hostkey,
authkeys,server,channel}.rs` placeholders; modify `kernel/src/main.rs`
(`mod ssh;`).

- [ ] **Step 1:** Branch + commit spec.
- [ ] **Step 2:** Create the 5 placeholder files with `SshError` enum + a
  `Config` struct + `pub fn spawn() -> Result<(), SshError>` returning
  `Err(SshError::NotImplemented)` for now.
- [ ] **Step 3 (test):** `make build` → `Finished` (warnings OK).
- [ ] **Step 4:** CHANGELOG + commit.

## Task 2 — Host key persistence (`hostkey.rs`)

**Files:** `kernel/src/ssh/hostkey.rs`; add `ed25519-dalek` to
`kernel/Cargo.toml`.

- [ ] **Step 1:** Implement `load_or_generate(path) -> SigningKey`:
  open + read 32 bytes; if fail, derive seed from `rng::fill_bytes` →
  write file via vfs.
- [ ] **Step 2 (test):** `make build`; `make run-test`, expect serial
  `ssh: host key generated` on the first boot.
- [ ] **Step 3:** CHANGELOG + commit.

## Task 3 — authorized_keys reader (`authkeys.rs`)

**Files:** `kernel/src/ssh/authkeys.rs`.

- [ ] **Step 1:** Inline base64 decoder; `load(path) -> Vec<[u8; 32]>`
  parsing OpenSSH `ssh-ed25519 <b64> [comment]` lines.
- [ ] **Step 2 (test):** stage a known file (`mcopy` in Makefile), assert
  `binfo!("ssh", "loaded {} key(s)", n)` shows the expected count.
- [ ] **Step 3:** CHANGELOG + commit.

## Task 4 — `sunset` integration spike

**Files:** Add `sunset` to Cargo.toml; `kernel/src/ssh/sunset_io.rs`.

- [ ] **Step 1:** Wrap `net::sockets::{recv,send}` in a struct that
  implements the smallest `sunset` transport trait. Stand up a `sunset::
  Server` with the host key, no channels yet.
- [ ] **Step 2 (test):** `make build`; manual `ssh -p 2222 root@127.0.0.1`
  should fail with "could not open channel" but the transport layer
  should at least complete KEX. Log the protocol phase reached.
- [ ] **Step 3:** CHANGELOG + commit.

## Task 5 — Accept loop + serve_one stub

**Files:** `kernel/src/ssh/server.rs`.

- [ ] **Step 1:** `spawn()` allocates the socket, listens on 22, spawns
  `serve_loop` via `embassy_executor`. Log `ssh: listening on 0.0.0.0:22`.
- [ ] **Step 2:** `serve_loop` accepts, hands off to a `serve_one(socket)`
  that just echoes raw bytes back (smoke before sunset wires in).
- [ ] **Step 3 (test):** Makefile `run-ssh-smoke` target: connect via
  `nc -p 2222 127.0.0.1` from host, send a line, expect echo.
- [ ] **Step 4:** CHANGELOG + commit.

## Task 6 — Pubkey auth + reject everything else

**Files:** `kernel/src/ssh/server.rs`.

- [ ] **Step 1:** Wire sunset's auth callback to look up the incoming
  pubkey in `authkeys::load`. Accept matches, reject otherwise (no
  password fallback).
- [ ] **Step 2 (test, manual):** `ssh -i good_key` succeeds (gets to
  "no shell" failure); `ssh -i other_key` fails at auth.
- [ ] **Step 3:** CHANGELOG + commit.

## Task 7 — Exec channel (phase 1)

**Files:** `kernel/src/ssh/channel.rs`.

- [ ] **Step 1:** On `exec` request, parse `argv`, post to `exec_queue`,
  capture stdout into a Vec via a new kernel mechanism (extend the WASIX
  state's stdout backing to also tee into a per-channel buffer). Send
  bytes over SSH data channel as they arrive.
- [ ] **Step 2 (test, integration):** Makefile `run-ssh-test` Boot QEMU
  with hostfwd, mcopy authorized_keys, host runs `ssh -p 2222 ...
  /bin/echo.wasm hi`. Assert stdout `hi`.
- [ ] **Step 3:** CHANGELOG + commit.

## Task 8 — PTY channel (phase 2)

**Files:** `kernel/src/ssh/channel.rs`; `kernel/src/pty/...` (no API
changes hopefully — re-use what Step 12 exposed).

- [ ] **Step 1:** On `shell` request (or `pty-req` + no `exec`),
  allocate a PTY pair, spawn `shell.wasm` on the slave (existing
  `wasm_task` pattern; use `exec_queue` with stdin/stdout/stderr =
  pts/N). Bridge SSH `data` ↔ pty master.
- [ ] **Step 2:** Window size from `pty-req` + `window-change` requests.
- [ ] **Step 3 (test, manual):** `ssh -p 2222 -i key root@127.0.0.1`
  drops into `ruos:/$`. Run `ls`, `cat`, exit.
- [ ] **Step 4:** CHANGELOG + commit.

## Task 9 — Docs + roadmap

**Files:** `docs/superpowers/roadmap-rust-os.md`, README.

- [ ] **Step 1:** Roadmap Step 16 → ✅ DONE. Link spec/plan.
- [ ] **Step 2:** README adds an "SSH" section: how to generate a host key,
  how to put your pubkey on the disk image with `mtools`, how to connect.
- [ ] **Step 3:** CHANGELOG + commit.

---

## Done criteria

- `make run-test` → TEST_PASS with serial containing `ssh listening on 0.0.0.0:22`.
- `make run-ssh-test` (new): host-side `ssh -p 2222 -i key root@127.0.0.1
  /bin/echo.wasm hello` exits 0, prints `hello`.
- `ssh -p 2222 -i key root@127.0.0.1` (no command, manual) lands in an
  interactive `ruos:/$` shell and supports basic commands.
- Wrong key → "Permission denied (publickey)" on the client; server logs
  the rejection.

## Notes for the implementer

- **Crypto integration with our CSPRNG.** `ed25519-dalek` wants a
  `rand_core::CryptoRng`. Wrap `crate::rng::random_bytes` as a tiny
  `RngCore` impl living next to `hostkey.rs`. Do NOT pull `getrandom`.
- **`sunset` API.** Pin to a specific commit/tag once available, since
  `sunset` is pre-1.0 and API changes between releases. Lock with
  `[patch.crates-io]` if needed.
- **Single-session.** The accept loop serves one connection at a time
  initially. Re-binding the listen socket after each session is the
  easy path because smoltcp's `accept` semantics differ from POSIX:
  the listen socket transitions to Established on accept and must be
  cycled. Use a fresh `alloc_tcp` + `listen` per accept.
- **No rekeying.** Sessions short-lived enough that we skip rekey
  triggers. If `sunset` insists, accept the rekey but reject extra
  channels.
- **Stdout teeing.** The current `wasm` runtime writes to PTY 0 (the
  framebuffer console). For SSH exec, we need a per-fiber stdout that
  routes to the SSH channel instead. Likely the cleanest path is to
  give the spawned fiber a PTY pair of its own, just like phase 2,
  and bridge the master to the SSH channel — exec then collapses
  into the phase-2 implementation with one extra `proc_exit` hook
  that closes the channel.
