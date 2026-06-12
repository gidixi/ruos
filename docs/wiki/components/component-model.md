# WebAssembly Component Model e WIT

> **Stato:** bozza
> **Aggiornato:** 2026-06-11
> **Fonti:** `wit/`, `kernel/src/wasm/wt/component.rs`, `tools/wt-tui/`

## Cos'è

ruOS utilizza il **WebAssembly Component Model** per fornire funzionalità avanzate ad alte prestazioni alle applicazioni *userland*, garantendo modularità, sicurezza ed evitando la duplicazione di librerie pesanti.

Anziché compilare staticamente grandi dipendenze (es. GUI o TUI come `ratatui` e `egui`) in ogni singola applicazione `.wasm`, le app le "importano" sotto forma di interfacce astratte. Il kernel si occupa poi di fornire un **Componente Provider Condiviso** (es. `tui.cwasm`) a runtime.

## Dove vive

| File / cartella | Ruolo |
|-----------------|-------|
| `wit/` | Definizioni IDL delle interfacce (`ruos-tui.wit`, `ruos-gui.wit`) |
| `kernel/src/wasm/wt/component.rs` | Linker dinamico e gestore dei Componenti in Wasmtime |
| `tools/wt-tui/` | Implementazione del Componente Provider condiviso per interfacce TUI |
| `user/rtop/` | Esempio di applicazione (Consumer) compilata via Component Model |

## Architettura dei Modelli: WIT

Il contratto tra il Guest (App) e l'Host (Kernel + Provider) è definito dal linguaggio **WIT** (WebAssembly Interface Type). 

I file `.wit` definiscono *Mondi* (Worlds) ed *Interfacce* esportate/importate. 

Esempio da `ruos-tui.wit`:
```wit
package ruos:tui;

interface canvas {
    record rect { x: u16, y: u16, width: u16, height: u16 }
    draw-text: func(bounds: rect, text: string);
    render: func();
}

world app {
    import canvas;
}

world provider {
    export canvas;
}
```

Questo definisce due mondi: un'applicazione che *importa* strumenti di disegno e un provider che li *esporta*.

## Il Linking Dinamico in Wasmtime

La logica di base risiede in `kernel/src/wasm/wt/component.rs`. Quando un file `.cwasm` di tipo Componente richiede esecuzione:

1. **Creazione dello Store:** Wasmtime prepara uno `Store` per l'istanza.
2. **Caricamento del Provider:** Il kernel istanzia il componente provider (es. `tui.cwasm`), che è una libreria reattiva compilata in `.cwasm`.
3. **Linker Shims:** Poiché `no_std` Wasmtime non supporta il linking automatico nativo tra due istanze dinamiche, il kernel registra degli *shim* manuali. Una funzione host importata dall'app inoltra in modo trasparente la chiamata alla funzione esportata dal provider.
4. **Boot dell'App:** L'app principale (`rtop.cwasm`) viene istanziata ed eseguita. L'app chiama `draw-text`, lo shim cattura la chiamata e la invia al modulo `tui`, che possiede le dipendenze native.

## Implementare un nuovo Componente

Per aggiungere un nuovo componente di sistema in ruOS:

1. **Definire l'interfaccia WIT:** Aggiungi un file `.wit` in `wit/` definendo strutture e funzioni.
2. **Creare il Provider:** Scrivi un crate Rust in `tools/` usando la macro `bindgen!` generata dal file WIT. Esponi il trait dell'interfaccia.
3. **Modificare l'App:** Usa `cargo component` (o `wit-bindgen`) nell'applicazione in `user/` per importare il mondo.
4. **Aggiornare il Kernel:** Nel file `component.rs` del kernel Wasmtime, implementa il routing (shim) che aggancia le chiamate in uscita dell'app a quelle in entrata del provider.
5. **Precompilazione AOT:** Assicurati che il Makefile traduca sia l'app che il provider in `.cwasm` tramite `tools/wt-precompile`.

Grazie a questo design, strumenti come `rtop` passano dall'essere enormi binari monolitici `.wasm` di svariati MB a leggeri componenti `.cwasm` di pochi KB.
