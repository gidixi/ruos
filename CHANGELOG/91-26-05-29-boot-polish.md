# 91 — Boot polish: TSC clock + log format + Unix-style handoff

**Data:** 2026-05-29

## Cosa

Output boot allineato al mockup spec dell'utente + Unix-style handoff
alla shell:

### Logger format
- Level: `INFO`/`WARN`/`ERR ` (4 char, full word) — era `I`/`W`/`E`.
- Module col: 4 char compact — era 8 padded.
- Timestamp: `[T+SS.MMMs]` no padding — era `[T+   SS.MMMs]`.
- `irq` → `intr` (interrupts phase + timer.rs).

### TSC pre-timer clock
- `kernel/src/boot/clock.rs` (nuovo): `init()` calibra TSC via PIT
  channel 2 (10ms round-trip). `elapsed_ms()` ritorna ms da boot.
- Logger usa `boot::clock::elapsed_ms()` invece di `timer::ticks()`,
  così le righe arch/mem/heap/acpi (pre-LAPIC) hanno timestamp reali
  e non `T+0.000s`.

### Banner
- ASCII chars (`+-|`) invece di Unicode box-drawing (`╔═╗║`) — font
  framebuffer `noto-sans-mono-bitmap` non copre U+2500-U+257F.
- Aggiunge `NNN MiB RAM / 1 CPU / WASIX-bootstrap` (queries Limine
  memmap diretto).
- Re-stamped post-fb-attach in `devices::init` così appare ANCHE sul
  framebuffer (era solo seriale prima).

### Unix-style boot handoff
- `executor::run` non auto-spawn più init/server/client.wasm — solo
  `/bin/shell.wasm`. Step 10/10.5 demo wasm restano file in tmpfs
  lanciabili manualmente da shell.
- `tick_task` dropped runtime `kprintln!("ruos: async tick={}", n)` +
  `kprintln!("ruos: executor up")`. Diventa heartbeat silente.
- `wasm fiber` runtime traces (suspend/sleep/sock-recv-n=...) gated
  dietro nuova feature `wasm-trace`. Default boot OFF.
- `user-bin/init.sh` slim: solo `echo ruos boot OK` (era 4-line demo).
- `user/shell/src/main.rs`: dopo `init.sh complete`, sleep 1s, ANSI
  clear screen (`\x1b[2J\x1b[H`), greeting verde, prompt
  interattivo automatico — no enter needed.

## Perché

User feedback: "rendere boot piu pro non hobby os" + "rimuovi i test
nell'avvio normale + dopo 1s mostra shell". Output ora pulito + Unix-
like + framebuffer-renderable.

Font box-drawing non disponibile in `noto-sans-mono-bitmap` 0.3 (solo
ASCII + Latin-1, U+0020..U+00FF). Box chars sono U+2500..U+257F.
ASCII fallback (`+-|`) è il workaround pulito; bundle font più grosso
differito a Step 13 (GUI).

## File toccati

- kernel/Cargo.toml (+feature `wasm-trace`)
- kernel/src/boot/{mod,log,banner}.rs
- kernel/src/boot/clock.rs (nuovo)
- kernel/src/boot/phases/{interrupts,devices}.rs
- kernel/src/main.rs (clock::init prima del banner)
- kernel/src/timer.rs ("intr")
- kernel/src/executor/mod.rs (drop demo wasm + heartbeat silenzioso)
- kernel/src/wasm/{fiber,mod}.rs (wtrace! gating)
- user-bin/init.sh (slim)
- user/init/src/main.rs (banner ASCII)
- user/shell/src/main.rs (1s sleep + ANSI clear + greeting)
- CHANGELOG/91-26-05-29-boot-polish.md (nuovo)
