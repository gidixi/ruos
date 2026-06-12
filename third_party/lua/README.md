# Lua interpreter (wasm32-wasi) for ruos

Standalone Lua, compiled to a `.wasm` module so ruos can **run `.lua` scripts
in-OS** via the `wasmi` interpreter. Two levels of interpretation: `wasmi`
(kernel) runs `lua.wasm`, and `lua.wasm` runs your script. Slow but zero kernel
changes — it's just another WASI tool in `/bin`.

```sh
$ nano /home/hello.lua      # edit in-OS (nano ships in /bin)
$ lua /home/hello.lua       # shell exec's /bin/lua.wasm with the script arg
hello world
```

## Build

Lua is third-party **C**, not a cargo crate, so it sits outside the
`user/*` → `user-bin/*.wasm` pipeline. Build it **once**, then commit the
output as a vendored prebuilt artifact:

```bash
# inside WSL (Ubuntu)
cd /mnt/w/Work/GitHub/ruos
third_party/lua/build-wasm.sh        # → user-bin/lua.wasm
git add -f user-bin/lua.wasm         # tracked despite the user-bin/*.wasm ignore
```

The script downloads `wasi-sdk` (to `/opt/wasi-sdk`, override `WASI_SDK_DIR`)
and the Lua source tarball on first run, then builds the `generic` (pure ANSI
C89) target. `make iso` packs `lua.wasm` into `bin.bgz` → `/bin` but never
rebuilds it.

## Build flags (why)

| Flag | Reason |
|------|--------|
| `LUA_USE_C89` | pure ANSI C: drops POSIX `popen`/`system`/`readline`/`dlopen` (none in the WASM sandbox). `os.exit` → WASI `proc_exit`. |
| `-D_WASI_EMULATED_SIGNAL` + `-lwasi-emulated-signal` | `lua.c` installs a `signal()` handler for the `^C` interrupt; wasi-libc emulates it. |

## Known limits / gotchas

- **No `os.execute` / `io.popen`** — no subprocess in the sandbox (C89 build
  already stubs them out).
- **WASI gaps**: if Lua calls a WASI Preview 1 function ruos hasn't implemented,
  it traps at runtime. Surface the missing call from the first boot log and add
  it kernel-side.
- **`setjmp/longjmp`**: Lua's error handling uses it. wasi-libc supports it; if
  a future Lua/toolchain combo regresses, add `-mllvm -wasm-enable-sjlj`.
- **Speed**: double interpretation. Fine for scripts, not for hot loops.
