# 213 — README sync: SMP + USB + rtop + SSD installer

**Data:** 2026-06-03

## Cosa
Allineato il README allo stato di `main`. Era fermo al commit 3b5f4ae
(2026-05-31, ~changelog 181: fd_readdir + hardening Fase A); mancavano tutti i
sottosistemi 182-212.

- Intro: aggiunti monitor `htop`-style, USB keyboard (xHCI, hub, hot-plug),
  SMP compute offload, self-install su SSD.
- Status: nuova tabella "Built alongside (beyond the numbered roadmap)" con
  SMP, USB, `rtop`, SSD self-install (Step 17 mouse/rlvgl resta `next`).
- Nuova sezione "Userspace tools": elenco raggruppato dei ~54 tool `/bin`.
- Sezione Test riscritta: dai 4 target originali a tutti i target per
  sottosistema (e1000, pipe, fuel, ctrlc, ssh-idle, smp, smp2, rtop, usb-key,
  usb-hub, usb-hotplug, gpt, m2a, m2b1, m2b2); marker `run-test` aggiornati
  (usb xhci up, rtop uptime).
- Nuova sezione "Install to an internal disk (SSD)": comando `install` (+
  guardia /mnt, `mkdisk`/`mkboot`).
- Real-hardware: keyboard PS/2 **o USB**; nota sugli hardening boot reali
  (clock PIT-free, LAPIC calibrato su ACPI PM timer, handoff USB legacy).
- Repository layout: aggiunti `cpu/`, `smp/`, `sched/`, `usb/`, `service/`,
  `sync/`, `gpt.rs`/`disk.rs`/`crc32.rs`.

## Perché
La documentazione era vecchia (~2 settimane, 31 changelog di ritardo): non
citava nessuna delle feature maggiori mergiate dopo Step 16.

## File toccati
- README.md
- CHANGELOG/213-26-06-03-readme-sync-smp-usb-installer.md (questo file)
