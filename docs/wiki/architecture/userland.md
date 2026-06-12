# Ecosistema Userland e Applicazioni

> **Stato:** bozza
> **Aggiornato:** 2026-06-11
> **Fonti:** `user/`, `user-bin/`, `kernel/src/wasm/host/`

## Cos'è

In ruOS, tutto ciò che non è compilato all'interno del kernel Ring 0 è considerato "Userland". E sebbene tutto giri fisicamente in Ring 0 e in un single-address-space, la divisione logica e di sicurezza è netta.

Tutte le applicazioni userland sono moduli WebAssembly. ruOS supporta due grandi categorie di programmi: i tool CLI interpretati e le applicazioni grafiche/complesse precompilate.

## I due volti dello Userland

### 1. I tool CLI (`.wasm`)
Applicazioni classiche a riga di comando che girano sull'interprete `wasmi`.
- **Target Rust:** `wasm32-wasip1`
- **Libreria standard (`std`):** Supportata tramite il subset WASI Preview 1 esposto dall'host.
- **Vantaggi:** Codice portatile, accesso ad allocazioni `std`, comodo per piccoli utility (come `ls`, `cat`, `shell`).
- **Svantaggi:** Molto più lento (~100x dell'esecuzione nativa) a causa dell'interprete. Non può utilizzare librerie pesanti in real-time.
- **Path predefinito:** Sono pacchettizzati nell'archivio `bin.bgz` come moduli standard.

### 2. Le App e GUI precompilate (`.cwasm`)
Applicazioni grafiche, window manager e componenti ad alte prestazioni che girano tramite il motore `Wasmtime AOT`.
- **Target Rust:** Tipicamente `wasm32-unknown-unknown` (con il WebAssembly Component Model abilitato).
- **Libreria standard (`std`):** In genere `no_std`, oppure parzialmente simulata dalle interfacce Component (`wit`). L'host (Wasmtime) non espone tutto lo stack POSIX ma set specifici di API (es. `ruos_gfx`, `wm`, interfacce WIT).
- **Vantaggi:** Prestazioni near-native senza bisogno del compilatore JIT a runtime. Molto leggeri se usano i Componenti Condivisi.
- **Path predefinito:** In genere compilate host-side da `tools/wt-precompile` e lette in binario o passate via VFS. 

## Sviluppare un tool CLI (wasmi)

I sorgenti si trovano nella directory `user/`. Per aggiungere un nuovo comando:
1. Crea una cartella in `user/mio-tool`.
2. Scrivi un classico binario Rust per il target `wasm32-wasip1`. Puoi usare `std::fs`, `std::env`, ecc.
3. Le host function in `kernel/src/wasm/host/` mapperanno la tua chiamata WASI in chiamate sicure al kernel (es. accessi VFS controllati per le capability).
4. Nel Makefile, assicurati che la cartella sia inclusa nel processo di build. Verrà compilato in `.wasm` e inserito nel `bin.bgz`.

## L'astrazione POSIX limitata

ruOS non è Linux. Anche con WASI, ci sono pesanti deviazioni:
- **Nessun thread:** Il kernel non implementa i pthreads. Non è possibile usare `std::thread::spawn`. Le app Wasm sono puramente single-thread (fiber-based).
- **Preemption cooperativa e Fuel:** Se scrivi un loop `while true {}` in un'app `wasmi` senza mai invocare una syscall (ad esempio `println!`), consumerai tutto il *fuel* e il kernel ucciderà l'app forzatamente (exit code 137). L'I/O cede cooperativamente la CPU.
- **Capability Paths:** Non puoi fare `std::fs::read("../file")`. Se il processo ha i permessi solo su una cartella virtuale, i percorsi devono essere relativi e `..` oltre la radice genera permessi negati.

## Sviluppare un'App GUI (egui)

Lo sviluppo GUI avviene tramite un repository o un modulo esterno (`ruos-desktop`), usando l'ecosistema `egui`.

1. L'app usa interfacce ad-hoc o il componente Wasmtime per renderizzarsi. 
2. Al posto delle syscall, il kernel Wasmtime mappa un framebuffer RAW (`ruos_gfx`) o inoltra i comandi grafici al **Compositor Kernel-Side**.
3. Il compositor, eseguito su pool SMP, disegna la finestra unendo i buffer off-screen.

L'approccio preferito per i nuovi design è basato sul **WebAssembly Component Model**, esternalizzando la logica di render a provider condivisi.
