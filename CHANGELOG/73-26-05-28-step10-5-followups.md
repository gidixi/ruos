# 73 — Followups tracciati per Step 10.5

**Data:** 2026-05-28

## Cosa

Creato `docs/followups/step-10-5.md` con 11 followup emersi dal
whole-implementation review di Step 10.5:

- **F1**: `sock_open`/`bind`/`listen`/`connect` host fns mancanti
  (pre-allocate kernel-side per limit wasi-libc). Architecturally
  significant ma "no preload" per data exchange resta vero.
- **F2**: `sock_accept` ritorna stesso FD di listen (smoltcp
  single-socket model). Multi-client server non funziona.
- **F3**: `embassy-futures` dep unused post-T3, rimuovere.
- **F4**: `vfs::VfsError` → wasi_errno mapping mancante.
- **F5**: `path_open` ignora oflags/rights/fdflags.
- **F6**: multi-iov per Socket/Vfs/Stdin = EINVAL (bash userà
  readv/writev).
- **F7**: race `KbdReadChar` vs `kbd_echo_task` (single-consumer).
  Da risolvere prima dello Step 11 (shell).
- **F8**: spec flow diagram non corrisponde all'as-built (socket
  activation model).
- **F9**: plan menziona `ResumableInvocation` (nit storico).
- **F10**: comment offset `clock_id` (1 line fix).
- **F11**: `poll_oneoff` event userdata = 0 (per multi-sub futuro).

F1, F2, F3, F7 = 🟠 Important. Altri = 🟡/🟢.

## Perché

Mirror del pattern Step 8/9/10. Tutti non-blocking per chiudere
Step 10.5. F7 va affrontato prima dello Step 11 (shell ha bisogno
exclusive keyboard ownership).

## File toccati

- docs/followups/step-10-5.md (nuovo)
- CHANGELOG/73-26-05-28-step10-5-followups.md (nuovo)
