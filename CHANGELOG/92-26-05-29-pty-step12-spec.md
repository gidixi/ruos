# 92 — Spec: PTY Step 12 (pseudo-terminal + line discipline)

**Data:** 2026-05-29

## Cosa

Spec in `docs/superpowers/specs/2026-05-29-rust-pty-step12-design.md`.

Multi-PTY pool statico (4 pair) + line discipline POSIX-style + termios
host fns + shell.wasm raw-mode line editor.

Decisioni strategiche:
- 4 PTY pair pre-allocati. Slave esposti `/dev/pts/0..3`. Pair 0 wired al
  boot (keyboard ↔ shell). Pair 1-3 per SSH/futuro.
- Master access kernel-internal (no `/dev/ptmx` Step 12). Rimandato Step 15.
- Termios subset: `c_iflag(ICRNL)`, `c_oflag(OPOST|ONLCR)`, `c_lflag(ICANON|
  ECHO|ISIG|IEXTEN)`, `c_cc[NCCS]`. Layout 60-byte mirror wasi-libc.
- Line editor lato shell.wasm: kernel ldisc fa solo cooked default,
  shell turns raw + gestisce arrows/history/tab/Ctrl-keys.
- Drop legacy `keyboard::queue` + `FdEntry::Stdin` + `SuspendReason::
  KbdReadChar`. Stdin = open `/dev/pts/0` slave.
- 0xE0 prefix latch + ANSI escape emission (chiude Step 9 F3).

Decomposizione 5 task:
1. PTY core (pair + ldisc + termios).
2. VFS integration (/dev/pts/0..3 + PtySlaveFile).
3. Wire keyboard + console_drain_task + boot FD setup.
4. WASIX tcgetattr/tcsetattr/isatty host fns.
5. Shell line editor (arrows/history/tab/Ctrl-keys).

Out of scope: /dev/ptmx allocator, ptsname, TIOCGWINSZ, job control,
pipes/redir, history persist, vi-mode.

Closes followups precedenti: Step 9 F3 (0xE0 latch), Step 10.5 F4
(keyboard single-Waker), Step 11 F7 (KbdReadChar verbose).

## Perché

Step 12 roadmap. Sblocca shell interattiva user-friendly + abstraction
per SSH (Step 15).

## File toccati

- docs/superpowers/specs/2026-05-29-rust-pty-step12-design.md (nuovo)
- CHANGELOG/92-26-05-29-pty-step12-spec.md (nuovo)
