# 84 — Followups tracciati per Step 11

**Data:** 2026-05-29

## Cosa

Creato `docs/followups/step-11.md` con 13 followup emersi dal
whole-implementation review:

- **F1** 🟠: `EXEC_QUEUE` single-slot — secondo `ruos_exec` concurrent
  perde request. Pre-Step-15 (SSH).
- **F2** 🟠: `ExecFuture::poll` post-load-store-waker race. Pre-SMP.
- **F3** 🟡: `ExecSlot.exit_code: *mut i32` dead field.
- **F4** 🟠: keyboard queue single-Waker race (secondo reader stdin
  perde wake). Pre-Step-15.
- **F5** 🔴: `path_open` ignora oflags → cat /missing crea file.
  Pre-esistente Step 10.5, riconfermato.
- **F6** 🟡: `limine.conf stack_size: 0x200000` defensive ma redundant.
- **F7** 🟠: `fd_filestat_get` size=0 (read-loop fallback works ma
  pre-alloc-based tool inefficient).
- **F8** 🟡: verbose suspend kprintln.
- **F9** 🟡: `decode_argv` silent failure → empty args + errno=0.
- **F10** 🟡: Makefile pattern rule per user wasms.
- **F11** 🟡: host fns len bounded check.
- **F12** 🟡: wasm_task pool_size = 4 stretto.
- **F13** 🟢: dirent layout duplication kernel/ls.wasm.

## Perché

Mirror pattern Step 8/9/10/10.5. F1/F2/F4 vanno chiusi prima di
Step 15 (SSH = 2° wasm interattivo). F5 è critical ma pre-esistente,
non blocca Step 11.

## File toccati

- docs/followups/step-11.md (nuovo)
- CHANGELOG/84-26-05-29-step11-followups.md (nuovo)
