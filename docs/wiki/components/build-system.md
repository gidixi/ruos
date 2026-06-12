# Build System e Precompilazione AOT

> **Stato:** bozza
> **Aggiornato:** 2026-06-11
> **Fonti:** `Makefile`, `tools/wt-precompile/`, `boot/limine.cfg`

## Cos'è

La toolchain e il build system di ruOS sono progettati per orchestrare progetti Rust indipendenti: un kernel puro `no_std`, una varietà di applicazioni utente e componenti Wasm, e infine pacchettizzare tutto in un'immagine ISO bootabile via **Limine**.

Il punto d'accesso principale è il `Makefile` presente nella root del repository. Consigliamo di invocare i comandi da un ambiente Linux/WSL.

## Flusso di Compilazione Standard

Eseguendo `make all` o `make iso`, avvengono le seguenti fasi:

1. **Kernel Build:** Compilazione di `kernel/` tramite Cargo nel target `x86_64-unknown-none`. Produce un binario statico (es. `ruos.elf`).
2. **Userland Build:** Compilazione dei tool in `user/` verso target `wasm32-wasip1`.
3. **Precompilazione AOT:** Strumenti e App ad alte prestazioni (o moduli Component Model) vengono precompilati usando il tool host-side `wt-precompile`.
4. **Pacchettizzazione BGZ:** Tutti gli eseguibili utente (`.wasm` e `.cwasm`) e le configurazioni vengono inclusi in un archivio speciale denominato `bin.bgz` (una sorta di Initramfs minimale in memoria).
5. **Generazione ISO:** Il binario del kernel, il `bin.bgz` e la configurazione del bootloader vengono impacchettati dal tool Limine in un file `ruos.iso`.

## Precompilazione AOT (`.cwasm`)

Poiché Wasmtime è incluso in ruOS in versione `no_std` priva del backend di compilazione Cranelift (per risparmiare memoria e ridurre i tempi di avvio del kernel), i moduli Wasm ad alte prestazioni e i componenti non possono essere compilati dal formato testo/byte `.wasm` al codice macchina in fase di avvio.

Questo problema è risolto da `tools/wt-precompile`:
1. Gira sull'Host (es. WSL) durante il build.
2. Legge un file `.wasm`.
3. Usa la libreria standard di Wasmtime per eseguire l'ottimizzazione AOT (Ahead-of-Time).
4. Emette un file `.cwasm` specifico per l'architettura x86_64, pronto ad essere caricato in memoria ed eseguito direttamente, azzerando il costo di compilazione a runtime (JIT).

## Varianti di Build e Testing

Il Makefile include alcuni comandi specifici per il testing:

- `make qemu`: Costruisce la ISO standard e lancia QEMU con supporto KVM (virtualizzazione hardware) e networking utente.
- `make test-boot`: Utilizzato in CI o per smoke-test, genera una ISO separata (`ruos-test.iso`) che include moduli o comportamenti specifici per autotest, senza sporcare l'ISO principale di release.
- `make clean`: Pulisce i file oggetto e distrugge la cartella `build/`.

L'approccio modulare permette di iterare rapidamente sul kernel ricaricando l'ISO senza dover ricompilare tool Wasm invariati, e viceversa.
