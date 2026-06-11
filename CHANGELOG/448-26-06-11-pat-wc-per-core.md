# 446 — PAT write-combining programmato su ogni core (fix GUI lenta su hardware reale)

**Data:** 2026-06-11

## Cosa

Nuova `cpu::init_pat()`: programma IA32_PAT (MSR 0x277) con il layout Limine
(PA0=WB, PA1=WT, PA2=UC-, PA3=UC, PA4=WP, PA5=WC, PA6=UC-, PA7=UC) e ricarica
CR3 (il TLB cachea il memory type per entry). Chiamata sul BSP in
`boot/phases/arch.rs` (difensiva — Limine lo fa già) e su ogni AP in
`cpu/ap.rs::ap_entry` prima di qualunque accesso al framebuffer.

## Perché

Il PAT MSR è **per-core**. Limine mappa il framebuffer write-combining via PAT
index 5 e programma il PAT solo sul BSP; gli AP partono col default di reset
dove index 5 = WT. I blit della GUI girano sui core compute/GUI (AP): ogni
present scriveva la VRAM write-through (non combinato) → ~MB/s invece di ~GB/s
su hardware reale. Nelle VM il framebuffer è RAM host, quindi il bug era
invisibile (VBox fluido, macchina reale a scatti).

## File toccati

- kernel/src/cpu/mod.rs
- kernel/src/cpu/ap.rs
- kernel/src/boot/phases/arch.rs
