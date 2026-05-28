# 26 — IDT + handler eccezioni + SERIAL globale + #BP smoke

**Data:** 2026-05-28

## Cosa
- `kernel/src/serial.rs`: aggiunto `SERIAL: spin::Mutex<Serial>` globale +
  `unsafe impl Send for Serial`. `Serial::new()` reso `const fn`.
- Nuovo `kernel/src/kprint.rs`: macro `kprintln!` su SERIAL globale.
- Nuovo `kernel/src/idt.rs`: IDT + handler `#DE`/`#UD`/`#GP`/`#PF`/`#DF`
  (su IST 0) + `#BP` resumable. Costanti `VEC_LAPIC_TIMER=0x20`,
  `VEC_KEYBOARD=0x21`, `VEC_SPURIOUS=0xFF`.
- `kmain` rifattorizzato per usare `SERIAL` + `kprintln!`; dopo `gdt::init()`
  chiama `idt::init()`, logga `idt up`, triggera `int3` (handler logga
  `bp ok rip=0x...` e ritorna).
- `main.rs`: aggiunto `#![feature(abi_x86_interrupt)]` (richiesto da nightly
  per `extern "x86-interrupt"`).

## Adattamenti API (x86_64 0.15.4)
- `Cr2::read()` ritorna `Result<VirtAddr, VirtAddrNotValid>`: gestito con
  `.unwrap_or(VirtAddr::zero())` nel `#PF` handler.

## Perché
Step 5 Task 2: kernel sopravvive a errori CPU e ha un canale di logging usabile
dagli handler (mutex statico).

## File toccati
- kernel/src/serial.rs, kernel/src/kprint.rs, kernel/src/idt.rs
- kernel/src/main.rs
- CHANGELOG/26-26-05-28-idt-exceptions.md
