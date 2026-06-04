---
description: Scaffold a new ruos userspace wasm tool (and optionally implement it from a description).
argument-hint: <name> [what the tool should do]
---

Create a ruos userspace wasm tool. Arguments: `$ARGUMENTS`.

Do NOT read the whole ruos codebase — the bundled skill + `reference.md` have what
you need. Steps:

1. **Parse** `$ARGUMENTS`: the first token is the tool `<name>` (must be
   `[a-z0-9_]`, = the shell command). The rest (if any) is a free-text
   description of what the tool should do. If no name was given, ask for one.

2. **Scaffold** — run the bundled script from inside the ruos checkout (it walks
   up from the cwd to find the repo root, creates `user/<name>/`, and wires
   `user/Cargo.toml` members + `Makefile` BIN_TOOLS + `limine.conf`):
   ```
   bash "${CLAUDE_PLUGIN_ROOT}/skills/ruos-wasm-app/scaffold.sh" <name> "<description>"
   ```
   (Build via WSL on Windows: `wsl -d Ubuntu -u root -e bash -lc 'cd <repo> && bash …'`.)

3. **Implement** — if a description was given, edit `user/<name>/src/main.rs` to do
   it. Plain `std` (args, `println!`/`eprintln!`, `std::fs`, `std::process::exit`)
   works via WASI; for a kernel capability import a `ruos::*` host fn — read
   `${CLAUDE_PLUGIN_ROOT}/skills/ruos-wasm-app/reference.md` for the list,
   signatures, and the kernel↔guest buffer convention. If no description, leave the
   template for the user.

4. **Compile-check**: `cd user && cargo build --target wasm32-wasip1 --release -p <name>` (via WSL).

5. **Report**: tell the user to run `make iso` and type `<name>` in the ruos shell,
   and that `user-bin/<name>.wasm` is a tracked blob to `git add` along with the
   scaffolded crate + the 3 wired files.

If the tool needs a kernel capability that no existing `ruos::*` host fn provides,
say so and point to `reference.md`'s "Adding a host fn" section (a kernel change in
`kernel/src/wasm/host/`) — don't silently stub it.
