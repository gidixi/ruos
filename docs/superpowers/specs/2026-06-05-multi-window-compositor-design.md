# Design: desktop multi-finestra a processi separati (compositor kernel-side)

**Data:** 2026-06-05
**Stato:** brainstorming — design approvato sui bivi architetturali; in attesa di review della spec.

## 1. Visione

Un desktop dove ogni app è un **processo wasm separato** (proprio `.cwasm`, propria
istanza, propria memoria → isolamento sandbox per app), ognuna disegna nella **sua
finestra**, e un **compositor nel kernel** possiede il framebuffer reale: compone
le finestre (z-order, decorazioni, damage) e instrada l'input alla finestra a
fuoco. Modello "Wayland-like", ma con il compositor nel kernel (Rust no_std).

Obiettivo finale: lanciare app (terminale, editor, system-info, ecc.) ognuna in
una finestra, isolata, con i multi-core usati per il lavoro pesante.

Questo è un progetto **grande** → si affronta a **sotto-progetti**, ognuno con il
suo spec→piano→build, partendo da un **GATE** che de-rischia l'incognita maggiore
(come il gate del Component Model).

## 2. Decisioni architetturali (dai bivi del brainstorming)

- **Modello app = processi separati** (NON app in-process dentro un'unica GUI).
- **Compositor = kernel-side** (Rust no_std nel kernel): possiede il framebuffer,
  alloca/gestisce le finestre, compone, instrada l'input. Le app sono "client".
- **Concorrenza = cooperativa, reactor, sul BSP**; **multi-CPU per il LAVORO**,
  non per app. Le app NON girano su core dedicati (vedi §6).

## 3. Architettura

### 3.1 App = "reactor" (il kernel guida il loop)
Oggi l'app GUI ha `fn main()` con `loop { gui.frame() }` che possiede la CPU.
Impossibile con più app + senza fiber (wasmtime no_std AOT non ha fiber per
sospendere un loop a metà). Quindi **inversione di controllo**:
- L'app **esporta `frame()`** (un singolo frame, poi ritorna) e mantiene lo stato
  nella sua linear memory tra le chiamate. Niente loop interno.
- Il **kernel** tiene vivi `Store`+`Instance` di ogni app e chiama `frame()` di
  ognuna a turno (round-robin), poi il compositor compone. Cooperativo, ogni
  `frame()` è una chiamata run-to-completion che ritorna → niente fiber.
- `Gui::frame` di gui-core **è già** una funzione "un frame" → il loop del `main`
  si sposta nel kernel. Cambio piccolo lato app (da `main`-con-loop a reactor).

### 3.2 Surface = buffer dell'app
Ogni app disegna nel **suo** buffer (nella sua linear memory). WIT
`ruos:gui/surface`:
```wit
resource surface {
  configure: func(width: u32, height: u32);
  commit: func(pixels: list<u8>);   // "la mia finestra è questi pixel"
}
create-surface: func() -> surface;
```
`commit(pixels)` → il kernel **legge** i byte (`mem::read`, come `blit`) nel buffer
della finestra corrispondente. Il kernel non alloca i pixel; l'app possiede il suo
buffer. (Per il GATE si può semplificare a una `surface` per app, niente resource
handle multipli — vedi §5.)

### 3.3 Compositor (kernel, Rust no_std)
Tiene una lista di **finestre**: `{ surface_buf, rect (x,y,w,h), z_order, focused }`.
Ogni giro del loop:
1. chiama `frame()` di ogni app → ognuna `commit`a la sua surface nel suo buffer;
2. il compositor **compone**: per ogni finestra in ordine z, blitta il suo buffer
   nella sua `rect` del framebuffer (riusa il blit fast-path + dirty-rect già
   esistenti) + disegna decorazioni (title bar, X) in Rust;
3. ricompone il cursore sopra (già fatto dal kernel).
La composizione è **lavoro pure-CPU** → parallelizzabile sugli AP (§6).

### 3.4 Input routing (kernel)
Oggi l'unica app prende tutto l'input. Il compositor:
- decide la finestra a **fuoco** (click dentro / sul title bar);
- traduce le coordinate mouse in coordinate-finestra;
- mette gli eventi **solo** nella coda della finestra a fuoco;
- l'app a fuoco riceve quegli eventi al suo prossimo `poll-event`.

### 3.5 Riuso di ciò che esiste
- Blit fast-path + dirty-rect (CHANGELOG 268/269) per la composizione.
- Lo strato WIT/wit-bindgen (CHANGELOG 275): `surface` sono altre host fn tipizzate.
- Il **compute-pool SMP** (smp-progress, Fase 2) per il compositing parallelo.
- Il cursore software kernel (CHANGELOG 266/270).

## 4. Decomposizione di B (sotto-progetti, in ordine)

1. **GATE — multi-istanza + surface** (§5): 2 app reactor, 2 surface, compositing
   minimale affiancato, entrambe si aggiornano. ← PRIMO.
2. **Input + focus**: routing dell'input alla finestra a fuoco.
3. **Window manager**: posizione/drag/resize/z-order/decorazioni.
4. **Compositing parallelo (SMP)**: comporre regioni/finestre sugli AP.
5. **Launcher/lifecycle**: spawn di un'app come processo + piazzamento + cleanup.

Ognuno: spec→piano→build a sé.

## 5. Il GATE (primo sotto-progetto — focus del primo piano)

**Prova:** il kernel tiene **2 istanze wasm reactor persistenti**, chiama il loro
`frame()` esportato a turno, ognuna disegna nella sua surface (buffer nella sua
memoria) e la `commit`a, e un **compositor minimale** le blitta **affiancate** nel
framebuffer; **entrambe si aggiornano** (es. ognuna mostra un contatore/colore che
cambia ogni frame).

**Niente ancora:** focus, routing input, decorazioni, drag, launcher, SMP. Solo:
multi-istanza persistente + `frame()` round-robin + commit/read surface + compose.

**Incognita decisiva che verifica:** *il kernel può tenere N istanze wasm
persistenti e chiamare un export `frame()` a turno in cooperativo?* (Oggi gira
**una** GUI wasm in una chiamata bloccante; il reactor + multi-istanza è il punto
nuovo da provare.) Inoltre: il budget memoria (N istanze + N buffer surface).

**App del gate:** NON il desktop egui da 10 MB — due **mini-app reactor** (piccola
linear memory, surface piccola, riempi-colore + contatore) come il guest del
bring-up ma reactor + surface. Tiene il gate economico e isola il meccanismo.

## 6. Multi-CPU (decisione: lavoro parallelo, non app-per-core)

- Le app restano **cooperative sul BSP** (reactor round-robin). NON un core per
  app (eviterebbe wasmtime multi-core-safe + lock pervasivi su tutto lo stato
  kernel — incognita seria, guadagno dubbio per GUI I/O-bound).
- I **multi-core** servono il **lavoro pesante**: il **compositing/raster** (job
  pure-CPU) si spalma sugli AP (sotto-progetto 4) via il compute-pool Fase-2; e
  un'app può **offloadare** un calcolo pesante agli AP (come smptest).
- Il compute-pool esiste già (APs eseguono job kernel pure-CPU in parallelo, BSP
  executor intatto) → fit naturale.

## 7. Cosa NON cambia
- Scheduling delle app = cooperativo single-CPU (BSP). Il design del progetto resta.
- Shell + i ~50 tool wasip1 su `run_cwasm` (path separato).
- L'engine wasmtime + le shim `platform.rs` (mmap/TLS) restano single-core (le app
  girano sul BSP).

## 8. Rischi / incognite (da verificare nel gate, prima di committare)
- **Multi-istanza reactor**: il kernel deve tenere ≥2 `Store`/`Instance` vivi e
  chiamare un export `frame()` a turno. Da provare (è il gate). Possibile blocco:
  l'attuale launch GUI assume un'unica istanza che possiede la CPU.
- **Budget memoria**: N istanze (linear memory ciascuna) + N buffer surface (una
  finestra 1280×800 = 4 MB). Per poche app va; va dimensionato l'heap. Il codice
  AOT (`Module`) è **condiviso** tra le istanze (una deserializzazione, N istanze).
- **Restructure reactor del desktop egui**: spostare il loop `main` nel kernel +
  esportare `frame()` mantenendo lo stato in uno static. Da verificare che gira
  identico (no garble) attraverso il nuovo modello.
- **Cooperatività**: un'app che non ritorna da `frame()` blocca tutto (come oggi
  il gui possiede la CPU). Accettato (cooperativo by design); mitigazione futura =
  fuel/epoch limit per `frame()`.

## 9. Testing
- GATE: boot-check / screendump che mostra **2 finestre affiancate, entrambe che
  si aggiornano** (QEMU+KVM, come le verifiche garble/cursor/poweroff).
- Per-sotto-progetto: screendump della feature (focus → solo la finestra attiva
  reagisce all'input; drag → si muove; ecc.).
- Riuso del pattern QMP screendump + `input-send-event` già usato.

## 10. Aperti (rimandati)
- Decorazioni avanzate, tiling, animazioni finestre.
- Persistenza layout, multi-monitor.
- App-per-core (Opzione 2) — ricerca a sé, non in scope.
