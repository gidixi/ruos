# Gestione della Memoria

> **Stato:** bozza
> **Aggiornato:** 2026-06-11
> **Fonti:** `kernel/src/memory/`

## Cos'è

ruOS è un sistema operativo **single-address-space**. Non esiste memoria virtuale separata per-processo, né context switch della MMU (Memory Management Unit) ad ogni cambio di task. Tutta la memoria (kernel, driver e moduli WebAssembly) risiede nello stesso spazio di indirizzamento Ring 0. 

L'isolamento non è garantito a livello di pagina per ogni "processo", ma è garantito dalla sandbox del runtime WASM (wasmi o Wasmtime) e dal compilatore.

## Dove vive

| File / cartella | Ruolo |
|-----------------|-------|
| `kernel/src/memory/heap.rs` | Allocatore globale (`talc`) e inizializzazione heap |
| `kernel/src/memory/alloc_magazine.rs` | Allocatore SMP lock-free per-core |
| `kernel/src/memory/frames.rs` | Allocatore di frame fisici (PMM) |
| `kernel/src/memory/mapper.rs` | Demand paging e gestione tabelle (VMM) |

## Modello: HHDM e Single-Address-Space

Il bootloader Limine fornisce al kernel una mappatura **HHDM** (Higher Half Direct Mapping). Questo significa che tutta la memoria fisica disponibile è mappata linearmente in un offset costante negli indirizzi virtuali alti.

Il kernel usa questa mappatura per accedere direttamente a qualsiasi frame fisico senza dover modificare continuamente le page table.

## Allocatore Globale (Heap)

L'allocatore globale di ruOS è **Talc** (un allocatore no_std veloce e compatto).

Per supportare la scalabilità SMP, l'allocatore puro (protetto da uno spinlock globale che causava contesa tra i core) è stato avvolto da un **Magazine Allocator per-core** (`alloc_magazine.rs`). Questo meccanismo lock-free smista le piccole allocazioni direttamente dai buffer locali della singola CPU, aggirando il collo di bottiglia del lock globale e delegandolo solo per allocazioni di grande dimensione.

La RAM dedicata all'heap è allocata all'avvio scansionando la memory map di Limine alla ricerca della più grande regione `USABLE`. Attualmente il limite è impostato a ~384 MiB per ospitare le app GUI e il compositor.

## Demand Paging e Isolamento Wasmtime

Mentre l'interprete `wasmi` si affida a controlli software per l'accesso alla memoria (`check_bounds`), il runtime AOT **Wasmtime** fa uso dell'hardware MMU:

1. Quando un'app `.cwasm` viene caricata, il runtime Wasmtime **riserva uno spazio di indirizzamento virtuale** (VA) esteso, sebbene non allochi subito memoria fisica (Demand Paging).
2. Viene posizionata una **Guard Page** alla fine della memoria lineare del guest.
3. Se un'app Wasmtime tenta un accesso fuori dai limiti, colpisce la guard page non mappata, sollevando un Page Fault che il kernel cattura per trappare e uccidere il modulo isolato, proteggendo il resto del sistema.

Questo approccio offre sicurezza architetturale pur mantenendo i benefici prestazionali del single-address-space.
