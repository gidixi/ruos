# 53 — Spec design: Async executor cooperativo (Step 9)

**Data:** 2026-05-28

## Cosa

Scritta la spec dello Step 9 in
`docs/superpowers/specs/2026-05-28-rust-async-executor-design.md`. Decisioni
strategiche (lockate via brainstorm interattivo):

- **Scope smoke**: full A+B+C — executor reale + 2 task interleaved + future
  svegliata da IRQ (timer + keyboard).
- **Executor crate**: `embassy-executor` 0.6 `default-features=false`, no
  `arch-*`, custom `__pender`. Idle = `hlt` in kmain loop.
- **kmain shape α**: `vfs::block_on` resta per init sync (VFS smoke
  intatto), poi `EXECUTOR.run(…)` prende controllo for-ever.
- **Primitives**: hand-roll `Delay(ticks)` + replace keyboard ISR (push
  in queue async, niente più kprintln dalla ISR). ~120 LoC nostri.
  embassy-time rimandata a Step 14.

Componenti: `executor/{mod,delay}.rs`, `keyboard/{mod,queue}.rs`,
modifiche minimal a `timer.rs` (+ `delay::timer_tick`), `main.rs`
(`executor::run` rimpiazza `loop { hlt }`), `Makefile` (HELLO →
`ruos: async tick=2`).

Out of scope: preemption (droppato dal pivot), SMP, embassy-time,
dynamic spawn.

## Perché

Step 9 della roadmap WASM-first. Sblocca Step 10 (WASM runtime async
host fns) e Step 11 (shell come task). Rimuove il `block_on` noop_waker
come unico runtime e introduce wake-by-IRQ vero.

## File toccati

- docs/superpowers/specs/2026-05-28-rust-async-executor-design.md (nuovo)
- CHANGELOG/53-26-05-28-async-executor-spec.md (nuovo)
