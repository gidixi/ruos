# Roadmap — ruos (Rust `no_std`, Limine, WASM userspace)

**Ultimo aggiornamento:** 2026-05-28 (pivot da Linux-ABI a WASM-first)

## North star

Eseguire **app `.wasm`** (compilate `wasm32-wasi`) come unico modello di
"userspace", con:

- una **shell** che esegue moduli WASM come comandi;
- **GUI** via `rlvgl` (host functions custom esposte al modulo WASM);
- **accesso remoto** via SSH (sessione interattiva attraverso PTY).

Il runtime WASM **è** il sandbox: superficie di syscall minima (WASI Preview 1),
isolamento garantito dal verificatore WASM, niente da reimplementare di Linux.

## Cosa il pivot ha DROPPATO (rispetto alla roadmap pre-2026-05-28)

- **Linux ABI / ELF userland.** Niente `fork`/`exec`/`mmap` Linux-syscall, niente
  loader ELF userland, niente libc Linux. App = `.wasm`.
- **User-mode CPU privilege (ring 3).** Niente `SYSCALL`/`SYSRET` MSR setup,
  niente GDT ring 3 attivata, niente TSS RSP0 per cross-ring. Sandbox = WASM,
  non page tables + privilegi CPU. Tutto kernel-mode (ring 0).
- **Preemptive thread scheduler.** Concurrency = async cooperative
  (`embassy-executor` o equivalente). Timer IRQ → wake del waker, non context
  switch hardware. Single-CPU; SMP eventuale dopo.
- **North star Podman/container.** Sostituito da WASM + GUI + SSH.

L'isolamento processi che con Linux richiedeva paging+ring 3 lo dà gratis il
sandbox WASM. Trade-off accettato: le app devono essere `wasm32-wasi` (la
toolchain è ottima per Rust/C/Go/Zig).

## Stato del codice

Funziona oggi (commit `969c2fd`+ in `main`):

- Boot Limine BIOS+UEFI hybrid ISO; kernel ELF higher-half a `0xFFFFFFFF80000000`.
- Seriale COM1 + `SERIAL: spin::Mutex<Serial>` globale + macro `kprintln!`
  (deadlock-safe via `without_interrupts`).
- Heap kernel 4 MiB via `talc`, backing su prima regione USABLE Limine accessibile
  tramite HHDM. `Box`/`Vec`/`String`/`BTreeMap` utilizzabili.
- GDT custom + TSS con IST 0 (16 KiB) per `#DF`.
- IDT con handler tipati `extern "x86-interrupt"` per `#DE`/`#UD`/`#GP`/`#PF`/
  `#DF` (su IST) + `#BP` resumibile.
- 8259 PIC mascherato; ACPI parsato (`acpi` crate 5.x) per LAPIC/IOAPIC base.
- LAPIC (xAPIC MMIO) enable + EOI + timer LVT in modalità periodica a 100 Hz,
  calibrato via PIT one-shot. `TICKS: AtomicU64` incrementato dall'handler timer.
- IOAPIC con redirection mask-first/atomic-low-write applicando ACPI IRQ
  source overrides.
- Tastiera PS/2 su IRQ1 (IOAPIC redirect → vettore `0x21`) → scancode raw su
  seriale.
- Mapping MMIO custom (`apic/mmio.rs`) — Limine HHDM non copre MMIO, page-walk +
  UC leaf con guardia `HUGE_PAGE`.

Asserzione `make run-test`: stringa `ruos: ticks=` sulla seriale. Verificato
TEST_PASS in QEMU, VirtualBox (con I/O APIC abilitato), e in principio su HW
reale via USB.

## Step 1-5 — Fondamenta (✅ DONE)

1. **Toolchain Rust nightly + target.** `x86_64-unknown-none` (target ufficiale
   dal 1.62, niente target custom). `build-std=core,alloc,compiler_builtins` in
   `.cargo/config.toml`. Nightly pinnato `nightly-2026-05-26`.
2. **Build cargo + Makefile + ISO.** Cargo compila il kernel; Makefile clona
   Limine v11.4.1-binary in `third_party/`, assembla `iso_root` + `xorriso` per
   ISO ibrida BIOS/UEFI, `limine bios-install`.
3. **Hello world Rust** `no_std`/`no_main`, seriale COM1 (`uart_16550`), panic
   handler che halta (`cli; hlt`).
4. **Heap + global allocator.** `talc` su Limine memory map + HHDM.
5. **IDT/GDT + APIC + timer + tastiera.** Crate `x86_64` 0.15 + `acpi` 5.x.

## Step 6 — Frame allocator fisico + paging API completata

- Frame allocator dalla Limine memory map (bitmap o stack di frame).
- Reserve regions: heap region (esposta da `memory::heap_region()`), kernel
  image, Limine reclaimable (post-reclaim), MMIO (via accessor da `apic/mmio.rs`),
  i `Box::leak` di PT pages.
- `Mapper` Rust generico costruito su `x86_64::structures::paging::OffsetPageTable`
  con HHDM offset come `PhysToVirt`. `map_page`/`unmap_page` espongono PRESENT/
  WRITABLE/NO_CACHE/WRITE_THROUGH/NO_EXECUTE.
- Sostituisce il `mmio.rs` ad-hoc con il `Mapper` generico (mantiene la guardia
  `HUGE_PAGE`).
- NO per-process page tables. NO ring 3. È paging "di sistema": heap growth,
  mmap futuri, MMIO devices.

## Step 7 — VFS minimale + tmpfs

- Trait `FileSystem`, `Inode`, `File` (open/read/write/seek/close + stat).
- `tmpfs` in-RAM: tree di `Inode` con contenuto `Vec<u8>` per file regolari.
- Popolazione iniziale a boot: `/init.wasm` (caricato come modulo Limine o da
  binari embedded in initrd), `/dev/console`, `/dev/random`, `/dev/zero`,
  `/dev/null`.
- VFS mount table (singolo mount inizialmente: `/` su tmpfs).
- Astrazione path: separator `/`, parsing senza alloc per lookup veloce.
- FAT (`fatfs` no_std) + block driver (virtio-blk via `virtio-drivers`)
  arrivano DOPO, solo se serve persistenza. Step 7 finisce con tmpfs.

## Step 8 — Framebuffer console

- Limine `FramebufferRequest` (RGB/BGR, pitch, dimensioni).
- Font bitmap 8x16 (es. font IBM VGA / `font8x8` crate).
- Scrolling, cursor lampeggiante (timer tick), color attributes.
- Trait `Console` con `write_str`. Impl: `SerialConsole`, `FramebufferConsole`,
  `MultiConsole` (entrambi).
- `kprintln!` ora scrive su MultiConsole. La seriale resta sempre attiva come
  debug log a doppio canale.

## Step 9 — Async executor no_std

- `embassy-executor` (consigliato: maturo, integrato con IRQ wake, scelta
  comune in OS hobby Rust) o alternative (`futures-lite` adattato).
- Tick scheduler: handler timer LAPIC `wake_all` o `Waker` registrato.
- Trait `AsyncRead`/`AsyncWrite` per console, tastiera, file VFS.
- Niente `Thread` astratti; le "task" sono `Future` ognuno con il proprio stack
  (gestito dall'executor).

## Step 10 — WASM runtime + WASI Preview 1

- Runtime: **`wasmi`** (Rust puro, `no_std`, interpreter) — match perfetto con
  lo stile del progetto. WAMR (C) via FFI è plan B se la performance non basta.
- Host functions WASI Preview 1 minime:
  - `args_get`, `args_sizes_get`
  - `environ_get`, `environ_sizes_get`
  - `clock_time_get` (LAPIC timer + epoch fissa)
  - `random_get` (CSPRNG ChaCha20 seedato da RDRAND, vedi Step 14)
  - `fd_read`, `fd_write`, `fd_seek`, `fd_close`, `fd_fdstat_get`
  - `path_open`, `path_create_directory`, `path_unlink_file`
  - `proc_exit`
- Verifica milestone: `hello_world.wasm` compilato con
  `cargo build --target wasm32-wasi` viene caricato da VFS (`/init.wasm`) ed
  eseguito; stampa "Hello from WASM!" sulla console.

## Step 11 — Shell locale

- Line editor minimale: input scancode → traduzione layout US (tabella),
  cursor ←/→, backspace, CR.
- PATH lookup nel VFS (es. `/bin/foo.wasm`); risoluzione comando → carica `.wasm`
  → esegue via runtime.
- Builtin minimali: `cd`, `pwd`, `ls`, `cat` (può essere builtin o `.wasm` —
  scegliere caso per caso), `exit`.
- Stdin/stdout/stderr collegati alla `MultiConsole` (o al PTY dello step
  successivo).
- Job control / pipe / redirezioni: DOPO, opzionali.

## Step 12 — PTY (pseudo-terminal)

- Coppia master/slave fd. Buffer circolare bidirezionale.
- Line discipline: raw mode, cooked mode (echo + line buffering).
- Shell locale gira sopra PTY (sostituisce stdin/stdout diretti). Stessa
  astrazione che userà SSH.

## Step 13 — Mouse PS/2 + rlvgl + host functions grafiche

- Driver mouse PS/2 (porta 0x64 controller, IRQ12 via IOAPIC, scancode 3 byte).
- Crate `rlvgl` (port Rust di LVGL, no_std).
- Host functions custom (NON WASI standard):
  - `ruos_fb_info(w, h, format)`
  - `ruos_draw_pixel`, `ruos_draw_rect`, `ruos_blit`
  - `ruos_input_poll()` → eventi keyboard/mouse
- App WASM grafiche sono ruos-specific (legate alle host fn custom), non
  portabili agli altri WASI runtime. Trade-off accettato.

## Step 14 — Networking

- Driver `virtio-net` per QEMU/VBox (crate `virtio-drivers` o port).
- Stack TCP `smoltcp` (no_std, ben mantenuto).
- **CSPRNG critico**: `ChaCha20Rng` (crate `rand_chacha`) seedato all'init da
  `RDRAND` (CPUID feature check + `rdrand` instruction). Esposto via:
  - `random_get` di WASI (Step 10).
  - API kernel per SSH (Step 15).
- Test: DHCP + ping in QEMU.

## Step 15 — SSH server

- Crate: `sunset` (no_std, no_alloc anche se `alloc` adesso esiste — comunque
  match perfetto) o `russh` (async, richiede alloc + executor — già pronto).
- Auth: chiave pubblica hardcoded all'inizio (testabile via `ssh -i ...`).
- Modello inizio: **exec non-interattivo** (`ssh user@ruos /bin/foo.wasm`) —
  basta runtime WASM + VFS, senza PTY. Già utile.
- Modello completo: **sessione interattiva** con PTY (Step 12) → shell
  (Step 11) sopra.

## Diagramma di dipendenza

```
[Step 5: IRQ/timer/kbd]
        |
        v
[Step 6: paging + frame alloc] --+
                                 |
                                 v
                       [Step 7: VFS + tmpfs] --+
                                               |
              [Step 8: framebuffer console] ---+
                                               |
                                               v
                          [Step 9: async executor]
                                               |
                                               v
                          [Step 10: WASM + WASI] --+--+--+
                                                   |  |  |
                                  [Step 11: shell] /  |  |
                                          |           |  |
                                          v           |  |
                                  [Step 12: PTY] <----+  |
                                                         |
              [Step 13: mouse + rlvgl + draw host fn] <--+
                                                         |
              [Step 14: virtio-net + smoltcp + CSPRNG] --+--+
                                                            |
                                          [Step 15: SSH] <--+
```

## Decisioni tecniche fissate

- **Runtime WASM:** `wasmi` (Rust puro, no_std, interpreter). Performance
  adeguata per CLI tool e shell. JIT (WAMR/wasmtime) solo se profiling lo
  giustifica.
- **Async executor:** `embassy-executor` (no_std, IRQ-aware).
- **CSPRNG:** ChaCha20 seedato da RDRAND. Mai usare il timer come entropy
  source.
- **SSH crate:** `sunset` (no_alloc) preferito; `russh` se l'integrazione
  async-first si rivela più semplice.
- **GUI:** `rlvgl` con host functions ruos-specific. App grafiche WASM non sono
  portabili — è OK.

## Cosa NON è in roadmap (rifiutato esplicitamente)

- Multi-utente Unix-style (uid/gid, permessi POSIX) — fuori scope hobby.
- Multi-CPU/SMP — solo se serve dopo Step 15.
- Filesystem on-disk persistente in Step 7 — solo tmpfs RAM. FAT/AHCI dopo.
- Hardware reale "ben rifinito" — l'OS funzionerà su HW reale (Limine ISO USB)
  ma il primary target di sviluppo è QEMU + VBox.
