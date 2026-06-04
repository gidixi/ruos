# 251 — Memoria eseguibile W^X

**Data:** 2026-06-04

## Cosa
Aggiunto allocatore di memoria eseguibile (W^X) sul paging:
`mapper::set_flags` (cambia i permessi di una pagina mappata via `update_flags`)
e `memory::exec` con `alloc_exec`/`protect_exec`/`free_exec` su una finestra VA
dedicata (scrivibile+NX → flip a RX). Self-test boot-checks: emette
`mov eax,42; ret`, protegge, chiama, verifica 42 (`mem exec W^X self-test ok`).

## Perché
Prerequisito #2 del desktop egui (piano
docs/superpowers/plans/2026-06-04-exec-memory-wx.md). Valida il meccanismo di
pagine eseguibili W^X usato poi dal runtime Wasmtime AOT (#3).

## File toccati
- kernel/src/memory/mapper.rs
- kernel/src/memory/exec.rs
- kernel/src/memory/mod.rs
- kernel/src/boot/phases/interrupts.rs
