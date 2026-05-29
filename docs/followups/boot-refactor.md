# Boot refactor — followups

Followup emersi dal whole-implementation review. Aperti al merge di
`feature/boot-refactor` → `main`. Nessuno blocca i prossimi step.

## F1 — Banner SHA stale tra commit

**File:** `kernel/build.rs`
**Severity:** 🟢 nit

`build.rs` ha `cargo:rerun-if-changed=../.git/HEAD` ma cargo cache la
`RUOS_GIT_SHA` env var su build incrementali. Dopo T3 commit, il banner
mostra ancora la SHA del T2 finché `make clean` o `cargo clean` non
gira.

**Fix:** aggiungere `cargo:rerun-if-changed=../.git/refs/heads/` per
catturare ogni commit, o aggiungere `cargo:rerun-if-env-changed` con
una var di stamp.

## F2 — Banner content padding lopsided

**File:** `kernel/src/boot/banner.rs`
**Severity:** 🟡 cosmetic

Border è 47 caratteri, content lines hanno padding diverso → leggera
disallineamento visivo. Es:

```
  ║   ruos v0.1.0     (f317f42, 2026-05-29)           ║   ← extra space
  ║   x86_64-unknown-none / Limine 11.4.1         ║       ← right-padded
```

**Fix:** calcolare width content uniforme, usare `{:<41}` con format
left-aligned dentro padding noto.

## F3 — `make test-boot` non controlla QEMU exit

**File:** `Makefile::test-boot`
**Severity:** 🟡 robustness

`timeout 60 qemu ... || true` swallow QEMU exit. Se QEMU crasha
(segfault, missing dep), `test-boot.log` è vuoto e grep error
misleading.

**Fix:** check QEMU exit code esplicitamente prima del grep, oppure
controllare `[ -s build/test-boot.log ]`.

## F4 — `executor::run` log "executor up" runtime non strutturato

**File:** `kernel/src/executor/mod.rs:141`
**Severity:** 🟢 nit

Il `kprintln!("ruos: executor up")` dentro tick_task è one-shot al
boot. Userland phase già emette `binfo!("user", "executor starting")`
prima. Duplicato + format misto.

**Fix:** migrare a `binfo!("user", "tick task spawned")` o rimuovere
duplicato.

## F5 — Banner Unicode box chars on framebuffer

**File:** `kernel/src/boot/banner.rs`
**Severity:** 🟡 pre-Step-13

Banner stampato pre-`devices::init`, solo serial. Se Step 13+ rifa
stamp post-fb-attach, glyph `╔╝═║` potrebbe non esistere in
`noto-sans-mono-bitmap` (font Step 8).

**Fix:** verificare copertura font per box-drawing chars (U+2500..U+257F)
o usare fallback ASCII (`+`/`-`/`|`) se non disponibile.

## F6 — `get_acpi_info()` clona Vec ad ogni read

**File:** `kernel/src/boot/phases/mod.rs::get_acpi_info`
**Severity:** 🟢 perf nit

Ogni `get_acpi_info()` clona `AcpiInfo` intero incluso `Vec<IrqOverride>`.
Cheap oggi (0-2 overrides) ma waste.

**Fix:** ritornare `MutexGuard` o extract by-field-once. Da
considerare se ACPI cresce.

## F7 — `arch::init` INT3 prints via kprintln, non binfo

**File:** `kernel/src/idt.rs` (handler #BP)
**Severity:** 🟢 by-design

Handler exception (#DE, #UD, #DF, #BP) usano `kprintln!` perché
chiamare `binfo!` deadlock-a su `CONSOLE.lock()` se exception fires
durante boot logger emit. Sotto cgcd `boot-checks` la riga
`ruos: bp ok rip=...` appare inframezzata.

**Fix (cosmetic only)**: documentare esplicitamente che exception
handlers stay kprintln, oppure flush console buffer prima di
sti+int3.
