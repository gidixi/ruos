# Wiki ruOS

Documentazione di riferimento del progetto **ruOS** — OS WASM-first x86-64 in
Rust `no_std`, bootloader Limine, app `.wasm` (WASI), GUI egui, accesso SSH.

Questa wiki spiega **come il sistema è fatto e perché**: architettura, componenti,
contratti tra i pezzi. Non è un changelog (quello sta in [`CHANGELOG/`](../../CHANGELOG/))
né una spec di design (quelle stanno in [`docs/superpowers/specs/`](../superpowers/)).
La wiki è la vista *stabile e navigabile*; spec e changelog sono la traccia storica.

## Come leggere

- Parti da [Architettura](architecture/overview.md) per la vista d'insieme.
- Scendi nei [Componenti](#componenti) per i dettagli di un sottosistema.
- Ogni pagina dichiara in testa il suo **stato** e le **fonti** (file sorgente che
  documenta), così sai quanto fidarti e dove guardare il codice vero.

## Come scrivere

**Prima di aggiungere o modificare una pagina, leggi [STYLE.md](STYLE.md).**
Definisce intestazione obbligatoria, naming dei file, regole di link al codice,
tono e lingua. Pagine che non seguono lo stile vanno riallineate.

## Indice

### Architettura
- [Panoramica](architecture/overview.md) — layer cake, boot, runtime, SMP, source map
- [Modello di Sicurezza](architecture/security.md) — single-address-space, capability paths, isolation
- [Ecosistema Userland](architecture/userland.md) — tool CLI `wasmi`, app AOT, ABI e limitazioni

### Componenti
- [Boot a fasi](components/boot-phases.md) — le 10 fasi di init, da GDT a executor
- [Runtime WASM](components/wasm-runtime.md) — wasmi + Wasmtime AOT, fibers, fuel/epoch, host ABI
- [Gestione della Memoria](components/memory.md) — heap, frame allocator, demand paging
- [Component Model e WIT](components/component-model.md) — Wasmtime components, dynamic linking
- [Build System e AOT](components/build-system.md) — Makefile, wt-precompile, ISO gen
- [Hardware Abstraction & Driver](components/drivers.md) — bus PCIe, DMA, device asincroni
- [VFS / Storage](components/vfs-storage.md) — tmpfs, FAT32, AHCI, GPT, disk authoring
- [Input](components/input.md) — PS/2 + USB HID keyboard/mouse, coda condivisa
- [Networking + SSH](components/networking-ssh.md) — smoltcp, NIC, DHCP, TCP, SSH, Wi-Fi
- [SMP / Executor async](components/smp-executor.md) — embassy, compute pool, CPU accounting
- [Compositor / Window Manager](components/compositor.md) — la GUI kernel-side, multi-finestra

## Stato della wiki

Copre tutti i sottosistemi principali (7 componenti + panoramica architettura).
Ogni pagina è in stato `bozza` — rispecchia il codice al 2026-06-10 ma non è
stata revisionata a fondo. Si rifinisce un componente alla volta, seguendo
[STYLE.md](STYLE.md).
