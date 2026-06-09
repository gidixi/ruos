# netconsole-rx

Host-side receiver for **ruos netconsole** — a zero-dependency, cross-platform
(Windows/Linux/macOS) drop-in for `nc -ul 6666`.

ruos built with `--features netconsole` broadcasts every kernel log line (INFO+)
as a UDP datagram to `255.255.255.255:6666`. Run this on any PC on the same LAN
to see the live stream plus the boot backlog (flushed once DHCP binds).

## Build

Pure `std`, no dependencies → builds for Windows, Linux and macOS unchanged.

**Native (on the target OS, with Rust installed):**

```sh
cargo build --release
# Linux/macOS: target/release/netconsole-rx
# Windows:     target\release\netconsole-rx.exe
```

**Cross-compile a Windows `.exe` from WSL/Linux** (no Windows Rust needed):

```sh
rustup target add x86_64-pc-windows-gnu
sudo apt-get install -y mingw-w64          # provides the x86_64-w64-mingw32 linker
cargo build --release --target x86_64-pc-windows-gnu
# -> target/x86_64-pc-windows-gnu/release/netconsole-rx.exe  (PE32+ console exe)
```

## Use

```sh
netconsole-rx               # listen on 0.0.0.0:6666
netconsole-rx -p 7000       # custom port
netconsole-rx --bind 0.0.0.0
netconsole-rx --src         # prefix each line with the sender's IP
netconsole-rx -h            # help
```

### Interactive commands

While running, type a command + Enter on stdin:

| Command       | Action                                        |
| ------------- | --------------------------------------------- |
| `c` / `clear` | clear the terminal screen (ANSI)              |
| `l` / `clog`  | clear the log file (truncate `netconsole.log`)|
| `h` / `help`  | show the command list                         |
| `q` / `quit`  | exit                                          |

(Commands need stdin, so they're inert when input is piped/redirected — the
receive loop keeps running regardless.)

Logs go to **stdout** (pipe/redirect freely); the listening banner and errors go
to **stderr**:

```sh
netconsole-rx | tee ruos-boot.log
```

A copy of everything is also written to **`netconsole.log`** in the same folder as
the executable. That file is **cleared (truncated) on every start**, so it always
contains only the current session.

## Notes

- **Windows firewall:** the first run may trigger a Windows Defender Firewall
  prompt for inbound UDP — allow it on the private/LAN profile, otherwise the
  broadcast datagrams are dropped before reaching the process.
- Build ruos with netconsole: `make iso CARGO_FEATURES=netconsole`, or
  `.\build-iso.ps1 -Netconsole`.
- Equivalent shell one-liners: `nc -ul 6666` (Linux), `ncat -ul 6666` (Windows
  with nmap), `socat -u UDP-RECV:6666 -`.
