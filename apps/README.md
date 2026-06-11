# apps/ — prebuilt app drop folder

Drop any precompiled **`.cwasm`** GUI app here and it is **automatically bundled
into the ISO `/bin`** by `make iso` (a generic glob hook in the Makefile). The
desktop compositor scans `/bin` for `manifest()` exports, so the app then appears
in the launcher — **no kernel or Makefile change per app**.

This decouples *where an app is built* from *the OS build*: build a `.cwasm` with
any SDK / on any machine, copy it here, run `make iso`. The bundled SDK at
`../demo-apps-sdk/` does this via its `deploy.ps1`.

```
apps/
  README.md      (tracked)
  .gitkeep       (tracked)
  *.cwasm        (gitignored — your built apps)
```

The `.cwasm` stem must equal the app's `manifest()` id (the spawn key), e.g.
`browser.cwasm` ↔ `declare_manifest!("browser", ...)`.

## ⚠️ `.cwasm` files go stale when the kernel's Wasmtime tunables change

A `.cwasm` is AOT machine code bound to the **exact engine config** it was
precompiled with (`memory_reservation` & co.). If the kernel's config changes
(`kernel/src/wasm/wt/mod.rs::engine_config` + `tools/wt-precompile`, e.g.
CHANGELOG 422 on 2026-06-10), every `.cwasm` in this folder — and any copy on
the disk's `/mnt/apps` — is **rejected at load** and vanishes from the
launcher. The only symptom is a serial line:

```
WARN wm  probe '/bin/<app>.cwasm': deserialize failed: Module was compiled
         with a memory reservation of '0' but '268435456' is expected
```

Fix: re-run the AOT step (sources stay valid). Either the SDK's `build.ps1`
(it rebuilds `wt-precompile` from this checkout on every run, so it always
matches), or directly:

```
cargo build --release --manifest-path tools/wt-precompile/Cargo.toml
tools/wt-precompile/target/release/wt-precompile <app>.wasm apps/<app>.cwasm
```

See `docs/api/README.md` § ".cwasm compatibility".
