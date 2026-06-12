#!/usr/bin/env bash
# Build hello-pthread.c as a wasm32-wasip1-threads module for ruos.
#
# Output: tools/hello-pthread/hello-pthread.wasm — VENDORED PREBUILT artifact
# (committed, like user-bin/lua.wasm): C is not part of the cargo pipeline and
# wasi-sdk is a ~100 MB toolchain we don't want as a `make iso` dependency.
# Rebuild ONLY when the .c changes, then commit the result.
#
# Run inside WSL (Ubuntu). Downloads wasi-sdk on first run (same recipe as
# third_party/lua/build-wasm.sh). Idempotent.
set -euo pipefail

WASI_SDK_VER=${WASI_SDK_VER:-24}
WASI_SDK_DIR=${WASI_SDK_DIR:-/opt/wasi-sdk}

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ ! -x "$WASI_SDK_DIR/bin/clang" ]; then
  echo ">> wasi-sdk not found at $WASI_SDK_DIR — downloading v$WASI_SDK_VER"
  tmp="$(mktemp -d)"
  tarball="wasi-sdk-${WASI_SDK_VER}.0-x86_64-linux.tar.gz"
  wget -q "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VER}/${tarball}" \
       -O "$tmp/$tarball"
  tar xzf "$tmp/$tarball" -C "$tmp"
  mv "$tmp/wasi-sdk-${WASI_SDK_VER}.0-x86_64-linux" "$WASI_SDK_DIR"
  rm -rf "$tmp"
fi

# NB: a differenza del target Rust wasm32-wasip1-threads, wasi-sdk NON passa
# --import-memory di default: senza, il modulo DEFINISCE la sua memoria shared
# e ogni Instance (= ogni thread) ne avrebbe una propria — l'ABI wasi-threads
# richiede memoria IMPORTATA (env::memory), che è anche ciò su cui il router
# .cwasm di ruos riconosce un modulo threaded. max-memory obbligatorio con
# --shared-memory (qui 64 MiB).
"$WASI_SDK_DIR/bin/clang" \
  --target=wasm32-wasip1-threads -pthread -O2 \
  -Wl,--import-memory,--export-memory,--max-memory=67108864 \
  -o "$HERE/hello-pthread.wasm" "$HERE/hello-pthread.c"

ls -la "$HERE/hello-pthread.wasm"
echo "OK — commit it: git add -f tools/hello-pthread/hello-pthread.wasm"
