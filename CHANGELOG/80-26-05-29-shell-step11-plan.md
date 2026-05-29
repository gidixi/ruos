# 80 — Piano implementazione: Shell Step 11

**Data:** 2026-05-29

## Cosa

Scritto piano Step 11 in
`docs/superpowers/plans/2026-05-29-rust-shell-step11.md`. Tre task
TDD bite-sized:

1. **Host fns + lifecycle args**: `wasm/host/proc.rs` con `ruos_exec` +
   `ruos_readdir`, SuspendReason variants, Fiber dispatch arms +
   `set_args`, lifecycle args_* reali (popolano da RuntimeState.args).
   HELLO: `init.wasm: argv0=/init.wasm` (validation via init.wasm
   esistente che ora stampa argv[0]).
2. **External tools**: 4 user crates nuovi (shell/ls/cat/echo),
   `/etc/init.sh`, 5 moduli Limine, Makefile build + iso staging
   /etc + /bin, executor spawn shell. HELLO:
   `shell: init.sh complete`.
3. **Drop kbd_echo_task**: closes F7 di docs/followups/step-10-5.md.
   HELLO invariato.

Numerazione changelog implementer: 81-83.

## Perché

Tradurre lo spec Step 11 in passi eseguibili TDD. shell.wasm come
Fiber + ruos_exec coerente con WASIX-first; tool ls/cat/echo dimostrazione
end-to-end userland reale.

## File toccati

- docs/superpowers/plans/2026-05-29-rust-shell-step11.md (nuovo)
- CHANGELOG/80-26-05-29-shell-step11-plan.md (nuovo)
