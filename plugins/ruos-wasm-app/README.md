# ruos-wasm-app — Claude Code plugin

Scaffold and build userspace wasm apps/tools for **ruos** (the x86-64 `no_std`
Rust OS in this repo). ruos userspace = `wasm32-wasip1` crates under `user/`; the
kernel is the host. This plugin packages everything needed to add a new tool
without re-reading the codebase.

## What's inside

| Component | What |
|---|---|
| **Skill** `ruos-wasm-app` | The how-to: app structure, scaffolding steps, template, host-fn map, gotchas. Auto-invoked when you're making a ruos tool. |
| `skills/ruos-wasm-app/scaffold.sh` | One command: creates `user/<name>/` + wires `user/Cargo.toml`, `Makefile` BIN_TOOLS, `limine.conf`. |
| `skills/ruos-wasm-app/reference.md` | The ~35 `ruos::*` host functions (signatures + the kernel↔guest buffer convention), what WASI `std` covers, and how to add a new host fn. |
| **Command** `/ruos-app` | `/ruos-app <name> [what it does]` — scaffold (+ implement from a description). |
| **Agent** `ruos-app-builder` | Delegate "build me a `<X>` tool" — scaffolds, implements, compile-checks. |

## Install

From the ruos repo (this repo is also the plugin's marketplace):

```
/plugin marketplace add E:\MinimalOS\BasicOperatingSystem
/plugin install ruos-wasm-app
```

(Or add the marketplace by its git URL once pushed.)

## Use

- **Slash command:** `/ruos-app hexdump "print a file as hex"`
- **Agent:** ask Claude to "use the ruos-app-builder agent to make a `<tool>` that …"
- **Manual:** `bash plugins/ruos-wasm-app/skills/ruos-wasm-app/scaffold.sh <name> "desc"`,
  then edit `user/<name>/src/main.rs`, then `make iso`.

Run from inside the ruos checkout (the scaffolder finds the repo root by walking
up from the cwd). After building, type the tool name at the ruos shell; remember
`user-bin/<name>.wasm` is a tracked blob to commit.

## Requirements

- The ruos repo (with `user/Cargo.toml`, `Makefile`, `limine.conf`).
- Rust nightly + `wasm32-wasip1` target (`rustup target add wasm32-wasip1`).
- Builds run via WSL on Windows (`wsl … bash -lc 'make iso'`).
