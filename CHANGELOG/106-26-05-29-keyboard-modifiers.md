# 106 — Keyboard modifiers (Shift, Ctrl, Caps Lock)

**Data:** 2026-05-29

## Cosa

Keyboard ISR ignorava modifier keys. Conseguenza: niente `_`, `:`, `?`,
`!`, `@`, maiuscole, Ctrl-combo dal kernel.

Fix:

1. **SCANCODE_MAP_SHIFTED** — tabella 89-entry parallela alla normale,
   per ogni scancode US QWERTY ritorna variante shifted:
   - Numeri row → simboli `!@#$%^&*()_+`
   - Lettere → uppercase
   - Punteggiatura → `:`/`"`/`~`/`<`/`>`/`?`/`|`/`{`/`}`
2. **Atomic state**: `SHIFT_DOWN`, `CTRL_DOWN`, `CAPS_LOCK`.
   Aggiornati su make/break codes:
   - 0x2A (LShift), 0x36 (RShift) → SHIFT_DOWN
   - 0x1D (LCtrl) → CTRL_DOWN
   - 0x3A (CapsLock) make-only → toggle CAPS_LOCK
3. **ISR dispatch**:
   - Modifier scancode → aggiorna state + eoi + return (non emette char)
   - Skip break codes per tasti normali
   - Resolve char: Shift→shifted, Caps→toggle case su lettere,
     Ctrl+letter → `byte & 0x1F` (Ctrl-A=0x01, ecc.)

Estesi 0xE0 prefix (frecce/Home/End/Del) restano invariati.

## Perché

User input bug: trattino `_` impossibile da tastiera senza Shift
handling. Stesso per simboli, maiuscole, Ctrl-C etc.

## File toccati

- kernel/src/keyboard/mod.rs
- CHANGELOG/106-26-05-29-keyboard-modifiers.md (nuovo)
