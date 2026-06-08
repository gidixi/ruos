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
- [Panoramica](architecture/overview.md) — *stub*

### Componenti
- [Compositor / Window Manager](components/compositor.md) — la GUI kernel-side, multi-finestra

### Sottosistemi (da scrivere)
- Boot a fasi — *TODO*
- Runtime WASM (wasmi + Wasmtime AOT) — *TODO*
- VFS / storage — *TODO*
- Input (PS/2 + USB HID) — *TODO*
- Networking + SSH — *TODO*
- SMP / executor async — *TODO*

## Stato della wiki

Appena nata. Coperto finora: il compositor. Il resto è elencato sopra come TODO —
si riempie un componente alla volta, ognuno seguendo [STYLE.md](STYLE.md).
