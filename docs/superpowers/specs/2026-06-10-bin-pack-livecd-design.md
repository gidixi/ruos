# Live-CD /bin via archivio compresso (bin.bgz) caricato da Limine

**Data:** 2026-06-10
**Stato:** approvato (brainstorming → design ok)

## Problema

Il live-CD su **hardware reale** ha richiesto un percorso fragile per popolare
`/bin`. Storia (changelog 348→359):

- **348**: i `/bin/*` tolti dai moduli Limine (61 moduli, ~45MB in RAM) →
  `/bin` letto off-boot dal medium.
- **352-358**: su HW reale il medium è la chiavetta USB (USB Mass-Storage) →
  serviti driver USB-MSC (BOT/SCSI), scala settore 2048↔512, port-power,
  pump enumerazione, fix `/mnt` ATAPI.
- **359**: multi-xHCI (chiavetta sul secondo controller mai inizializzato).

Tutto questo per leggere `/bin` a runtime dal medium. È complesso, dipende
dall'hardware (xHCI, port-power, timing), e la chiavetta deve restare inserita.

### Idea

Limine legge il medium di boot **via firmware UEFI**, sempre, su ogni HW. Quindi:
impacchetta tutto `/bin` in **un singolo archivio compresso** che Limine carica
come modulo opaco in RAM. Dopo l'handoff, il **kernel** lo decomprime in tmpfs
`/bin`. Niente driver USB-MSC a runtime, niente ATAPI per `/bin`, e la **chiavetta
USB è scollegabile** appena finito il boot (l'archivio è già in RAM).

### Vincolo esplicito

Limine fa solo da **corriere di un blob**: non monta i singoli file, non conosce
il contenuto. La logica unpack+mount vive nel **kernel**, come fase di boot.

## Misure reali (/bin sull'ISO)

| Tipo            | Conta | Size totale | Note                          |
|-----------------|-------|-------------|-------------------------------|
| `.wasm` (CLI)   | 56    | 3.6 MB      | tool shell (ls, cat, gzip…)   |
| `.cwasm` (GUI)  | 11    | 128 MB      | app egui AOT; `viewer` da 61MB |

Il peso è nei `.cwasm`. Heap kernel = 384 MB. **I moduli Limine vivono in memoria
HHDM, fuori dallo heap talc** (mai liberati, validi per tutta la vita kernel,
vedi `kernel/src/modules.rs`): quindi `bin.bgz` (~65 MB compresso) **non pesa
sullo heap** — lo legge e basta. Pressione heap = solo tmpfs finale (132 MB) +
buffer transitorio di un file alla volta.

## Decisioni (brainstorming)

- **Strategia A**: impacchetta TUTTO `/bin` (wasm + cwasm). GUI inclusa e
  scollegabile. (Scartate: C = solo CLI; B = A + USB-MSC fallback.)
- **bin.bgz assente/corrotto → mini-fallback**: un set rescue minimo
  (`shell.wasm` + diagnostica) resta come moduli Limine separati in HHDM, montato
  in `/bin` solo se l'unpack fallisce. Sistema sempre bootabile a prompt.
- **ISO ship solo `bin.bgz`**: `/bin` loose rimosso del tutto dall'ISO (niente
  ridondanza ATAPI).

## Architettura

### Formato archivio `bin.bgz` (container di membri gzip indipendenti)

```
offset  campo
0       magic  "RBIN"            (4 byte)
4       ver    u8 = 1
5       count  u32 LE            (numero di entry)
9       entries[count]:
          name_len u16 LE
          name     [name_len] byte (UTF-8, es. "ls.wasm")
          gz_len   u32 LE
          gz       [gz_len] byte  (membro gzip completo: header+deflate+trailer)
```

Tutto little-endian (x86-64). Ogni file è un **membro gzip indipendente**: il
kernel chiama `gzip_core::decompress(member)` per ciascuno, scrive `/bin/<name>`,
libera il buffer. Niente parser tar nel kernel.

**Perché membri separati e non un solo gzip-di-tar**: con membri separati il peak
heap è `tmpfs finale (132MB) + un solo file decompresso transitorio (max viewer
61MB) ≈ 193MB`. Un singolo gzip di un tar costringerebbe a decomprimere l'intero
stream (132MB) prima di poterlo spezzare → peak ~264MB. Membri separati riusano
`decompress` as-is e dimezzano il transitorio.

### `gzip-core` → no_std + alloc (riuso in kernel E userland)

La logica gzip è già scritta e testata (`user/gzip-core`). Si rende `no_std` per
usarla anche nel kernel:

- `#![no_std]` + `extern crate alloc`; `Vec`/`Box`/`String` da `alloc`,
  `fmt` da `core` (già così in `format.rs`/`crc32.rs`).
- `miniz_oxide` con `default-features = false, features = ["with-alloc"]`.
- `cli.rs` + l'export `run_cli` dietro `#[cfg(feature = "std")]`.
- `Cargo.toml`: `[features] default = ["std"]; std = []`. I bin userland
  (gzip/gunzip/zcat) usano il default (`std`). Il kernel dipende con
  `default-features = false` (solo `compress`/`decompress`/`crc32`/`GzError`).
- I test esistenti girano sotto feature `std` (host) — invariati.

### Build — tool host `mkbinpack`

Nuovo binario host (Rust std, `tools/mkbinpack/` o membro workspace user host-only):
legge la lista di file da impacchettare (`user-bin/*.wasm` + i `.cwasm` da
`build/`), per ciascuno `gzip_core::compress(bytes, 6)`, emette `bin.bgz` nel
formato sopra. Argomenti: lista path in input + path output.

- **Makefile**: nuovo target `bin.bgz` che dipende da `$(BIN_WASMS)` + i `.cwasm`;
  ricostruito quando cambia un qualunque bin. L'ISO copia **solo** `bin.bgz` in
  `iso_root/` (non più i 67 file in `iso_root/bin/`).
- Riduce anche la dimensione ISO (un blob compresso vs 132MB loose).

### Limine (`limine.conf`)

- **Rimuovi** i ~20 `module_path: /bin/*.wasm` (rescue set di 359) e il fallback
  `shell.cwasm`.
- **Aggiungi** un modulo `bin.bgz` con cmdline `/payload/bin.bgz`: il prefisso
  `/payload/` è già skippato da `modules::mount_all` (resta in HHDM, non copiato
  in tmpfs). Recuperato via `modules::payload("bin.bgz")`.
- **Rescue set** (mini-fallback): `shell.wasm` + un pugno di tool diagnostici
  (`ls cat echo dmesg lspci`) come moduli con cmdline prefisso **`/rescue/`**.
  Nuovo prefisso skippato in `mount_all` (come `/payload/`), tenuti in HHDM,
  montati in `/bin` SOLO se l'unpack di `bin.bgz` fallisce.

NB installer SSD: `mkboot`/`install` copiano `/payload/*` sull'ESP. `bin.bgz`
sotto `/payload/` verrà incluso → il sistema installato su SSD ottiene lo stesso
archivio (unpack identico al boot). Da verificare in fase di piano che l'installer
non lo tratti come artefatto di boot (kernel/EFI/conf); altrimenti usare un
prefisso dedicato `/archive/` ed estendere lo skip + l'accessor.

### Kernel — nuova fase boot `unpack_bin`

Sostituisce la fase `media_bin`. Posizione nel flusso boot:

```
arch → mem → interrupts → pci → devices → fs(tmpfs) → unpack_bin → storage → usb → userland
```

Spostata **subito dopo `fs`**: non dipende da storage né da usb (legge solo il
modulo HHDM). Elimina ogni dipendenza dal medium per `/bin`.

Logica:

1. `modules::payload("bin.bgz")` → `Option<&'static [u8]>`.
2. Se presente: parse header container; per ogni entry
   `gzip_core::decompress(member)` → `vfs` write `/bin/<name>` → drop buffer.
   Log `unpack_bin: unpacked N bins from bin.bgz (M bytes)`.
3. Se assente, header invalido, o **tutte** le entry falliscono → **mini-fallback**:
   monta in `/bin` i moduli `/rescue/*` dalle loro HHDM buffer. Log `bwarn` forte
   (`bin.bgz missing/corrupt → rescue shell`).
4. Membro singolo corrotto (CRC/size mismatch) → `bwarn` + skip quel file,
   continua con gli altri (shell sopravvive se i tool base sono ok).

### Drop

- Fase `media_bin` (ATAPI `/bin` overlay + pump USB-MSC). Rimossa dal flusso.
- Driver USB-MSC (`usb/msc.rs`, `usb/xhci/bulk.rs`, `SectorScale`): **resta nel
  codice ma dormiente** (non più invocato per `/bin`). Rimozione = YAGNI, fuori
  scope (potrebbe servire per storage USB in `/mnt` futuro).

## Error handling (riepilogo)

| Caso                                   | Comportamento                          |
|----------------------------------------|----------------------------------------|
| `bin.bgz` assente / header invalido    | mini-fallback rescue, bwarn forte      |
| Tutte le entry corrotte                | mini-fallback rescue, bwarn forte      |
| Singolo membro corrotto (CRC/size)     | bwarn + skip file, continua            |
| Rescue set anch'esso assente           | panic boot esplicito (non bootabile)   |

## RAM / peak (verifica nel piano)

- `bin.bgz` ~65 MB in HHDM (fuori heap).
- tmpfs finale `/bin` = ~132 MB (heap).
- transitorio = 1 file decompresso alla volta (max `viewer` 61 MB).
- **peak heap ≈ 193 MB** su 384 MB. Margine ok. Da confermare a runtime con `free`.

## Test

- **Host** (`cargo test`): `mkbinpack` roundtrip — pack di un set di file →
  parse + `decompress` per entry → byte identici agli originali; nomi corretti.
- **gzip-core no_std**: i test esistenti restano verdi sotto feature `std`;
  aggiungere un build-check `--no-default-features` (compila per target kernel-like).
- **`make run-test`** (QEMU headless): boot → log `unpack_bin: unpacked N bins`,
  `/bin/ls.wasm` eseguito da tmpfs, `TEST_PASS`.
- **Fallback**: boot con `bin.bgz` volutamente corrotto → log rescue, prompt shell
  raggiungibile, `dmesg` ispezionabile.
- **HW reale**: boot da chiavetta USB → a prompt → **stacca USB** → lancia
  `ls`, `cat`, una GUI (`viewer`) → tutto da RAM.

## File toccati (previsione)

- `user/gzip-core/{Cargo.toml,src/lib.rs,src/format.rs,src/crc32.rs,src/cli.rs}` (no_std + feature std)
- `tools/mkbinpack/` (nuovo tool host)
- `kernel/src/boot/phases/unpack_bin.rs` (nuovo), `kernel/src/boot/phases/mod.rs`, `kernel/src/boot/mod.rs`
- `kernel/src/modules.rs` (skip prefisso `/rescue/`, accessor rescue)
- `kernel/Cargo.toml` (dep gzip-core no_std)
- `kernel/src/boot/phases/media_bin.rs` (rimosso dal flusso)
- `limine.conf`
- `Makefile` (target bin.bgz, ISO ship solo bin.bgz)
- `docs/api/` — **non toccato** (nessuna host fn app-facing nuova)
