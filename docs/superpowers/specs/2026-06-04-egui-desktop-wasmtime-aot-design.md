# egui desktop su ruos via Wasmtime AOT — design

**Data:** 2026-06-04
**Stato:** spec approvata (design), pre-piano
**Topic:** comando `gui` → desktop egui completo come app WebAssembly AOT

## 1. Obiettivo

Digitando `gui` nella shell ruos parte un **desktop egui** completo — finestre
fluttuanti, widget gallery, finestre native ruos — sul framebuffer. Riusa il
contenuto upstream `egui_demo_lib` (backend-agnostico) con un **backend ruos**
custom. L'app gira a velocità quasi-nativa tramite **Wasmtime no_std in modalità
AOT** (modulo precompilato sul build host), senza portare Cranelift dentro il
kernel.

Il desktop include un **terminale** funzionante: una finestra che esegue i tool
`.wasm` esistenti (shell, ls, ps, nano…) su un **PTY** reale, esattamente come la
console locale e SSH. Le operazioni native del desktop (es. "nuova cartella" da
un file manager) **non** passano per i tool: chiamano direttamente il kernel via
host fn WASI (vedi §6, due canali).

### Non-obiettivi (YAGNI / drop espliciti)

- **Niente window manager OS.** Le "finestre" sono finestre **interne** di egui
  dentro una sola app; non sono finestre del sistema operativo. Niente
  compositor kernel, niente z-order tra app, niente routing input multi-app.
- **Niente JIT on-device.** Cranelift dentro ruos = scartato (no_std best-effort,
  fragile, non incluso nel build no_std ufficiale di Wasmtime, sandbox ring-0
  indebolita). Vedi §11.
- **Niente egui nel kernel.** egui richiede `std`; il kernel è `no_std`. egui
  gira esclusivamente nell'app `wasm32-wasip1` (dove ruos fornisce `std`).
- **Niente migrazione di tutti i tool a Wasmtime.** wasmi resta il runtime per i
  ~54 tool esistenti. Wasmtime è aggiunto **solo** per la GUI (rischio isolato).
- **Niente GPU.** Render = rasterizzatore software (`tiny-skia`) → framebuffer.
- **Niente fibre Wasmtime (stack-switch) in v1.** Blocking cooperativo = modello
  poll-based (vedi §7).

## 2. Contesto e vincoli (stato attuale del codice)

- Runtime wasm attuale: **wasmi** via `wasm::fiber::Fiber::new(bytes).run()`.
  Host fn installate da `wasm::host::install(&mut Linker<RuntimeState>)`,
  namespace: `wasi_snapshot_preview1` (25 fn) + `ruos` (33 fn) distribuite su
  `lifecycle/fd/path/clock/random/sock/proc/term/sysinfo/service/smp`.
- **Nessun host fn grafico.** Nessun `ruos_gfx`.
- Framebuffer: `console::fb::FbInfo { addr, width, height, pitch, bpp, pixel:
  Rgb|Bgr }` + atomici `FB_VIRT/FB_PITCH/FB_BPP`. La console di testo possiede il
  framebuffer (render::flush blitta span dirty).
- **Nessun driver mouse** (no PS/2 IRQ12). Keyboard sì (`keyboard/mod.rs`),
  input PS/2+USB confluiscono in una coda unica.
- App tool sono `wasm32-wasip1` con `std` (la doc garantisce: "unmodified
  wasm32-wasip1 std binary run").
- Toolchain build = WSL Ubuntu, nightly `2026-05-26`, `make iso`.

Fatti esterni che fondano il design (ricerca 2026-06-04):

- **egui richiede `std`** (default, nessuna feature no_std/alloc). → solo lato app.
- **`egui_demo_lib` è backend-agnostico** (dipende solo da `egui`+`egui_extras`,
  non da eframe/wgpu/glow). → riusabile con backend ruos.
- **Wasmtime è ora un crate `no_std`.** In no_std supporta **solo AOT** (modulo
  precompilato altrove); **Cranelift NON è incluso** nel build no_std.
  Requisiti embedder: global allocator, `core`+`alloc`, panic handler, e uno shim
  di piattaforma (`wasmtime-platform.h`-equivalente) per memoria virtuale/
  eseguibile. `signals-based-traps` è **off-by-default** → niente mmap né signal
  handler obbligatori.
- Precedente: Theseus OS ha portato Wasmtime no_std bare-metal (2022); le parti
  dure furono memoria eseguibile, gestione trap, e `bincode 2.0` no_std.

## 3. Architettura

```
BUILD HOST (WSL)
  cargo build -p gui --target wasm32-wasip1        → gui.wasm
  wasmtime compile --target x86_64 gui.wasm -o gui.cwasm   (Cranelift gira QUI)
  gui.cwasm → modulo Limine (/bin/gui.cwasm)

RUOS (no_std, ring 0)
  shell: "gui" → runtime router
      *.cwasm  → Wasmtime engine (AOT, niente Cranelift on-device)
      *.wasm   → wasmi (invariato)
  Wasmtime instance:
      Linker: wasi_p1 (logica host riusata) + ruos + ruos_gfx (nuovo)
      ResourceLimiter + epoch interruption (no fuel di wasmi)
  GUI app loop:
      ruos_gfx_poll_event → egui::RawInput
      ctx.run(input, DemoWindows::ui) → FullOutput
      ctx.tessellate() → Vec<ClippedPrimitive>
      tiny-skia raster (dirty rect, 720p) → buffer RGBA8888
      ruos_gfx_blit(buf, rect)
  kernel gfx service:
      GUI-mode ON → console flush sospeso (flag atomico)
      blit buffer → framebuffer (conversione RGBA→Bgr/Rgb)
      mouse/key IRQ → input queue → coalescing → gfx_poll_event
  app exit → GUI-mode OFF → console ridisegnata; serial log mai interrotto
```

### 3.1 Esecuzione tool e terminale (due runtime, ponte PTY)

Un guest wasm non può eseguire un altro modulo wasm. Quindi i tool NON li lancia
l'app `gui`: li lancia il **kernel**, e l'app ci dialoga via **PTY** (lo stesso
meccanismo di `ssh_spawn.rs`, terzo front-end a un PTY dopo console locale e
SSH).

```
gui app (Wasmtime)
  finestra "terminal" (emulatore vte+egui)
    │ host fn pty_open / proc_spawn / fd_read / fd_write
    ▼
kernel: pty_open → proc_spawn("/bin/shell.wasm", slave)   [tool gira su WASMI]
PTY master ◄── output ANSI ── shell + ls/ps/nano (wasmi fibers)
gui legge master → vte parse → grid celle → render egui
gui scrive tasti → master → line discipline → tool
```

**Due runtime coesistono:** la `gui` gira su Wasmtime AOT; i tool restano su
wasmi. Sono task cooperativi indipendenti sul BSP; il PTY è il ponte kernel-side.
Nessun annidamento di runtime nel guest.

**Due canali per le operazioni (perf):**

1. **Canale nativo** — feature GUI (bottone "nuova cartella", file manager,
   monitor). L'app chiama **direttamente** le host fn WASI/`ruos` già esistenti
   (`path_create_directory`, `fd_readdir`, `proc_stat`…). Nessun tool spawnato →
   velocità kernel-nativa, istantanea.
2. **Canale terminale** — l'utente digita un comando (`mkdir foo`). La shell lo
   esegue spawnando il tool `.wasm` su PTY (overhead = instanziazione wasmi,
   ~ms). Necessario per la semantica shell e per eseguire programmi (nano, ps).

Regola: le funzionalità del desktop usano il canale nativo; lo spawn di tool è
solo per il terminale interattivo e per lanciare programmi.

## 4. Componenti (unità isolate, ognuna con confine chiaro)

| Unità | Posizione | Responsabilità | Dipende da |
|---|---|---|---|
| `wasm/wasmtime_rt` | kernel `no_std` | engine Wasmtime, instance, run loop, epoch | platform shim, host fn |
| platform shim | kernel | exec-mem W^X, guest mem, trap senza signal, dealloc | `memory/paging`, heap |
| runtime router | kernel (`wasm/mod.rs` / exec_queue) | sceglie wasmi vs Wasmtime per estensione | entrambi i runtime |
| `gfx` service | kernel | possiede fb in GUI-mode, `info`/`blit`, suspend/restore console | `console::fb` |
| mouse PS/2 | kernel | driver IRQ12 → eventi coda input | IDT/IOAPIC, input queue |
| input coalescer | kernel | unisce key+mouse → eventi GUI | keyboard, mouse |
| `ruos_gfx` ABI | kernel↔app | `gfx_info`/`gfx_blit`/`gfx_poll_event` | gfx service, `host/mem` |
| `ruos_proc` ABI | kernel↔app | `pty_open`/`proc_spawn`/`proc_poll`/`pty_set_winsize` | pty, exec_queue |
| terminale emulatore | `gui` app (wasm) | vte parse PTY master → grid celle → render egui; tasti → master | `ruos_proc`, vte |
| `gui` app | wasm | egui + egui_demo_lib(ridotto) + tiny-skia + backend ruos | host ABI |
| host build step | Makefile/WSL | `wasmtime compile` pinnato a versione runtime | wasmtime CLI |

Criterio di isolamento: ogni unità ha un'interfaccia stretta. Es. `gfx` espone
solo `info/blit/suspend/restore`; l'app non vede mai i pixel del framebuffer
direttamente (solo `gfx_blit`).

## 5. Host ABI `ruos_gfx` (nuovo namespace)

Tutti gli accessi alla memoria guest passano per
`wasm::host::mem::check_bounds` (regola esistente: un solo accessor auditato).

- `gfx_info(out_ptr: i32) -> errno`
  Scrive in guest una struct `GfxInfo { width:u32, height:u32, stride:u32,
  format:u32 }`. `format` = costante RGBA8888 (canonico app-side); la
  conversione verso il layout fisico (`Bgr`/`Rgb`) è del kernel. La risoluzione
  riportata può essere quella **ridotta** (es. 1280×720) se lo scaling è attivo
  (§9).

- `gfx_blit(buf_ptr: i32, buf_len: i32, x:i32, y:i32, w:i32, h:i32) -> errno`
  Copia un rettangolo RGBA8888 (`w*h*4` byte) dal buffer guest al framebuffer,
  con conversione layout e (se attivo) scaling. Solo la regione dirty.

- `gfx_poll_event(out_ptr: i32, max:i32, timeout_ms:i32) -> count`
  Riempie fino a `max` eventi `GfxEvent` coalescati dalla coda input. Bloccante
  **cooperativo**: se non ci sono eventi, l'app cede l'esecuzione (epoch yield,
  §7) e viene risvegliata da timer/input. `timeout_ms` = wake garantito per
  animazioni egui. `GfxEvent` discriminato:
  `{ kind: 0=key,1=mouse_move,2=mouse_btn,3=resize,4=quit; payload... }`.

- Fase 2 (opzionali, non v1): `gfx_set_cursor`, `gfx_clipboard_get/set`.

### 5.1 Host ABI `ruos_proc` (esecuzione tool + terminale)

Per il canale terminale (§3.1). Le operazioni FS native del desktop usano invece
le host fn WASI già esistenti (`path_*`, `fd_*`) — nessuna nuova ABI.

- `pty_open(out_ptr: i32) -> errno`
  Alloca una coppia PTY. Scrive in guest `{ master_fd:i32, slave_id:i32 }`. Il
  `master_fd` è un FD WASI normale dell'app (read/write standard).

- `proc_spawn(path_ptr, path_len, argv_ptr, env_ptr, slave_id, out_pid_ptr) -> errno`
  Il kernel risolve `/bin/<path>.wasm`, lo esegue **su wasmi** con stdio legato
  al PTY slave (riusa `exec_queue`), registra il pid in `proc`. Ritorna il pid.

- `proc_poll(pid: i32, out_ptr: i32) -> errno`
  Stato del processo: `{ exited:bool, code:i32 }`. Non bloccante; l'app sa quando
  il prompt deve tornare.

- `pty_set_winsize(master_fd, cols:i32, rows:i32) -> errno`
  Comunica la dimensione del terminale (per app che leggono winsize).

I/O del terminale = `fd_read`/`fd_write` WASI sul `master_fd` (cooperativi via
poll + epoch yield, §7). Capability-scoped come ogni path host fn.

## 6. Flusso di un frame

```
LAPIC timer (100 Hz) / mouse IRQ12 / key IRQ1
  → input queue → coalescer
  → app sveglia da gfx_poll_event → costruisce egui::RawInput
  → ctx.run(input, |ctx| DemoWindows::ui(ctx)) → FullOutput (+ repaint flag)
  → ctx.tessellate(pixels_per_point) → Vec<ClippedPrimitive>
  → tiny-skia: per ogni mesh, rasterizza triangoli texturati+alpha nel buffer
     RGBA dirty (font atlas come texture; AA via alpha dei vertici)
  → gfx_blit(buffer, dirty_rect)
  → kernel converte layout + (scaling) + copia nel framebuffer
```

**Repaint on-demand (obbligatorio per perf):** l'app dorme su `gfx_poll_event`.
Ridisegna solo se (a) arriva input, (b) egui ha chiesto `request_repaint`
(animazioni), o (c) scade `timeout_ms`. Idle = costo zero.

### 6.1 Flusso finestra terminale

```
apertura terminale:
  pty_open() → master_fd; proc_spawn("shell", slave) → pid
ad ogni frame (o quando c'è output):
  fd_read(master_fd) non-bloccante → byte ANSI
  vte parse → aggiorna grid celle (char + SGR + cursore)
  egui: disegna la grid (font monospace) nella finestra; request_repaint se nuovi byte
tasti nella finestra focalizzata:
  egui key → byte → fd_write(master_fd) → line discipline → shell/tool
chiusura/uscita comando:
  proc_poll(pid) → exited → prompt o chiudi finestra
```

Output del tool sveglia il repaint (il PTY master diventa leggibile → wake
cooperativo dell'app, come ogni FD). Il terminale è una finestra egui come le
altre: si può spostare/ridimensionare; `pty_set_winsize` segue il resize.

## 7. Concorrenza / modello blocking

Wasmtime deve integrarsi con l'executor embassy cooperativo single-core. Le
chiamate host bloccanti (`gfx_poll_event`, `fd_read` su PTY vuoto) non possono
busy-wait.

**Decisione v1 — poll-based + epoch, niente fibre stack-switch:**

- Le host fn bloccanti ritornano subito un esito "would-block" e l'instance
  viene **interrotta via epoch** (`set_epoch_deadline` / `epoch_yield`),
  restituendo il controllo all'executor. L'executor riprende l'instance al wake
  (timer/input). Lo stato di "in attesa di evento" è tenuto in `RuntimeState`,
  non nello stack nativo del guest.
- Niente `wasmtime::Fiber` (richiede stack-switch con supporto piattaforma
  custom) in v1. Se in futuro serve I/O bloccante più ergonomico, valutare le
  fibre come follow-up.
- Fuel di wasmi → sostituito da **epoch interruption** per il bound anti-runaway.

## 8. Proprietà del framebuffer

- `gfx` introduce un flag atomico `GUI_MODE`. Quando ON:
  - `console::fb::FramebufferConsole::write_str` salta `render::flush`
    (controlla `GUI_MODE`); il testo va comunque a serial + ring buffer dmesg.
  - `tick_cursor` (timer IRQ) salta il disegno del cursore.
- All'avvio di `gui`: `gfx::enter()` setta `GUI_MODE`, opzionale clear fb.
- All'uscita di `gui`: `gfx::leave()` azzera `GUI_MODE` e forza un **repaint
  completo della console** (riemissione della grid → framebuffer) così il
  prompt riappare intatto.
- Single-core + `without_interrupts` durante blit garantiscono che IRQ
  (tick_cursor) non interleavino col disegno GUI.

## 9. Perf

Anche con AOT (codice quasi-nativo), `tiny-skia` rasterizza su CPU senza GPU.
Mitigazioni **non opzionali in v1**:

1. **Repaint on-demand** (§6).
2. **Dirty-rect blit** — solo regioni cambiate (egui fornisce le clip rect).
3. **Risoluzione di rendering ridotta** — default raster a 1280×720 (o
   `pixels_per_point` < nativo) poi scaling nel `gfx_blit`. Parametrico.
4. **Contenuto leggero all'avvio** — poche finestre aperte; `egui_extras`
   senza feature `image`/`svg` (pesanti). `default_fonts` ok.

Target realistico: idle gratis; interazione usabile (punta/clicca/sposta), non
60 fps fluido. Numero esatto da **benchmark** (§ piano) prima di fissare la
risoluzione.

## 10. Testing

- **Mouse PS/2**: unit decode pacchetti 3-byte; integrazione — `smoke.sh`
  asserisce ricezione evento mouse (iniettato via QEMU `mouse_move`).
- **Wasmtime shim**: run di un `hello.cwasm` minimale → asserzione stringa
  successo su seriale (estende `make run-test`).
- **Exec-mem W^X**: unit test paging — pagina mappata RX esegue, RW non esegue.
- **gfx**: blit di un pattern noto → rilettura pixel framebuffer (modello
  `console::fb::self_test`).
- **terminale**: `proc_spawn` di `echo`/`ls` su PTY → l'app legge il master →
  asserisce output atteso; spawn `shell` + comando scriptato → exit code via
  `proc_poll`.
- **gui end-to-end**: boot headless con autostart `gui`, asserisce "frame reso"
  + screenshot QEMU per ispezione manuale.

## 11. Alternative considerate e scartate

- **JIT on-device (Cranelift in ruos)**: no_std best-effort/fragile, non incluso
  nel build no_std ufficiale di Wasmtime; Theseus non completò il live JIT;
  codice JIT gira ring-0 (no ring 3) → sandbox indebolita. **Scartato.**
- **egui nel kernel (ring-0 native)**: egui vuole `std`; kernel `no_std` →
  richiederebbe port enorme/non supportato. **Scartato.**
- **Migrazione totale a Wasmtime**: più lavoro e rischio su tutti i 54 tool per
  zero beneficio sui tool non-GUI. **Scartato** (wasmi resta).
- **wasmi + sole mitigazioni (no Wasmtime)**: più economico ma desktop lento;
  resta come fallback se l'integrazione Wasmtime si rivela troppo costosa.
- **Ribir / rlvgl** (turni precedenti): Ribir vuole wgpu+std (no fit); rlvgl
  (no_std, retained) è il fit naturale ma l'utente vuole l'ecosistema egui e il
  desktop demo upstream.

## 12. Rischi e mitigazioni

| Rischio | Mitigazione |
|---|---|
| Wasmtime no_std ring-0 x86_64 poco battuto | **Spike** (run `.cwasm` hello) PRIMA del resto; se fallisce, fallback wasmi+mitigazioni |
| Integrazione blocking Wasmtime↔embassy | poll-based + epoch v1 (no fibre) |
| Kernel size cresce (Wasmtime ≫ wasmi) | runtime GUI-only; valutare feature-gate / lazy |
| Perf egui anche AOT | 720p + dirty-rect + repaint on-demand + contenuto leggero |
| Versione `.cwasm` host ≠ runtime | pin versione Wasmtime in Makefile + check al load |
| `bincode`/dipendenze no_std | seguire ricetta Theseus (bincode 2.0) |
| Emulatore terminale: TUI fullscreen (nano) richiede ANSI/box-drawing completi | riusare logica `vte`+grid+SGR già nel kernel (`console/`); coprire prima shell+tool semplici, nano in coda |
| Coesistenza Wasmtime(gui)+wasmi(tool) sullo stesso executor | sono task cooperativi indipendenti; PTY è il solo punto di contatto; testare con shell+`ls` prima di nano |

## 13. Prerequisiti e ordine (alto livello — il piano dettaglierà i task)

1. **Driver mouse PS/2** (IRQ12) → coda input. *(indipendente, testabile da solo)*
2. **Exec-memory W^X** nel paging (`map_page` con flag eseguibile).
3. **Spike Wasmtime no_std**: integrare crate + platform shim, eseguire
   `hello.cwasm`. *(gate decisionale: se KO → fallback)*
4. **Host ABI `ruos_gfx`** + `gfx` service + console suspend/restore.
5. **`gui` app**: backend egui (input→RawInput, tiny-skia raster, blit) →
   finestra "about" → demo ridotto + 1-2 finestre ruos (es. `ps`/`rtop`,
   browser `/mnt` via canale nativo).
6. **Host ABI `ruos_proc`** (`pty_open`/`proc_spawn`/`proc_poll`) +
   **emulatore terminale** nell'app (vte → grid → egui) → finestra "terminal"
   che esegue la shell e i tool su PTY.
7. **Runtime router** (`.cwasm`→Wasmtime) + **build step** Makefile.
8. **Benchmark FPS** su QEMU → tuning risoluzione/dirty-rect/contenuto.

## 14. Riferimenti

- egui / egui_demo_lib: https://github.com/emilk/egui
- Wasmtime no_std (issue #8341), Platform Support
  (https://docs.wasmtime.dev/stability-platform-support.html), portability
  article (https://bytecodealliance.org/articles/wasmtime-portability)
- Cranelift no_std (issue #1067), regressione no_std (wasmtime #1158)
- Theseus Wasmtime no_std port:
  https://www.theseus-os.com/2022/06/21/wasmtime-complete-no_std-port.html
- tiny-skia: rasterizzatore software
- ARCHITECTURE.md, roadmap step 13 (mouse PS/2 + GUI host fn)
