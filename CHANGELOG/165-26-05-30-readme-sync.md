# 165 — README: allineamento a stato Step 16 corrente

**Data:** 2026-05-30

## Cosa
Riallineato `README.md` allo stato reale del repository dopo i commit
delle entry [[160-26-05-30-init-smoke-split]] – [[164-26-05-30-exec-inherit-pty]]:

- **Tagline**: rimossa l'affermazione obsoleta "boots in QEMU, prints to
  COM1, heap" sostituita con la descrizione attuale (boot QEMU/VBox/USB,
  `.wasm` userspace, TCP/IP, AHCI+FAT, SSH).
- **North star**: rimosso completamente il riferimento ai container
  Podman-style (drop esplicito del pivot 2026-05-28). Inline la nota
  sul WASM sandbox + async cooperative.
- **Status table**: Step 16 ora elenca "pubkey **+ password** auth" e
  "runs disklessly" (entry 161 + 162).
- **Prerequisites**: aggiunti `mtools` + `dosfstools` (servono al target
  `disk` per generare la FAT); `sshpass` come opt-in per il test
  `run-passwd-test`.
- **Test section**: sostituito l'esempio fittizio "ruos: hello serial /
  heap ok / alloc box" con l'elenco reale dei 4 target di test
  (`run-test`, `run-ssh-test`, `run-passwd-test`,
  `run-passwd-diskless-test`) e i marker che ciascuno controlla.
- **SSH section**: nuova tabella metodi (password vs pubkey); workflow
  out-of-the-box (`ssh root@<ip>` password `ruos`) prima del workflow
  pubkey legacy; documentato `make iso RUOS_PASSWORD=...` e
  `make passwd-on-disk RUOS_PASSWORD=...`. Aggiornati i "Current limits
  (MVP)" — rimosso "pubkey-only", aggiunta nota di sicurezza sul
  default plain-text nel binario kernel.
- **Repository layout**: aggiornato per riflettere i moduli
  effettivi sotto `kernel/src/` (boot/, apic/, vfs/, ssh/, wasm/, ...) +
  aggiunti `user/`, `user-bin/`, `tests/`. Chiarito che
  `third_party/sunset/` è vendorato e committato, mentre
  `third_party/limine/` è clonato a runtime e gitignorato.

## Perché
La sezione "north star" del README diceva ancora "evolve into an OS
capable of running containers (Podman-style)" — direttamente in
contraddizione con il pivot del 2026-05-28 documentato nel CLAUDE.md
("Niente Linux ABI / ELF userland. App = .wasm. Niente Podman."). I
contributori esterni che leggevano solo il README ne avrebbero ricavato
intenzioni di progetto sbagliate.

Inoltre l'esempio output del test era roba dei primissimi commit
("ruos: hello serial / heap ok / alloc box=0xCAFEBABE"), e la sezione
SSH manuale richiedeva ancora di copiare a mano le chiavi pubblice — un
flusso che non è più necessario per giocare con la VM (basta
`ssh root@... ; password: ruos`).

## File toccati
- README.md
