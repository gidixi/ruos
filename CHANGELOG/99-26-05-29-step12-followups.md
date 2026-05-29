# 99 — Followups tracciati Step 12 PTY

**Data:** 2026-05-29

## Cosa

`docs/followups/step-12.md` con 7 item dal review:

- F1 🟠 ConsoleFile::read usa master_output_read (semanticamente wrong)
- F2 🟠 Termios layout assunto non verificato
- F3 🟡 /dev/ptmx dynamic allocator (pre-Step 15)
- F4 🟡 line editor interattivo non testato programmaticamente
- F5 🟡 multi-iov PTY EINVAL (carry-over)
- F6 🟢 pty::master_input_push ISR contention nit
- F7 🟡 history non persiste

F1/F2 pre-Step 15. Altri opportunistici.

## Perché

Mirror pattern step 8/9/10/11. Step 12 chiude APPROVE WITH FOLLOWUPS.

## File toccati

- docs/followups/step-12.md (nuovo)
- CHANGELOG/99-26-05-29-step12-followups.md (nuovo)
