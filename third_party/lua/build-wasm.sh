#!/usr/bin/env bash
# Build the standalone Lua interpreter as a wasm32-wasi module for ruos.
#
# Output: user-bin/lua.wasm  — a VENDORED PREBUILT artifact. Lua is C, not a
# cargo crate, so it lives outside the `user/*` → `user-bin/*.wasm` pipeline.
# Build it ONCE here, then commit the result (`git add -f user-bin/lua.wasm`);
# `make iso` packs it into /bin like any other tool but never rebuilds it.
#
# Run inside WSL (Ubuntu). Needs: wget, tar, make, sudo. Downloads wasi-sdk +
# the Lua source tarball on first run. Idempotent.
set -euo pipefail

LUA_VER=${LUA_VER:-5.4.7}
WASI_SDK_VER=${WASI_SDK_VER:-24}
WASI_SDK_DIR=${WASI_SDK_DIR:-/opt/wasi-sdk}

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
OUT="$REPO/user-bin/lua.wasm"

# 1. wasi-sdk (clang preconfigured for wasm32-wasi) ---------------------------
if [ ! -x "$WASI_SDK_DIR/bin/clang" ]; then
  echo ">> wasi-sdk not found at $WASI_SDK_DIR — downloading v$WASI_SDK_VER"
  tmp="$(mktemp -d)"
  tarball="wasi-sdk-${WASI_SDK_VER}.0-x86_64-linux.tar.gz"
  wget -q "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VER}/${tarball}" \
       -O "$tmp/$tarball"
  tar xzf "$tmp/$tarball" -C "$tmp"
  sudo mv "$tmp/wasi-sdk-${WASI_SDK_VER}.0-x86_64-linux" "$WASI_SDK_DIR"
  rm -rf "$tmp"
fi

# 2. Lua source --------------------------------------------------------------
SRC="$HERE/lua-$LUA_VER"
if [ ! -d "$SRC" ]; then
  echo ">> fetching Lua $LUA_VER"
  wget -q "https://www.lua.org/ftp/lua-$LUA_VER.tar.gz" -O "$HERE/lua-$LUA_VER.tar.gz"
  tar xzf "$HERE/lua-$LUA_VER.tar.gz" -C "$HERE"
fi

# 2b. patch loslib.c for WASI ------------------------------------------------
# WASI's libc has no tmpnam() and no system(). Stub the two `os` entry points
# that need them: os.tmpname (always reports failure) and os.execute (no
# subprocess in the sandbox). Substring-keyed seds → no-ops once applied.
sed -i \
  -e 's/\bL_tmpnam\b/32/' \
  -e 's/e = (tmpnam(b) == NULL);/(void)b; e = 1;/' \
  -e 's@system(cmd)  /\* default definition \*/@((cmd) == NULL ? 0 : -1)@' \
  "$SRC/src/loslib.c"
# WASI also lacks tmpfile(): make io.tmpfile fail cleanly (returns nil + error).
sed -i 's/p->f = tmpfile();/p->f = NULL;  \/* WASI: no tmpfile *\//' "$SRC/src/liolib.c"

# 3. build standalone `lua` → wasm -------------------------------------------
# LUA_USE_C89   : pure ANSI C — no POSIX popen/system/readline (no shell in the
#                 sandbox), no dlopen. os.exit maps to WASI proc_exit.
# _WASI_EMULATED_SIGNAL + -lwasi-emulated-signal : lua.c calls signal() for the
#                 ^C interrupt handler; wasi-libc emulates it.
# -mllvm -wasm-enable-sjlj : Lua's error handling (luaD_throw) uses setjmp/longjmp;
#                 wasi-sysroot's setjmp.h hard-errors without this. NOTE: it lowers
#                 to the wasm exception-handling proposal — the RUNTIME must support
#                 EH. Wasmtime does; wasmi may trap. Verify in-OS.
WASI="$WASI_SDK_DIR"
SJLJ="-mllvm -wasm-enable-sjlj"
make -C "$SRC/src" clean || true
make -C "$SRC/src" generic \
  CC="$WASI/bin/clang --sysroot=$WASI/share/wasi-sysroot" \
  AR="$WASI/bin/llvm-ar rc" \
  RANLIB="$WASI/bin/llvm-ranlib" \
  MYCFLAGS="-D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS -DLUA_USE_C89 -O2 $SJLJ" \
  MYLDFLAGS="-lwasi-emulated-signal -lwasi-emulated-process-clocks $SJLJ"

# 4. install -----------------------------------------------------------------
cp "$SRC/src/lua" "$OUT"
echo
file "$OUT" || true
echo ">> wrote $OUT"
echo ">> commit it:  git add -f user-bin/lua.wasm"
