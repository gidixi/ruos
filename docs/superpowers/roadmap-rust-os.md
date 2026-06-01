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

## Step 17 — Mouse PS/2 + rlvgl + host functions grafiche

- Driver mouse PS/2 (porta 0x64 controller, IRQ12 via IOAPIC, scancode 3 byte).
- Crate `rlvgl` (port Rust di LVGL, no_std).
- Host functions custom (NON WASI standard):
  - `ruos_fb_info(w, h, format)`
  - `ruos_draw_pixel`, `ruos_draw_rect`, `ruos_blit`
  - `ruos_input_poll()` → eventi keyboard/mouse
- App WASM grafiche sono ruos-specific (legate alle host fn custom), non
  portabili agli altri WASI runtime. Trade-off accettato.

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

**Rimane da fare:**
- **Fase 2** — Executor SMP-safe: run-queue mpmc o per-CPU + work-stealing,
  rimozione dell'invariante single-core, IRQ routing e TLB shootdown.

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
  [Step 17: mouse + rlvgl]  ← framebuffer/executor (8,9)
  [Step 18: SMP]            ← trasversale, alto rischio, ultimo
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
- Multi-CPU/SMP — solo se serve dopo Step 16.
- Filesystem on-disk persistente in Step 7 — solo tmpfs RAM. FAT/AHCI spostati
  allo Step 15 (richiede prima lo Step 13 PCI/ECAM).
- Hardware reale "ben rifinito" — l'OS funzionerà su HW reale (Limine ISO USB)
  ma il primary target di sviluppo è QEMU + VBox.
