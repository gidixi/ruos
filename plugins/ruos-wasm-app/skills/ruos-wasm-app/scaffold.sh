#!/usr/bin/env bash
# Scaffold a new ruos userspace wasm tool and wire it into the build.
#   usage: bash .claude/skills/ruos-wasm-app/scaffold.sh <name> ["one-line desc"]
# Creates user/<name>/{Cargo.toml,src/main.rs} and (idempotently) adds the tool to
# user/Cargo.toml members, the Makefile BIN_TOOLS, and limine.conf. Then edit
# user/<name>/src/main.rs and run `make iso`.
set -euo pipefail

NAME="${1:?usage: scaffold.sh <name> [\"desc\"]}"
DESC="${2:-a ruos userspace tool}"
case "$NAME" in
  *[!a-z0-9_]*) echo "error: name must be [a-z0-9_] (got '$NAME')"; exit 1 ;;
esac

# Find the ruos repo root by walking up from the CWD (so this works whether the
# script lives in the repo or in an installed plugin cache — run it from anywhere
# inside the ruos checkout).
ROOT="$PWD"
while [ "$ROOT" != "/" ] && [ ! -f "$ROOT/user/Cargo.toml" ]; do ROOT="$(dirname "$ROOT")"; done
[ -f "$ROOT/user/Cargo.toml" ] || { echo "error: not in the ruos repo (no user/Cargo.toml found from $PWD upward)"; exit 1; }
cd "$ROOT"
command -v perl >/dev/null || { echo "error: perl required for wiring"; exit 1; }
[ -e "user/$NAME" ] && { echo "error: user/$NAME already exists"; exit 1; }

# --- 1. the crate ---------------------------------------------------------
mkdir -p "user/$NAME/src"
cat > "user/$NAME/Cargo.toml" <<EOF
[package]
name = "$NAME"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "$NAME"
path = "src/main.rs"
EOF

cat > "user/$NAME/src/main.rs" <<EOF
//! $NAME -- $DESC
//!
//! Usage:
//!   $NAME [args...]
//!
//! Plain \`std\` works via WASI preview 1: std::env::args, println!/eprintln!,
//! std::fs (the VFS: /, /tmp, /mnt, ...), std::process::exit(code). For a kernel
//! capability (disk/net/proc/term/smp) import a ruos host fn — see the skill's
//! reference.md, e.g.:
//!   #[link(wasm_import_module = "ruos")] extern "C" { fn uname(ptr: i32, cap: i32) -> i32; }

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // TODO: implement $NAME.
    println!("$NAME: hello ({} args)", args.len());
}
EOF

# --- 2. workspace member (first standalone "]" closes user/Cargo.toml members) ---
if grep -q "\"$NAME\"" user/Cargo.toml; then
  echo "note: \"$NAME\" already in user/Cargo.toml members"
else
  perl -0pi -e "s/\n\]/\n    \"$NAME\",\n]/" user/Cargo.toml
fi

# --- 3. Makefile BIN_TOOLS (prepend right after ':=') ---------------------
if grep -qE "^BIN_TOOLS[[:space:]]*:=.*[[:space:]]$NAME([[:space:]]|\\\\|$)" Makefile \
   || grep -qE "^BIN_TOOLS[[:space:]]*:=[[:space:]]*$NAME([[:space:]]|\\\\)" Makefile; then
  echo "note: $NAME already in Makefile BIN_TOOLS"
else
  perl -pi -e "s/^(BIN_TOOLS\s*:=\s*)/\${1}$NAME /" Makefile
fi

# --- 4. limine.conf module entry (insert before the kernel payload module, i.e.
#        at the end of the /bin tool list — the /payload/* modules come last) ---
if grep -q "/bin/$NAME.wasm" limine.conf; then
  echo "note: /bin/$NAME.wasm already in limine.conf"
else
  before="$(grep -c "/bin/$NAME.wasm" limine.conf || true)"
  perl -0pi -e "s{(\n    module_path: boot\(\):/boot/kernel\n)}{\n    module_path: boot():/bin/$NAME.wasm\n    module_cmdline: /bin/$NAME.wasm\$1}" limine.conf
  if ! grep -q "/bin/$NAME.wasm" limine.conf; then
    echo "warn: could not auto-insert into limine.conf — add these 2 lines inside the /ruos entry:"
    echo "    module_path: boot():/bin/$NAME.wasm"
    echo "    module_cmdline: /bin/$NAME.wasm"
  fi
fi

echo
echo "Scaffolded user/$NAME and wired user/Cargo.toml, Makefile BIN_TOOLS, limine.conf."
echo "Next:"
echo "  1. edit user/$NAME/src/main.rs   (host fns: see .claude/skills/ruos-wasm-app/reference.md)"
echo "  2. make iso                      (builds user-bin/$NAME.wasm + the ISO)"
echo "  3. boot ruos, type '$NAME' at the shell"
echo "  4. git add user/$NAME user-bin/$NAME.wasm user/Cargo.toml Makefile limine.conf"
