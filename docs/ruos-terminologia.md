# Specifiche Architetturali: I Paradigmi Inediti di ruOS

Questo documento definisce formalmente la terminologia per i concetti architetturali originali introdotti dal design di ruOS. Poiché il sistema si discosta in modo radicale dall'architettura classica, questi termini identificano le tecnologie e i meccanismi che sostituiscono i costrutti storici dei sistemi operativi tradizionali.

## 1. Ring-0 WASM Sandboxing
* **Cosa sostituisce:** L'isolamento hardware basato sul Ring 3 della CPU e sulle tabelle di paginazione separate per processo (per-process page tables).
* **Definizione e Implementazione:**
In ruOS non esiste alcuna separazione dei privilegi a livello di CPU. Il kernel, i runtime e le applicazioni utente (sia gli strumenti CLI che le app GUI) condividono un unico spazio di indirizzamento e vengono eseguiti al massimo livello di privilegio, il Ring 0. L'isolamento non è garantito dall'hardware, ma in via esclusiva dal runtime WebAssembly (l'interprete `wasmi` per i tool `.wasm` e il compilatore AOT `Wasmtime` per le app `.cwasm`). Il limite del raggio di azione ("blast-radius") di un'applicazione è confinato unicamente alla sua memoria lineare gestita dal runtime.

## 2. Direct Closure Binding
* **Cosa sostituisce:** Le chiamate di sistema hardware (le interfacce basate sulle istruzioni `SYSCALL` e `SYSRET`).
* **Definizione e Implementazione:**
Le applicazioni WASM non eseguono istruzioni privilegiate della CPU; dichiarano invece delle "importazioni" (es. WASI Preview 1 o le API dei moduli host come `ruos`, `wm`, `sys` o `ruos_gfx`). Durante l'istanziazione, il Linker del kernel associa direttamente queste firme a delle "closure" Rust native che operano in Ring 0. Questo disaccoppiamento avviene sia in forma fortemente tipizzata (tramite Component Model e file `.wit` per i tipi complessi), sia in forma "raw" tramite la macro `func_wrap` per garantire massime prestazioni grafiche.

## 3. Instruction Fuel Metering
* **Cosa sostituisce:** Lo Scheduler Preventivo (che storicamente interrompe i processi in modo coercitivo in base allo scorrere del tempo).
* **Definizione e Implementazione:**
La gestione della concorrenza in ruOS è cooperativa, single-core e asincrona, guidata dal timer interrupt (LAPIC a 100 Hz). Essendo sprovvisto di preemption, per impedire che un programma malevolo saturi la CPU all'infinito il runtime implementa il "Fuel Metering". A ogni frammento di esecuzione WASM viene assegnato un budget massimo di 2.000.000.000 (due miliardi) di istruzioni. Se un loop computazionale esaurisce il carburante senza mai cedere il controllo, il task viene terminato forzatamente con l'exit code 137. I task legati all'I/O, invece, ricaricano il budget a ogni chiamata verso il kernel.

## 4. Audited Linear Offsets
* **Cosa sostituisce:** I puntatori di memoria virtuale dello spazio utente e le complesse routine hardware per transitare la memoria, come le logiche `copy_from_user` di Linux.
* **Definizione e Implementazione:**
Quando un'applicazione guest WASM passa un puntatore al kernel (ad esempio per eseguire un `wm.commit(ptr, len, w, h)`), i parametri `ptr` non sono veri indirizzi RAM, ma semplici offset (scostamenti) relativi alla propria memoria lineare isolata. L'ispezione della memoria avviene unicamente tramite l'*audited guest-memory accessor* (`kernel/src/wasm/host/mem.rs::check_bounds`), l'unica funzione che valuta l'offset e i limiti per prevenire letture fuori dal buffer. Tutta l'architettura garantisce l'assenza di operazioni "raw" su memoria non validata.

## 5. Process-Isolated Kernel Compositor
* **Cosa sostituisce:** L'architettura client-server dei display manager in spazio utente (come X11, Wayland o il Desktop Window Manager).
* **Definizione e Implementazione:**
ruOS implementa la gestione delle finestre e il Compositor direttamente all'interno del kernel, ma mantenendo l'isolamento multi-finestra. Ogni app GUI attiva è una istanza WASM separata e ignara delle altre. L'app renderizza tramite CPU (`tiny-skia`) la propria superficie locale in formato RGBA8888 e la notifica al kernel. Il kernel esegue nativamente il binding dell'input, decora i contorni con i controlli della finestra (title-bar, drag, close) e, sfruttando i processori applicativi (Application Processors), parallelizza il calcolo per "spalmare" e comporre (compositing) lo schermo finale. Le finestre comunicano con l'hardware e con gli input dell'utente esclusivamente in maniera "cieca" tramite gli offset verificati e la ricezione asincrona nella coda eventi (poll_event).
