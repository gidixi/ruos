# 205 — Boot clock/LAPIC senza PIT (fix hang HW reale)

**Data:** 2026-06-02

## Cosa
Eliminata ogni dipendenza dal **PIT** nel path di boot precoce, che faceva
**congelare il PC reale subito dopo l'handoff di Limine** (schermo nero, prima di
qualsiasi log a video). Due loop di polling del PIT **non bounded**:

1. `boot::clock::calibrate_tsc_per_ms` → `while (port_61.read() & 0x20)==0 {}`
   (PIT ch2 via speaker port 0x61). Primo passo di `kmain`.
2. `apic::lapic::calibrate` → `loop { ... pit ch0 readback ... }` (PIT ch0).
   Fase interrupts (anch'essa prima della console framebuffer).

Su UEFI moderno il PIT/speaker è spesso **gated off** → quei bit non cambiano mai
→ hang infinito. QEMU/VBox emulano il PIT alla perfezione → non si vedeva. Erano
**prima** della console framebuffer, quindi su HW reale (senza seriale) = nero
muto.

## Fix
- **boot clock**: TSC freq da **CPUID** (leaf 0x15 crystal ratio, poi 0x16 base
  MHz — sempre disponibili), con PIT come fallback **bounded** (cap a cicli TSC,
  non può più hangare) e default 2 GHz come ultima risorsa. `calibrate_tsc_per_ms`
  non hanga mai.
- **LAPIC timer**: `lapic::calibrate` ora misura la finestra col **TSC**
  (`read_tsc`/`tsc_per_ms`, calibrati in kmain prima della fase interrupts),
  niente più PIT. Rimosso l'import `Port` ora inutilizzato.

## Perché
Era la vera causa del "non parte più" su HW reale (NON la USB: spostare la fase
USB dopo il framebuffer non aveva cambiato il sintomo — vedi CHANGELOG 204, che
resta valido per visibilità/handoff ma non era la causa).

## File toccati
- kernel/src/boot/clock.rs (CPUID + PIT bounded + default)
- kernel/src/apic/lapic.rs (calibrazione LAPIC su TSC, no PIT)

## Note
- Verificato QEMU: TEST_PASS, `lapic calibrated 62855600 ticks/sec`, timer 100Hz,
  timestamp sani. Da confermare su HW reale (era non testabile da qui).
- Stesso fix serve su main al merge (il PIT unbounded è codice vecchio condiviso).
