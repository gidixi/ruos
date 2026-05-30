# Pipe Unix + exec concorrente — Design Spec

**Data:** 2026-05-30
**Branch:** `feature/pipes-concurrent-exec`

## Contesto

ruos esegue userspace come moduli WASM (WASI Preview 1) su un executor async
**cooperativo, single-CPU** (niente thread preemptive, niente SMP — drop
espliciti del pivot 2026-05-28). La shell (`/bin/shell.wasm`) è anch'essa un
modulo WASM: parsa la riga, chiama la host fn `exec` per lanciare i comandi.

Oggi manca l'IPC byte-stream tra processi: la shell non supporta `cmd1 | cmd2`.
Il `proc` registry traccia i fiber vivi; `exec` è **single-slot** (la shell
posta un figlio e aspetta `done` prima del successivo → esecuzione sequenziale).

## Obiettivo

Supportare le **pipeline** `cmd1 | cmd2 | …` nella shell, con semantica Unix:
streaming + backpressure, EOF a chiusura del writer, exit code dell'ultimo
stadio. Gli stadi girano **concorrenti** sull'executor cooperativo (1 CPU) —
nessun multi-thread / SMP richiesto: producer/consumer si interlacciano via i
`fd_read`/`fd_write` che già sospendono il fiber.

## Non-goal (YAGNI)

- **Niente SMP / thread preemptive.** Concorrenza = cooperativa, come tutto il
  resto del kernel.
- **Niente redirezione file** `>` / `<` / `>>`. Solo `|`. Dopo, se serve.
- **Niente builtin dentro pipeline.** `cd`/`pwd`/`exit` in una pipeline →
  errore (o eseguiti standalone se non in pipeline). Le pipe utili sono
  esterno|esterno (`ls | grep`, `cat | wc`).
- **Niente pipe nominate (FIFO)** nel VFS, niente host fn `pipe()` esposta ai
  programmi generici, niente `pipefail`.
- **Lunghezza pipeline limitata** (max `PIPE_MAX_STAGES = 4`) per budget
  stack/task dei fiber wasmi.

## Architettura

La shell parsa `a | b | c`, serializza la lista `[{path, argv}…]` e la passa al
kernel con **una sola host fn `exec_pipeline`**. Il kernel:

1. crea N-1 pipe;
2. costruisce N fiber, legando i fd 0/1/2 di ciascuno alle estremità giuste
   (pts ai bordi della catena, pipe agli interni);
3. esegue gli N stadi **concorrenti** sull'executor;
4. attende il completamento di tutti;
5. ritorna l'exit code dell'ultimo stadio.

Tutta la logica pipe + concorrenza è kernel-side. I **figli WASM restano
invariati**: leggono fd0 / scrivono fd1 come sempre; il kernel ha solo
sostituito il backing di quei fd (pipe invece di pts).

Scelta `exec_pipeline` (vs `pipe()`+`exec` stile POSIX esposti alla shell):
la shell è WASM e il suo `exec` è bloccante single-slot — orchestrare N exec
concorrenti dal lato shell andrebbe contro il modello. Concentrare pipe +
concorrenza nel kernel mantiene la shell semplice e i figli intatti.

```
shell.wasm:  "ls | grep x"
   parse -> [{ "/bin/ls", ["ls"] }, { "/bin/grep", ["grep","x"] }]
   exec_pipeline(serialized) ────────────────► host fn
                                                  │
   kernel: crea pipe P                            │
     stage0 ls   : stdin=pts stdout=P.write stderr=pts ─┐ concorrenti
     stage1 grep : stdin=P.read stdout=pts stderr=pts ──┘
     join(all) -> exit = stage1.code ──────────► ritorna alla shell
```

## Componenti

### 1. `kernel/src/pipe/mod.rs` — oggetto Pipe

```rust
struct PipeInner {
    buf: VecDeque<u8>,
    cap: usize,                 // bounded, es. 64 KiB
    writers: usize,             // estremità di scrittura aperte
    readers: usize,             // estremità di lettura aperte
    read_waker:  Option<Waker>,
    write_waker: Option<Waker>,
}
type Pipe = Arc<Mutex<PipeInner>>;

pub struct PipeReadFile  { inner: Pipe }
pub struct PipeWriteFile { inner: Pipe }
```

- `PipeReadFile::read(buf)`: se `inner.buf` non vuoto → copia, wake write_waker,
  `Ok(n)`. Se vuoto && `writers == 0` → `Ok(0)` (EOF). Altrimenti registra
  read_waker → `Pending`.
- `PipeWriteFile::write(buf)`: se `readers == 0` → `Ok(0)` (stdout chiuso, lo
  stadio termina, niente deadlock). Se c'è spazio (`cap - len`) → push, wake
  read_waker, `Ok(n)`. Se pieno → registra write_waker → `Pending`.
- `Drop for PipeWriteFile`: `writers -= 1`; se 0 → wake read_waker (EOF reader).
- `Drop for PipeReadFile`: `readers -= 1`; se 0 → wake write_waker (sblocca un
  writer fermo su buffer pieno, che vedrà `readers == 0` → `Ok(0)`).
- Stesso pattern wakered di `PtySlaveFile` / `PtyPair` (Step 12). `Arc` +
  `spin::Mutex`; accesso sotto `without_interrupts`.

### 2. VFS — fd anonimi per le estremità

`kernel/src/vfs/file.rs`:
- `FileImpl::PipeRead(PipeReadFile)`, `FileImpl::PipeWrite(PipeWriteFile)` +
  arm in `read`/`write`/`stat`/`seek` (`seek` → `NotPermitted`; `stat` →
  device, size 0; il lato sbagliato di r/w → `NotPermitted`).

`kernel/src/vfs/mod.rs`:
- `pub fn pipe() -> (Fd, Fd)`: crea `PipeInner`, registra i due `FileImpl` via
  `fd::allocate`, ritorna `(read_fd, write_fd)`. Fd anonimi (nessun path),
  vivono nella `FDS` globale come ogni altro fd.

### 3. Exec concorrente — coordinatore pipeline

`kernel/src/wasm/pipeline.rs` (nuovo):
- `async fn run_pipeline(stages: Vec<(String, Vec<Vec<u8>>)>, cwd: String) -> i32`
  - crea le N-1 pipe via `vfs::pipe()`;
  - per ogni stadio costruisce un `Fiber`, imposta argv/cwd e **lega i fd**:
    stdin = (primo ? pts ereditato : `pipe[i-1].read_fd`),
    stdout = (ultimo ? pts ereditato : `pipe[i].write_fd`),
    stderr = pts ereditato. (Estende il pattern `rebind_stdio_pty`.)
  - esegue gli N fiber **concorrenti** (task/stack separati, signal di
    completamento per stadio, à la `exec_queue`); ritorna l'exit dell'ultimo.
- Concorrenza realizzata estendendo il meccanismo `exec_queue` da single-slot a
  multi-stadio (vedi piano). Stack separati per stadio (no join-su-stesso-stack:
  i fiber wasmi sono stack-heavy). Cap `PIPE_MAX_STAGES`.

### 4. Host fn `exec_pipeline`

`kernel/src/wasm/host/proc.rs`:
- `exec_pipeline(buf_ptr, buf_len, exit_ptr) -> i32` (import module `ruos`).
- La shell serializza la pipeline; formato semplice length-prefixed:
  `u32 nstages` poi per stadio `u32 path_len, path, u32 argc, (u32 len, bytes)*`.
- Solleva `SuspendReason::ExecPipeline { stages, cwd, exit_ptr }`; il fiber
  dispatch delega a `pipeline::run_pipeline` (come fa `exec` con `exec_queue`);
  scrive l'exit code e riprende la shell.

### 5. Shell — parsing `|`

`user/shell/src/main.rs`:
- Split della riga su `|` rispettando le virgolette (riusa il tokenizer
  esistente per ogni segmento).
- 0 pipe → percorso `exec` attuale **invariato** (inclusi i builtin).
- ≥1 pipe → se un segmento è un builtin → errore `shell: builtin in pipeline
  non supportato`; altrimenti serializza e chiama `exec_pipeline`.
- Exit code dell'ultimo stadio mostrato come per `exec`.

## Gestione errori

- Path di uno stadio non trovato / non istanziabile → quello stadio esce
  127/126 (come `exec_worker`); gli altri stadi proseguono (il reader a valle
  vede EOF). `run_pipeline` ritorna comunque l'exit dell'ultimo stadio.
- Pipeline troppo lunga (> `PIPE_MAX_STAGES`) → host fn ritorna errno; la shell
  stampa `shell: pipeline troppo lunga`.
- Serializzazione malformata → errno; nessun crash kernel.
- Pipe `cap` pieno con reader morto → il writer riceve `Ok(0)`/broken-pipe-like
  e lo stadio termina (no deadlock): se `writers>0` ma il reader è uscito,
  occorre tracciare anche `readers` per segnalare al writer la chiusura →
  `PipeInner.readers` + `Drop for PipeReadFile`; write con `readers==0` →
  `Ok(0)` (il programma vede stdout chiuso).

## Strategia di test

- **Unit kernel** (`#[cfg(test)]` o smoke a boot dietro feature): Pipe
  producer/consumer — write/read, EOF a writer-drop, EOF-scrittura a
  reader-drop, backpressure (write Pending a buffer pieno, sblocco a drain).
- **Integrazione** (`make run-ssh-test`-style o nuovo target): pipeline reale
  via shell.
  - `ls / | grep bin` → output contiene `bin`.
  - `cat /etc/init.sh | wc -l` (se `wc` esiste; altrimenti aggiungere una
    `.wasm` di test `pipecat`/`pipegrep` minima).
  - Assert via seriale / sessione SSH non-interattiva.

## Done criteria

- `ls / | grep <dir>` nella shell locale (e via SSH) stampa solo le righe che
  matchano.
- Una pipeline a 3 stadi funziona in streaming (producer infinito + `head`-like
  che chiude → il producer termina senza deadlock).
- Comando singolo (no `|`) invariato; builtin invariati.
- `make run-test` verde; nuovo test pipeline verde.

## Piano implementativo (sintesi — dettaglio nello step writing-plans)

1. `pipe/mod.rs`: PipeInner + PipeReadFile/PipeWriteFile + wakers + Drop, con
   unit test producer/consumer.
2. VFS: `FileImpl::PipeRead/PipeWrite` + `vfs::pipe()`.
3. `pipeline.rs` + estensione exec a multi-stadio concorrente; bind fd per
   stadio.
4. Host fn `exec_pipeline` + `SuspendReason::ExecPipeline` + dispatch nel fiber.
5. Shell: parsing `|` + serializzazione + chiamata; rifiuto builtin in pipeline.
6. Test d'integrazione + CHANGELOG + roadmap nota (pipe = parte di Step 11/shell
   o nota a sé).
