# 100 — Pulizia rumore di boot + rimozione test bin legacy

**Data:** 2026-06-08

## Cosa

Ripulito il rumore nel log di boot e tolti i binari di test legacy dall'immagine:

- **Supervisor heartbeat**: non logga più `super: all N cores alive` ogni
  secondo. Ora lo stato di salute steady-state è silenzioso; resta il `WARN`
  su core muti e una sola riga INFO `all N cores alive (recovered)` al ritorno
  dallo stato muto. Prima floodava serial + ring buffer `dmesg`.
- **Banner duplicato su serial**: il re-stamp post-attach del framebuffer ora
  va SOLO al framebuffer (`banner::stamp_fb_only` + `MultiConsole::write_fb_only`),
  così la serial non riceve due copie del banner.
- **Riga `ruos: gfx init ...`** (kprintln grezzo) convertita a `binfo!("gfx", ...)`
  per stare nel formato strutturato `[T+..] INFO mod msg`.
- **Test bin legacy** `readdirtest`, `spinloop`, `smptest` rimossi: tolti da
  `BIN_TOOLS` (Makefile), da `limine.conf` (le entry module_path/module_cmdline,
  che causavano un panic Limine "Failed to open module" a boot), e dai members
  del workspace `user/`; cancellati i crate sorgente e gli artefatti `.wasm`.
  Rimossa la chiamata `readdirtest` da `smoke.sh` e la relativa asserzione
  `TEST_FAIL_READDIR` da `make run-test`.
- **Test target dipendenti disattivati**: i 3 bin alimentavano test harness
  dedicati. Rimossi i target Makefile `run-fuel-test` (fuel-kill via `spinloop`),
  `run-smp2-test` (SMP speedup via `smptest`), `run-m2b1-test` (GPT data-part via
  `readdirtest` come file campione) + le entry `.PHONY`, e cancellati gli script
  `tests/{fuel-test,smp2-test,m2b1-test}.sh` + `user-bin/m2b1-init.sh`.
  `run-smp-test` (non usa quei bin) resta.
- **Fixture cwasm in `/bin`**: tolte le copie `reactor-close.cwasm` e
  `probe.cwasm` da `$(ISO_ROOT)/bin` nei target `iso` e `test-boot`. Sono
  fixture dei soli demo boot-checks (`run_lifecycle_demo`, `run_wasip1_probe_demo`),
  già `include_bytes!` nel kernel (`wm.rs`) — niente le carica da `/bin` a
  runtime. I prereq di build restano (servono al kernel per `include_bytes!`).

## Perché

Il log di boot/`dmesg` era inutilizzabile: il heartbeat del supervisor lo
riempiva di una riga al secondo. Banner doppio e riga gfx non strutturata
erano residui. I tre tool erano diagnostici di vecchi step (readdir/SMP/preempt)
che non servono nell'immagine a regime.

> **Nota copertura**: `readdirtest` faceva da regression test per `fd_readdir`
> (`std::fs::read_dir`) nel battery `make run-test`. Rimuovendolo si perde quella
> asserzione di CI. Da reintrodurre se serve di nuovo coprire `fd_readdir`.

## File toccati

- kernel/src/executor/mod.rs
- kernel/src/gfx/mod.rs
- kernel/src/console/mod.rs
- kernel/src/boot/banner.rs
- kernel/src/boot/phases/devices.rs
- Makefile
- user/Cargo.toml
- user-bin/smoke.sh
- user/readdirtest/ (rimosso)
- user/spinloop/ (rimosso)
- user/smptest/ (rimosso)
- user-bin/{readdirtest,spinloop,smptest}.wasm (rimossi)
