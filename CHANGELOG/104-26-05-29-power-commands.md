# 104 — poweroff + reboot

**Data:** 2026-05-29

## Cosa

Aggiunti builtin shell `poweroff` + `reboot`.

### Kernel
- `kernel/src/power.rs` (nuovo):
  - `reboot() -> !`: keyboard controller port 0x64 cmd 0xFE (1024
    retry loop con wait input buffer). Fallback: null IDT + int3 →
    triple-fault. Halt finale.
  - `poweroff() -> !`: tenta in sequenza:
    - 0x604 = 0x2000 (QEMU isa-debug-exit)
    - 0x4004 = 0x3400 (VirtualBox)
    - 0xB004 = 0x2000 (QEMU q35 ACPI shutdown)
    - HLT loop finale.
- `kernel/src/main.rs`: `mod power;`
- `kernel/src/wasm/host/proc.rs`: `ruos_poweroff` + `ruos_reboot`
  host fns sotto module "ruos". Signature `() -> !` (mai ritorna).
  Logga `kprintln!("ruos: poweroff/reboot requested by wasm")` poi
  chiama `crate::power::*`.

### Shell
- extern "C" import `poweroff` + `reboot`.
- `run_command` dispatch builtin → unsafe call (mai ritorna).
- `complete_command` aggiunge `poweroff` + `reboot` a tab completion.
- `builtin_help` riscritta multi-line con descrizione di ognuno.

ACPI S5 sleep proper (FADT + DSDT _S5 SLP_TYPa parsing) rimandato:
serve AML parser. Le 3 port I/O coprono QEMU + VBox.

## Perché

User feedback: "mancano poweroff/reboot". Cleanup VM/HW shutdown
graceful.

## File toccati

- kernel/src/power.rs (nuovo)
- kernel/src/main.rs (+mod power)
- kernel/src/wasm/host/proc.rs (+2 host fns)
- user/shell/src/main.rs (+2 extern + dispatch + completion + help)
- user-bin/shell.wasm (rebuilt)
- CHANGELOG/104-26-05-29-power-commands.md (nuovo)
