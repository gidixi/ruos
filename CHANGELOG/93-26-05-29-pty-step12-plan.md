# 93 — Plan: Step 12 PTY (5 task)

**Data:** 2026-05-29

## Cosa

Plan in `docs/superpowers/plans/2026-05-29-rust-pty-step12.md`.
5 task TDD bite-sized:

1. **PTY core** (pty/{mod,pair,ldisc,termios}.rs): infra senza wiring.
2. **VFS integration**: PtySlaveFile + TmpKind::PtySlave + mount
   /dev/pts/0..3.
3. **Wire keyboard + console drain + RuntimeState FD 0/1/2**:
   0xE0 latch, ANSI emit, drop legacy queue + Stdin + KbdReadChar.
4. **WASIX host fns** tcgetattr/tcsetattr/isatty.
5. **Shell line editor**: raw mode + arrows + history + tab +
   Ctrl-A/E/L/C.

Numerazione changelog implementer: 94-98.

Sentinel `shell: init.sh complete` invariato attraverso tutti i task.

## Perché

Step 12 PTY tradotto in 5 commit incrementali. Multi-agent ready.

## File toccati

- docs/superpowers/plans/2026-05-29-rust-pty-step12.md (nuovo)
- CHANGELOG/93-26-05-29-pty-step12-plan.md (nuovo)
