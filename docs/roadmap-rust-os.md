# Roadmap — ruos (Rust `no_std`, Limine, WASM userspace)

**Ultimo aggiornamento:** 2026-06-05 (step 1-19 ✅; GUI = egui, non rlvgl)

## North star

Eseguire **app `.wasm`** (compilate `wasm32-wasi`) come unico modello di
"userspace", con:

- una **shell** che esegue moduli WASM come comandi;
- **GUI** via **egui** (rasterizzata on-device con `tiny-skia`, eseguita come
  `.cwasm` su Wasmtime AOT; host fn `ruos_gfx` + bridge WIT). Era pianificata
  `rlvgl`, sostituita da egui;
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

Tutti gli step 1-19 sono ✅ DONE in `main` (dettaglio per-step sotto; quadro
d'insieme: [`README.md`](../../README.md) e
[`docs/ARCHITECTURE.md`](../ARCHITECTURE.md)). In sintesi, oggi gira:

- Boot Limine BIOS+UEFI hybrid ISO; kernel ELF higher-half; boot a fasi
  (arch → mem → interrupts+SMP → pci → devices → fs → storage → usb → userland).
- Fondamenta: COM1 + `kprintln!`, heap `talc` (16 MiB), frame allocator + paging,
  GDT/TSS/IDT, LAPIC/IOAPIC, timer 100 Hz (calibrato su ACPI PM su HW reale).
- I/O: PCIe (ECAM), AHCI/GPT/FAT32 (`/mnt`), networking (`smoltcp` + virtio-net +
  e1000, DHCP), **xHCI USB** (tastiera **e mouse** HID + hub + hot-plug).
- Input: PS/2 e USB per tastiera **e mouse** → shell (PTY) e GUI.
- Runtime: **`wasmi`** (tool `.wasm`) + **Wasmtime AOT no_std** (`.cwasm`
  GUI/component), fiber + fuel + ResourceLimiter, un solo accessor memoria guest.
- Userland: shell con pipeline, ~54 tool WASI, SSH (ed25519, password+pubkey),
  self-install su SSD, SMP compute pool (speedup 2-3×).
- **GUI**: servizio framebuffer `gfx` (`ruos_gfx`) + desktop **egui** (Wasmtime
  AOT) + **compositor kernel-side** multi-finestra (focus, drag/raise/close,
  compositing SMP, launcher).

Verificato in QEMU, VirtualBox e su **hardware reale** (USB input, GUI, installer
SSD). Battery di test headless: `make run-test` + i target `run-*` per sottosistema.

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

## Step 6 — Frame allocator fisico + paging API completata (✅ DONE)

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

## Step 7 — VFS minimale + tmpfs (✅ DONE)

- Trait `FileSystem`, `Inode`, `File` (open/read/write/seek/close + stat).
- `tmpfs` in-RAM: tree di `Inode` con contenuto `Vec<u8>` per file regolari.
- Popolazione iniziale a boot: `/init.wasm` (caricato come modulo Limine o da
  binari embedded in initrd), `/dev/console`, `/dev/random`, `/dev/zero`,
  `/dev/null`.
- VFS mount table (singolo mount inizialmente: `/` su tmpfs).
- Astrazione path: separator `/`, parsing senza alloc per lookup veloce.
- FAT (`fatfs` no_std) + block driver (virtio-blk via `virtio-drivers`)
  arrivano DOPO, solo se serve persistenza. Step 7 finisce con tmpfs.

## Step 8 — Framebuffer console (✅ DONE)

- Limine `FramebufferRequest` (RGB/BGR, pitch, dimensioni).
- Font bitmap 8x16 (es. font IBM VGA / `font8x8` crate).
- Scrolling, cursor lampeggiante (timer tick), color attributes.
- Trait `Console` con `write_str`. Impl: `SerialConsole`, `FramebufferConsole`,
  `MultiConsole` (entrambi).
- `kprintln!` ora scrive su MultiConsole. La seriale resta sempre attiva come
  debug log a doppio canale.

## Step 9 — Async executor no_std (✅ DONE)

- `embassy-executor` (consigliato: maturo, integrato con IRQ wake, scelta
  comune in OS hobby Rust) o alternative (`futures-lite` adattato).
- Tick scheduler: handler timer LAPIC `wake_all` o `Waker` registrato.
- Trait `AsyncRead`/`AsyncWrite` per console, tastiera, file VFS.
- Niente `Thread` astratti; le "task" sono `Future` ognuno con il proprio stack
  (gestito dall'executor).

## Step 10 — WASM runtime + WASI Preview 1 (✅ DONE)

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

## Step 11 — Shell locale (✅ DONE)

- Line editor minimale: input scancode → traduzione layout US (tabella),
  cursor ←/→, backspace, CR.
- PATH lookup nel VFS (es. `/bin/foo.wasm`); risoluzione comando → carica `.wasm`
  → esegue via runtime.
- Builtin minimali: `cd`, `pwd`, `ls`, `cat` (può essere builtin o `.wasm` —
  scegliere caso per caso), `exit`.
- Stdin/stdout/stderr collegati alla `MultiConsole` (o al PTY dello step
  successivo).
- Job control / pipe / redirezioni: DOPO, opzionali.

## Step 12 — PTY (pseudo-terminal) (✅ DONE)

- Coppia master/slave fd. Buffer circolare bidirezionale.
- Line discipline: raw mode, cooked mode (echo + line buffering).
- Shell locale gira sopra PTY (sostituisce stdin/stdout diretti). Stessa
  astrazione che userà SSH.

## Step 13 — PCI/PCIe enumeration (ECAM) (✅ DONE)

**Fondamenta comuni per ogni device PCIe** (NIC virtio-net dello Step 14, AHCI
dello Step 15, futuri NVMe/xHCI). Spec:
`docs/superpowers/specs/2026-05-29-rust-pci-ecam-design.md`.

- Estrazione **ECAM** dalla tabella ACPI **MCFG** (`acpi` crate, già parsato) →
  `Vec<EcamRegion>` su `AcpiInfo`. MCFG assente = non fatale (Vec vuoto).
- Modulo `pci/`: addressing config-space via formula
  `base + (bus<<20 | dev<<15 | fn<<12)`, accesso volatile su `map_io_page` (UC,
  idempotente — riusa il Mapper dello Step 6).
- Enumerazione piatta di tutti i bus/device/function di ogni regione ECAM →
  `Vec<PciDevice>` (vendor/device id, class/subclass/prog-if, header type, BAR
  decodificati con size-probing memoria/IO + 32/64-bit + prefetchable).
- API consumer: `find_class(class, subclass, prog_if)` → device → `bar(n)` →
  finestra MMIO. Helper Command-register: `enable_mmio()` (Memory Space),
  `enable_bus_master()` (Bus Master, richiesto per DMA). Walker capability-list
  (espone MSI/MSI-X per uno step MSI futuro; questo step solo enumera).
- **Non-goal (YAGNI):** niente fallback porte legacy `0xCF8/0xCFC` (target =
  `q35`, MCFG sempre presente), niente ricorsione PCI-to-PCI bridge (scan piatto),
  niente programmazione MSI/MSI-X, niente hotplug/PM/IOMMU/SR-IOV.
- **Smoke (`make run-test`):** QEMU `-machine q35 -device qemu-xhci` →
  `ruos: pci init ok devices=N` (N≥1), `find_class(0x0C,03,30)` trova l'xHCI,
  decode+sizing di BAR0 (BAR memoria 64-bit) loggato.

## Step 14 — Networking (✅ DONE)

- Driver `virtio-net` per QEMU/VBox (crate `virtio-drivers` o port). Device
  PCIe → discovery via Step 13 (`pci::find_class`/BAR). Costruisce l'**allocator
  DMA** (buffer fisicamente contigui) riusato poi da AHCI (Step 15).
- Stack TCP `smoltcp` (no_std, ben mantenuto).
- **CSPRNG critico**: `ChaCha20Rng` (crate `rand_chacha`) seedato all'init da
  `RDRAND` (CPUID feature check + `rdrand` instruction). Esposto via:
  - `random_get` di WASI (Step 10).
  - API kernel per SSH (Step 16).
- Test: DHCP + ping in QEMU.

## Step 15 — AHCI / SATA disk + FAT persistente (✅ DONE)

**Prerequisito: Step 13 (PCI/ECAM).** AHCI è un device PCIe → serve prima il
sottosistema PCI (`find_class` + BAR decode + Command bits). Spec:
`docs/superpowers/specs/2026-05-29-rust-pci-ecam-design.md`. Riusa l'allocator
DMA introdotto dallo Step 14 (networking).

Obiettivo: leggere/scrivere un disco SATA reale e montarci sopra un filesystem
persistente (FAT), sostituendo il solo tmpfs RAM dello Step 7 dove serve durabilità.

**Componenti:**

1. **Discovery via PCI** — `pci::find_class(0x01, 0x06, 0x01)` (Mass Storage /
   SATA / AHCI). BAR5 (`ABAR`) = base MMIO dei registri HBA. `enable_mmio()` +
   `enable_bus_master()` (AHCI fa DMA).

2. **HBA / port bring-up** — mappa `ABAR` (UC via `map_io_page`/`map_io_range`).
   Registri generici: `CAP`, `GHC` (AHCI Enable + reset), `PI` (ports
   implemented), `IS`. Per ogni porta attiva con device presente (`PxSSTS` DET):
   stop engine (FRE/ST clear), alloca **Command List** (1 KiB) + **FIS Receive
   Area** (256 B) + **Command Tables** (con PRDT) — buffer DMA fisicamente
   contigui dal frame allocator (Step 6), indirizzo fisico nei registri
   `PxCLB/PxFB`. Restart engine.

3. **ATA command set** — `IDENTIFY DEVICE` (capacità, LBA48, modello).
   `READ DMA EXT` / `WRITE DMA EXT` (LBA48) via FIS H2D, polling su `PxCI`/`PxIS`
   inizialmente, IRQ dopo (vettore AHCI via IOAPIC, o MSI-X — spec PCI follow-up).

4. **Block device layer** — nuovo trait kernel `BlockDevice { read_blocks,
   write_blocks, block_size, block_count }`. AHCI port = una impl. Astrae anche
   futuri NVMe/virtio-blk.

5. **FAT su block device** — crate `fatfs` (no_std) sopra il `BlockDevice`.
   Mount nel VFS (Step 7) come secondo mount (es. `/mnt` o `/`), accanto/al posto
   di tmpfs. File `.wasm` caricabili da disco persistente invece che da initrd.

6. **DMA infra** — i buffer AHCI (command list, FIS, PRDT data) richiedono
   memoria DMA: fisicamente contigua, indirizzo fisico noto (HHDM `virt = phys +
   offset`), uncached o flush appropriato. Riusa l'helper allocator DMA dello
   Step 14 (condiviso anche con futuri xHCI/virtio).

**Smoke contract (`make run-test`):** QEMU `q35` (controller AHCI built-in) +
`-drive file=disk.img,format=raw,if=none,id=d0 -device ide-hd,drive=d0,bus=ahci.0`
(o `-device ich9-ahci`). Asserzioni seriali: `ruos: ahci port0 sectors=N`
(IDENTIFY ok), round-trip write→read di un settore con pattern verificato,
mount FAT + `ls` di un file noto.

**Note:** niente partizioni/GPT inizialmente (FAT su disco intero); niente
write-back cache, niente NCQ multi-command (un command slot alla volta basta);
TRIM/SMART/hotplug fuori scope. NVMe = step parallelo separato (stesso
`BlockDevice`, controller PCIe diverso).

**Dipendenze:** Step 6 (frame allocator per DMA) + Step 13 (PCI/ECAM) + Step 7
(VFS per il mount) + Step 14 (allocator DMA). Indipendente da SSH/GUI.

## Step 16 — SSH server (✅ DONE)

Spec/piano: `docs/superpowers/specs/2026-05-30-rust-step16-ssh-design.md`.
Crate: **`sunset`** (no_std/no_alloc — match naturale), vendorizzato in
`third_party/sunset/` per patchare la chiusura canale. Auth: chiave pubblica
ed25519 hardcoded (`/mnt/auth.key`), host key persistente (`/mnt/host.key`).

Funziona end-to-end, verificato con OpenSSH (`make run-ssh-test`):
- KEX (curve25519-sha256) + chacha20-poly1305, CSPRNG da RDRAND.
- Auth pubkey ed25519 con verifica firma reale.
- **Sessione interattiva** su PTY (Step 12) → shell (Step 11): prompt,
  line-editing, comandi, exit.
- **Exec non-interattivo** (`ssh user@ruos cmd`): output completo (fix
  early-EOF lato sunset, CHANGELOG 157).

Limiti MVP (vedi spec + README): 1 sessione alla volta, 1 chiave, porta 22
fissa, exec gira attraverso la shell (prompt nel risultato), no exit-status,
no window-size, no SFTP/forwarding.

## Compatibilità WASI — incrementale (in corso)

Lavoro trasversale di "aumento compatibilità WASI/WASIX", indipendente dagli
step. Primo item landato:

- **`fd_readdir`** (spec `docs/superpowers/specs/2026-05-31-rust-fd-readdir-design.md`,
  CHANGELOG 171-175). `std::fs::read_dir` e crate tipo `walkdir`/`glob`
  funzionano da un binario `wasm32-wasip1` `std` puro, senza binding custom.
  Aggiunti: `FdEntry::Dir`, `path_open(O_DIRECTORY)`, host fn `fd_readdir`
  (dirent Preview 1 + cookie), risoluzione path relativa a `dir_fd`. La host
  fn legacy `ruos.readdir` (record 12-byte) resta per `ls`/`find`/`du`/`grep`.

## Step 17 — Mouse + GUI egui + host functions grafiche (✅ DONE)

**Pivot rispetto al piano originale:** la GUI è **egui** (rasterizzata on-device
con `tiny-skia`), **non `rlvgl`**. La scelta dà UI portabile sviluppabile su PC
e ricompilata invariata in ruos. La GUI gira su **Wasmtime AOT** (vedi Step
16-bis sotto), non su `wasmi`.

- **Mouse PS/2** (IRQ12, pacchetto 3 byte) **e mouse USB HID boot** (xHCI) →
  coda `MouseEvent` comune.
- **Servizio framebuffer `gfx`** con ABI `ruos_gfx`:
  - blit RGBA8888 (convertito al layout RGB/BGR del pannello), rendering
    dirty-rect, cursore software;
  - eventi input al guest: `MouseMove`/`MouseButton` + tasti (scancode PS/2
    Set 1; la tastiera USB mappa usage→Set 1).
- **Desktop egui** (`gui.cwasm`) end-to-end: submodule `ruos-desktop` (`gui-core`
  portabile + `ruos-backend`), raster `tiny-skia`.
- **Bridge tipizzato WIT / Component Model** (`ruos:gui/*`) oltre alle host fn
  raw, per ABI sicure verso desktop e compositor.
- **Compositor / window manager kernel-side**: ogni finestra è un'app WASM
  separata; input routing + click-to-focus, decorazioni + drag/raise/close,
  compositing SMP-parallelo a bande, launcher + lifecycle.

Le app grafiche sono ruos-specific (legate a `ruos_gfx`/WIT), non portabili agli
altri runtime WASI. Trade-off accettato.

## Step 16-bis — Runtime Wasmtime AOT no_std (✅ DONE)

Secondo runtime accanto a `wasmi`, abilitante per Step 17-19: **Wasmtime** in
`no_std`, runtime-only (no JIT/Cranelift), che esegue `.cwasm` **AOT-precompilati**
(`tools/wt-precompile`) a velocità quasi-nativa, con allocatore di memoria
eseguibile **W^X** (`memory/exec.rs`) e feature **Component Model**. Il router
`.cwasm` della shell instrada a Wasmtime; `wasmi` resta per i tool `.wasm`.

## Step 18 — SMP / multi-CPU

Oggi ruos è uniprocessor (UP): solo il BSP (CPU 0) gira; gli AP
(Application Processors) restano in `wait-for-SIPI`. Banner stampa
hardcoded "1 CPU" anche se VM ne ha 4+.

Obiettivo: detect + bring-up + scheduler multi-core.

### Fase 0 — fondamenta per-CPU (✅ DONE)

Branch `feature/smp-phase0-percpu`. Spec: `docs/superpowers/specs/2026-05-31-smp-phase0-percpu-design.md`.
Audit: `docs/superpowers/notes/2026-05-31-smp-lock-audit.md`.

Deliverable completati (tutto su 1 CPU, nessun AP avviato):

1. **`IrqMutex<T>`** — lock primitivo IRQ-safe: maschera IF durante il lock,
   impedisce deadlock ISR-vs-thread anche su core singolo. Drop ripristina
   lo stato IF salvato. Disponibile per futuri siti ISR-shared.

2. **Per-CPU data via GS-base** — `struct PerCpu { cpu_id, lapic_id, … }`,
   MAX_CPUS=16, array statico `PER_CPU`. BSP inizializzato con `init_bsp()`;
   `this_cpu()` legge GS_BASE MSR senza lock. AP slot riservati per Fase 1.

3. **Per-core GDT/TSS + double-fault IST** — array statici di 16 GDT/TSS e
   16 stack double-fault (uno per CPU). `gdt::init(cpu_id)` carica il
   descriptor del core corrente in GDTR e il relativo TSS in TR. BSP su
   slot 0; i 16 KiB `DOUBLE_FAULT_STACKS` sono partizionati per indice.

4. **Enumerazione CPU via ACPI MADT** — `acpi::enumerate_cpus()` itera le
   entry `LocalApic` del MADT e popola una lista (`cpu::cpu_count()`). AP
   rilevati ma **non avviati** (nessun INIT-SIPI-SIPI). Puramente informativo.

5. **Lock audit completo (~52 siti)** — zero MUST-FIX: ogni sito di stato
   condiviso già protetto da `spin::Mutex`, atomic con ordinamento corretto,
   o init-once/per-core per costruzione. Invariante executor documentato:
   un solo core (BSP) chiama `run()`/`poll()`; la run-queue **non è** ancora
   SMP-safe (Fase 2).

**Cosa NON fa Fase 0:** nessun AP avviato, nessun trampoline 16-bit, nessun
INIT-SIPI-SIPI, nessun IRQ routing multi-core, nessun TLB shootdown. Il
kernel gira esattamente come prima — stessa performance, stessa stabilità —
ma ora ha le fondamenta strutturali per le fasi successive.

### Fase 1 — AP bring-up → idle (✅ DONE)

Branch `feature/smp-phase1-ap-bringup`. Spec: `docs/superpowers/specs/2026-06-01-smp-phase1-ap-bringup-design.md`.

Deliverable completati:

1. **Limine MpRequest** — Limine consegna gli AP già in long-mode (64-bit) con
   stack temporaneo. Nessun trampoline 16-bit scritto a mano: il bootloader lo
   gestisce. `MP_REQUEST` statico dichiarato prima di `kernel_main`.

2. **`idt::load()` richiamato su ogni AP** — ogni AP carica il proprio IDTR
   tramite `idt::load()` prima di abilitare le interruzioni, usando il descrittore
   IDT condiviso con il BSP.

3. **LAPIC-based `cpu_id`** — il cpu_id è derivato dal LAPIC ID letto a runtime
   via MMIO (`0xFEE00020 >> 24`) e poi mappato in un indice denso via
   `set_cpu_mapping`. Approccio VMM-independent: funziona su QEMU, VirtualBox e
   hardware reale indifferentemente.

4. **`smp::bringup()` coordinator** — per ogni AP Limine: assegna cpu_id denso,
   chiama `cpu.bootstrap(ap_entry, id)` (INIT-SIPI-SIPI gestito dal firmware
   Limine), attende con spin ≤ 200 M iterazioni che ogni AP chiami `mark_online`.
   Log finale: `N/N APs online`.

5. **AP entry Rust** — `ap_entry(cpu_id)`: carica GDT/TSS per-core (slot
   `cpu_id`), legge LAPIC ID via MMIO, inizializza `PerCpu`, segnala
   `mark_online(cpu_id)`, entra in `hlt` loop (parcheggio idle).

6. **Test integrazione** — `make run-smp-test` (script `tests/smp-test.sh`):
   QEMU `-smp 4`, 60 s timeout, asserisce `3/3 APs online` + assenza `#PF`.
   Verificato su QEMU -smp 4 (3/3 online) e **VirtualBox con 4 vCPU**
   (banner sha == HEAD, `acpi: 4 CPU(s) found`, `smp: 3/3 APs online`,
   `init.sh complete`, nessun #PF).

**Cosa NON fa Fase 1:** nessun IRQ/timer sugli AP, nessun executor multi-core,
nessun TLB shootdown, nessun task pinned. Gli AP parcheggiano in `hlt`
in attesa di Fase 2.

### Fase 2 — kernel compute offload pool (✅ DONE)

Branch `feature/smp-phase2-executor`. Spec: `docs/superpowers/specs/2026-06-01-smp-phase2-compute-pool-design.md`.

Deliverable completati:

1. **SMP-safe work queue** — coda MPSC (`crossbeam-queue` SegQueue) condivisa tra
   BSP e AP. Il BSP inserisce job kernel (`SmpJob = Box<dyn FnOnce() + Send>`);
   ogni AP gira in un busy-poll loop (`ap_worker_loop`) estraendo ed eseguendo job.
   Nessun `hlt`, nessun STI, nessun preemption sugli AP.

2. **Host function `ruos_smp_bench`** — interfaccia WASM→kernel (modulo `ruos`,
   fn `smp_bench`) che esegue un benchmark dual-phase: fase parallela (N job CPU
   distribuiti sulla pool) + fase sequenziale (stessa work sul BSP). Restituisce
   un report ASCII: `parallel=Xms sequential=Yms speedup=Z.ZZx cores=[a,b,c]`.

3. **Tool WASI `smptest`** — binario `wasm32-wasip1` che invoca `ruos_smp_bench`
   e stampa il report. Montato in `/bin/smptest.wasm` via ISO.

4. **Speedup reale misurato** — su QEMU `-smp 4` (1 BSP + 3 AP): speedup tipico
   ~3.3× (es. `parallel=152ms sequential=506ms speedup=3.32x cores=[1,2,3]`).
   I core `[1,2,3]` confermano che tutti e 3 gli AP hanno eseguito job distinti.

5. **Test integrazione** — `make run-smp2-test` (script `tests/smp2-test.sh`):
   QEMU `-smp 4`, boot + SSH + `smptest` via PTY, asserisce `speedup >= 1.5x` e
   `>= 2 core distinti`. Verificato su QEMU (3.32x, 3 core) e VirtualBox con 6
   vCPU (`5/5 APs online`, banner sha == HEAD, `init.sh complete`, nessun #PF).

**Cosa NON fa Fase 2:** nessun `.wasm` girato sugli AP (solo job kernel), nessun
IPI-wake (AP usa busy-poll), nessun work-stealing per-CPU, nessun async executor
multi-core. Il BSP mantiene l'executor cooperativo invariato.

**Rimane da fare (futuro):**
- **Fase 3** — Executor SMP-safe: run-queue mpmc o per-CPU + work-stealing,
  IPI-wake per AP idle, routing IRQ su AP, TLB shootdown, esecuzione `.wasm`
  su AP con WASM runtime thread-safe.

**Componenti:**

1. **Detect via ACPI MADT** — già parsato in Step 5, conta entries
   `LocalApic` (LAPIC ID + processor UID + enable bit). Esponi via
   `cpu::count()`. Banner mostra `N CPU (1 active)` → progressivamente
   `N CPU active`.

2. **AP trampoline** — codice 16-bit real-mode → 32 → 64-bit long-mode
   in pagina fissa <1 MB (es. 0x8000). Setup GDT/IDT/CR3 condivisi col
   BSP (o nuovi per-CPU). Jump a una `ap_entry` Rust.

3. **INIT + SIPI sequence** — per ogni AP LAPIC ID:
   - Write LAPIC ICR: vector=0, delivery_mode=INIT, level=Assert
   - Delay 10 ms (timer Step 5)
   - Write ICR: vector=trampoline_phys>>12, delivery_mode=SIPI
   - Delay 200 µs, ripeti SIPI una volta (spec Intel SDM)
   - Aspetta che AP segnali ready (atomic flag)

4. **Per-CPU state** — struct con stack, current_task, lapic_id, GDT/TSS.
   Accesso via `GS_BASE` MSR + `swapgs`. Layout:
   ```rust
   #[repr(C)] struct PerCpu {
       cpu_id: u32, lapic_id: u32,
       kernel_stack_top: u64,
       current_fiber: *mut Fiber,
       // ...
   }
   ```

5. **Spinlock audit** — `spin::Mutex` su single-CPU è no-op effettivo
   (no contention, only BSP runs). Su SMP serve:
   - Memory ordering audit (SeqCst vs Acquire/Release)
   - Lock contention paths (sock pool, FDS, MOUNTS, CONSOLE, PAIRS)
   - Risolvere followups F2 (post-load-store-waker race in exec_queue),
     F1 EXEC_QUEUE single-slot mpmc

6. **Executor multi-CPU** — embassy raw oggi single-thread. Opzioni:
   - Per-CPU run queue + work-stealing (Rayon-style)
   - Global mpmc queue + N pollers
   - Pinned tasks (specifico CPU per IRQ affinity)

7. **IRQ routing** — IOAPIC redirect entries con destination LAPIC ID.
   Distribuire IRQ (keyboard → CPU 0, network → CPU 1, ecc.) o
   round-robin.

8. **TLB shootdown** — quando una CPU unmap'a pagina condivisa, deve
   notificare le altre via IPI per flush TLB locale. Critico per
   sicurezza memoria.

**Effort:** ~3-4 settimane. Alto rischio nuovi bug latenti (race
condition prima invisibili).

**Smoke contract:** 4 CPU attive, `cpu::count()` ritorna 4, executor
distribuisce wasm task su N core (osservabile via per-CPU log
prefix tipo `[CPU 2] INFO ...`).

**Rimandato post-Step-16 (SSH).** Single-CPU basta per WASIX
bootstrap + shell + SSH locale. SMP serve quando arriverà:
- Multi-utente SSH simultaneo (Step 16.5+)
- Performance compute-heavy wasm (bash/python multi-thread)
- Real hardware deployment con N core

## Step 20-24 (Roadmap Futura)

Con gli step 1-19 tutti completati (GUI e SMP inclusi), la nuova "North Star" si sposta verso l'espansione hardware, prestazioni multi-core complete e l'ecosistema app WASM.

### Step 20 — Wi-Fi RTL8188EU: Completamento
Il driver Wi-Fi attuale (RTL8188EU) supporta scan e associazione (WPA2). Rimane da completare il data path:
- **Crypto Path**: Implementare AES-CCMP (TX/RX) per decifrare/cifrare i frame 802.11 dati.
- **DHCP over Wi-Fi**: Instradare i pacchetti chiari verso `smoltcp` e ottenere un IP.

### Step 21 — SMP Fase 3: Executor Multi-Core
L'SMP attuale offre un pool di compute offload. La Fase 3 porterà l'executor su tutti i core:
- **Run-queue per-CPU / MPMC** con work-stealing (Rayon-style).
- **IPI-wake**: Svegliare gli AP dallo stato di idle `hlt` quando c'è lavoro (anziché usare busy-poll).
- **TLB Shootdown**: Sincronizzazione sicura della TLB tra core via IPI.
- **Esecuzione WASM su AP**: Far girare le app (fiber) concorrentemente su core multipli con un runtime thread-safe.

### Step 22 — Webserver HTTP
Sulla base della spec 2026-06-08:
- Integrazione di un server HTTP `no_std` (es. `picoserve`).
- Esposizione diagnostica o file sharing direttamente dall'OS, in stile SSH.

### Step 23 — Storage Avanzato (NVMe)
- Driver NVMe nativo via PCIe, riusando l'allocatore DMA e l'infrastruttura ECAM.
- Supporto potenziale a file system più robusti (es. ext2) rispetto al FAT32 attuale.

### Step 24 — Espansione Userland & GUI Apps
- **Terminale grafico**: Un vero emulatore di terminale scritto in egui per avere la shell `wasmi` come finestra nel desktop.
- **App complesse**: Porting di linguaggi interpretati in WASM (es. MicroPython, Lua).

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
                          [Step 10: WASM + WASI] --+--+
                                                   |  |
                                  [Step 11: shell] /  |
                                          |           |
                                          v           |
                                  [Step 12: PTY] <----+

Catena critica north-star (accesso remoto):

[Step 6: paging] ─┐
[Step 7: VFS] ────┤
                  v
        [Step 13: PCI/ECAM] ──┬──▶ [Step 14: virtio-net + CSPRNG] ──▶ [Step 16: SSH]
                              │                  │ (DMA infra)
                              │                  v
                              └──────────▶ [Step 15: AHCI + FAT] ──▶ (NVMe / virtio-blk)

Rami indipendenti (qualsiasi momento dopo i loro prereq):
  [Step 17: mouse + GUI egui + compositor]  ← framebuffer/executor (8,9)
  [Step 18: SMP]                            ← trasversale, alto rischio, ultimo
```

## Decisioni tecniche fissate

- **Runtime WASM:** `wasmi` (Rust puro, no_std, interpreter) per i tool `.wasm`
  CLI; **Wasmtime AOT no_std** (runtime-only, no JIT) per i `.cwasm`
  GUI/Component-Model dove serve velocità quasi-nativa. Due runtime, stessa
  filosofia sandbox.
- **Async executor:** `embassy-executor` (no_std, IRQ-aware).
- **CSPRNG:** ChaCha20 seedato da RDRAND. Mai usare il timer come entropy
  source.
- **SSH crate:** `sunset` (no_alloc) preferito; `russh` se l'integrazione
  async-first si rivela più semplice.
- **GUI:** **egui** (non `rlvgl`), rasterizzata on-device con `tiny-skia`, su
  Wasmtime AOT, con host fn `ruos_gfx` + bridge WIT e un compositor kernel-side.
  App grafiche WASM ruos-specific (legate a `ruos_gfx`/WIT), non portabili — è OK.

## Modello di Sicurezza (Capability-Based Security)

In un sistema basato su WASM in Ring 0, non si usano controlli hardware o permessi sui file basati su UID/GID; si utilizza invece un approccio chiamato Capability-Based Security gestito direttamente dal runtime e dal kernel al momento dell'iniezione delle interfacce.

Ecco come si gestirebbero le autorizzazioni (anche in un potenziale scenario multi-utente):

1. **Concessione granulare dei percorsi (Capability-Scoped Paths e Preopens)**
Nei sistemi WASI, i programmi non vedono l'intero file system per impostazione predefinita, ma solo le directory che il kernel "inietta" loro all'avvio (chiamate preopens).
In ruOS attualmente, l'ambiente riceve la radice completa tramite il file descriptor fd 3 = `/`.
In uno scenario multi-utente, il kernel inietterebbe a ciascun utente solo la propria directory (es. fd 3 = `/utenti/alice`).
I tentativi di uscire da questa radice autorizzata (ad esempio usando `../` nei percorsi) vengono bloccati a livello di funzione host dal kernel. Se l'utente cerca di aprire un file o cambiare directory fuori dal suo perimetro tramite chiamate come `path_open` o `chdir`, il kernel restituisce immediatamente l'errore 76 `ENOTCAPABLE` (al di fuori della capability concessa).

2. **Filtro delle Funzioni Host (Import)**
Le applicazioni WASM non possono eseguire istruzioni privilegiate della CPU; possono solo chiamare le funzioni host che il kernel decide esplicitamente di esportare e collegare al loro modulo.
Per differenziare i privilegi, il kernel potrebbe creare ambienti di esecuzione su misura. A uno strumento di sistema eseguito da un amministratore verrebbero forniti i collegamenti a funzioni distruttive o di rete (come `mkdisk`, `install` o `tcp_dial`).
Al contrario, al processo di un utente standard queste funzioni non verrebbero fornite affatto. Qualsiasi tentativo di invocare un'operazione non autorizzata (come usare `exec` per avviare binari di sistema) fallirebbe restituendo sempre l'errore 76 `ENOTCAPABLE`.

3. **Protezione dei Processi e della Memoria via Software**
Poiché tutti i processi girano nello stesso spazio di indirizzamento (Ring 0), la separazione è garantita dal fatto che la memoria lineare WASM di un utente è fisicamente inaccessibile al processo di un altro utente.
Ogni lettura o scrittura tra il kernel e lo spazio utente passa attraverso un'unica funzione rigorosamente ispezionata (l'audited guest-memory accessor), prevenendo attacchi in cui un utente cerca di leggere la memoria altrui.
Le API di controllo dei processi possiedono già logiche di autoprotezione: ad esempio, la funzione host `proc_kill` attualmente rifiuta di terminare i demoni protetti del kernel. Questa stessa logica software può essere estesa per verificare l'appartenenza di un processo: il kernel scarterebbe semplicemente le chiamate `proc_kill` se l'ID del processo bersaglio appartiene a un altro utente.

In sintesi, le autorizzazioni si gestiscono limitando ciò che il runtime inietta nel programma. Invece di far partire un processo per poi bloccarlo quando cerca di accedere a un file vietato, in un'architettura WASM il programma nasce all'interno di una "realtà" su misura, in cui i file di altri utenti e le funzioni amministrative semplicemente non esistono.

## Cosa NON è in roadmap (rifiutato esplicitamente)

- ~~Multi-CPU/SMP — solo se serve dopo Step 16.~~ → era differito, **poi
  implementato** (Step 18: bring-up AP + compute pool; il compositor lo usa per
  il compositing parallelo).
- Filesystem on-disk persistente in Step 7 — solo tmpfs RAM. FAT/AHCI spostati
  allo Step 15 (richiede prima lo Step 13 PCI/ECAM).
- Hardware reale "ben rifinito" — primary target di sviluppo resta QEMU + VBox,
  ma l'OS è **verificato su HW reale** (USB input, GUI/desktop, installer SSD).
