# Modello di Sicurezza e Hardening

> **Stato:** bozza
> **Aggiornato:** 2026-06-11
> **Fonti:** `kernel/src/wasm/host/`, `docs/superpowers/specs/`

## Cos'è

ruOS implementa un modello di sicurezza atipico rispetto a Linux o Windows. Poiché le performance estreme e la riduzione del context-switch sono obiettivi primari, ruOS rinuncia alla classica separazione Ring 0 / Ring 3 e allo spazio di indirizzamento per-processo.

Tutto il codice (sia il kernel che lo userland) gira in **Ring 0** (kernel mode) in un **Single-Address Space**.

## Il Sandbox Boundary: WebAssembly

Se un processo gira in Ring 0 e non usa le Page Table per l'isolamento, cosa impedisce a un'app malevola di leggere le password o fare crashare il sistema operativo?

L'unico confine di sicurezza è **WebAssembly**. Il codice sorgente dell'utente o di terze parti non viene mai eseguito nativamente: viene tradotto in byte-code Wasm.
1. Il codice utente non può emettere istruzioni macchina (niente `mov eax, cr3`, niente interrupt diretti).
2. L'accesso alla memoria è strettamente limitato all'interno della *linear memory* allocata per l'istanza Wasm.
   - Per `wasmi`, ogni accesso è validato a runtime con un bounds-check software estremamente rigoroso.
   - Per `Wasmtime`, ogni accesso è validato grazie ad un mix di bound-checks compilati AOT e guard-pages hardware gestite dal runtime prima di invocare il codice.
3. Il codice non può fuggire dal suo ambiente. Qualsiasi azione "utile" richiede una Host Call.

## Capability-Based Security nel VFS

L'accesso ai file non è globale. Ogni applicazione riceve "pre-open" file descriptors al boot (tramite `wasi`). 

La sicurezza del filesystem in ruOS è strettamente path-based e **capability-scoped**. Il kernel impedisce *path traversal*:
- Un'applicazione con accesso a `/mnt/disk` non può usare percorsi come `../../etc/shadow`.
- Il risolutore di path nelle Host Functions rifiuta di risalire la radice (root) virtuale assegnata al processo.

## Mitigazioni Future: Hardware Hardening

Essendo un sistema "Software-isolated", ruOS è suscettibile a vulnerabilità nei runtime o bug nel codice *unsafe* del kernel.

Per irrobustire il sistema, specialmente in vista del multi-tenant, le spec future (es. `2026-06-10-multi-tenant-hardening-design.md`) prevedono di integrare isolamento assistito dall'hardware pur mantenendo lo spazio di indirizzamento singolo:

- **Intel MPK (PKU):** Memory Protection Keys for Userspace. L'idea è assegnare una chiave PKU al kernel e chiavi diverse alle app. Anche se un bug in Wasmtime permettesse a un'app di generare un puntatore fuori dai suoi limiti verso il kernel, la CPU genererebbe un'eccezione perché il tag PKU dell'app non corrisponde a quello della pagina puntata. Questo aggiunge una "porta di ferro" hardware attorno alla sandbox software.
- **Epoch Watchdog:** Già integrato, questo meccanismo garantisce che un'applicazione che esegue un attacco Denial of Service (es. loop infinito puro) venga comunque trappata dal kernel, evitando che il sistema intero resti bloccato a causa della mancanza di preemption hardware preemptiva.
