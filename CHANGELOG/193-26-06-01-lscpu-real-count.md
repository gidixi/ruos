# 193 — lscpu: report real online CPU count

**Data:** 2026-06-01

## Cosa

`ruos_cpuinfo` (host fn dietro `lscpu`) ritornava un n_cpus hardcoded `"1"`
con commento "kernel is currently single-CPU". Dopo SMP Fase 1 gli AP vengono
avviati e registrati online, ma il dato esposto restava stale → `lscpu`
mostrava 1 CPU anche con 4 core.

Fix: `n_cpus = 1 (BSP) + crate::cpu::cpus_online()` (il BSP non chiama
`mark_online`, quindi va sommato). `lscpu` ora riporta il count reale.

Verificato: QEMU `-smp 4` via SSH → `CPU(s): 4`.

## Perché

Il bring-up SMP funzionava (3/3 APs online) ma il tool di introspezione
mostrava il vecchio valore hardcoded, facendo sembrare il sistema single-CPU.

## File toccati

- kernel/src/wasm/host/sysinfo.rs (ruos_cpuinfo: n_cpus reale)
