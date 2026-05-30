# 158 — SSH Task 9: docs + roadmap

**Data:** 2026-05-30
**Status:** DONE. Step 16 documentato e marcato completo.

## Cosa

### Roadmap (`docs/superpowers/roadmap-rust-os.md`)

- Step 16 — SSH server → **✅ DONE**: riassunto di cosa funziona (KEX +
  chacha20-poly1305, auth pubkey ed25519 firma reale, shell PTY interattiva,
  exec non-interattivo), crate `sunset` vendorizzato, link allo spec, elenco
  limiti MVP.
- Step 15 — AHCI/FAT → **✅ DONE** (era già mergiato, header non aggiornato).

### README (`README.md`)

- Tabella Status riscritta: era pre-pivot (tasking/syscall ABI/ring 3, ferma
  allo Step 5). Ora riflette gli Step 1-16 reali (tutti ✅) + nota sul modello
  userland WASM e Step 17 come prossimo.
- Nuova sezione **SSH**: come funziona (porta 22, host key auto a
  `/mnt/host.key`, authorized key `/mnt/auth.key`), `make run-ssh-test`, passi
  manuali (ssh-keygen → mcopy pubkey su `::/auth.key` → connessione
  interattiva ed exec), nota perm chiave 0600, elenco limiti MVP.
- Corretta nota stale "framebuffer su roadmap dopo Step 5" → framebuffer
  console (Step 8) attivo + shell locale via PS/2.

## Perché

Chiudere Task 9 dello Step 16: roadmap allineata, README utilizzabile per
generare host key / mettere la pubkey sull'immagine / connettersi.

## File toccati

- docs/superpowers/roadmap-rust-os.md (Step 15 + 16 → DONE)
- README.md (tabella Status, sezione SSH, fix nota framebuffer)
