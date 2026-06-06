# 305 — Fast cpu_id via RDTSCP + IA32_TSC_AUX (SMP Step 1a)

**Data:** 2026-06-05

## Cosa
`cpu_id()` ora ha un fast path che evita la lettura LAPIC MMIO (~200 ns):

- `cpu/mod.rs`:
  - `RDTSCP_OK: AtomicBool` — settato una volta dal BSP via `detect_rdtscp()`
    (`CPUID.80000001h:EDX[27]`).
  - `set_tsc_aux(dense_id)` — `wrmsr IA32_TSC_AUX(0xC000_0103) = dense_id` (no-op se
    `!RDTSCP_OK`).
  - `cpu_id()` — se `RDTSCP_OK`: una `rdtscp` (ritorna TSC_AUX in ECX = dense id),
    altrimenti il path LAPIC esistente (fallback verbatim).
  - `init_bsp()` — chiama `detect_rdtscp()` + `set_tsc_aux(0)` (il BSP è id 0).
  - `probe_fast_cpuid()` — diagnostico boot: marker
    `cpuprobe rdtscp=.. rdpid=.. tscaux_rw=..`.
- `cpu/ap.rs` — `set_tsc_aux(cpu_id)` come PRIMA istruzione di `ap_entry` (prima di
  qualunque `cpu::cpu_id()`), così nessun AP osserva un TSC_AUX stale una volta che
  `RDTSCP_OK` è true.
- `boot/phases/interrupts.rs` — chiama `cpu::probe_fast_cpuid()` dopo `smp::bringup()`.

**Diverse-system-safe:** feature-detect via CPUID + fallback LAPIC; mai assume RDTSCP.
`RDPID` (più diretto) scartato perché NON portabile (assente su VirtualBox e Coffee
Lake; presente solo su QEMU `-cpu max`). `RDTSCP`+`TSC_AUX` verificato funzionante su
QEMU, VirtualBox e HW reale (i7-8700).

## Perché
Lo spike Step 1 ha provato che un allocatore per-core regredisce finché `cpu_id()`
costa ~200 ns (LAPIC MMIO) ed è chiamato su ogni alloc. RDTSCP+TSC_AUX porta il costo
a ~few cicli senza MMIO e senza il quirk gs-base (rotto su VBox). Misurato:
`allocbench cpuid ns_per_call` **~200 → 23 ns**; `make test-boot` PASS (cpu_id corretto
in tutto boot+SMP). Ri-misura a `-smp 4`: magazine A da 31.9 → 9.6 ms, batte il default
talc (14.9 ms). È il prerequisito (Step 1a) della scelta allocatore (Step 1b = magazine
A). Vedi `docs/superpowers/decisions/2026-06-05-allocator-architecture.md` §8.

## File toccati
- kernel/src/cpu/mod.rs
- kernel/src/cpu/ap.rs
- kernel/src/boot/phases/interrupts.rs
- CHANGELOG/305-26-06-05-smp-fast-cpuid-rdtscp.md
