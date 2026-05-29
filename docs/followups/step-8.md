# Step 8 — followups

Followup non-blocking emersi dal whole-implementation review di Step 8
(framebuffer console). Aperti al merge `e6d8171` (feature/fb-console → main,
2026-05-28). Nessuno blocca lo Step 9; affrontare opportunisticamente o
quando il codice intorno viene toccato.

## F1 — `FB_VIRT` publish ordering

**File:** `kernel/src/console/fb.rs` (`FramebufferConsole::new`)
**Severity:** 🟡 race window

`new()` pubblica `FB_VIRT` (Release) *prima* di chiamare `self.clear()`. Il
timer ISR è già attivo a quel punto in `kmain`: può vedere `FB_VIRT != null`
e fare XOR sulla cella in alto-sinistra mentre `clear()` la sta riempiendo.
Single-CPU + IRQ-only: no UB, ma glitch visivo di un tick.

**Fix:** pubblicare `FB_VIRT` *dopo* `clear()`, oppure wrappare
`FramebufferConsole::new` (o il call site in `fb_init::init`) in
`without_interrupts`.

## F2 — SGR 38/48 truecolor tail swallow

**File:** `kernel/src/console/ansi.rs` (`apply_sgr`)
**Severity:** 🟡 misuse on truecolor input

Truecolor (`\x1b[38;2;R;G;Bm`) è fuori scope, ma il ramo `38/48` oggi
controlla solo `Some(5)`. Se arriva `;2;...`, i restanti parametri
(`2`, `R`, `G`, `B`) ricadono nel top-level match: un eventuale `0` nella
tail triggera reset accidentale di fg/bg.

**Fix:** quando il param dopo 38/48 non è 5, consumare la tail
`;2;R;G;B` (o tutta la sequenza) invece di re-feedarla al match.

## F3 — `ConsoleFile::write` nested-lock vs keyboard ISR

**File:** `kernel/src/vfs/devices.rs` (`ConsoleFile::write`)
**Severity:** 🟠 deadlock landmine (pre-esistente, allargato da Step 8)

`ConsoleFile::write` lockа `SERIAL` direttamente, senza
`without_interrupts`. Dopo Step 8 la catena è `CONSOLE → SerialConsole →
SERIAL`: se un future locka `SERIAL` via `ConsoleFile` e la keyboard IRQ
parte mid-write, l'ISR `kprintln!` spinna su `SERIAL` → deadlock single-CPU.

**Fix:** instradare `ConsoleFile::write` via `CONSOLE.lock()`, oppure
wrappare la write in `without_interrupts`. Idealmente: tutti i writer
seriali dietro la stessa disciplina di `kprintln!`.

## F4 — Spec atomics stale

**File:** `docs/superpowers/specs/2026-05-28-rust-fb-console-design.md`
**Severity:** 🟢 doc drift

Lo spec elenca `FB_PIXEL_BGR` e `CURSOR_SHOWN` come atomics richiesti. Il
fix I1 li ha rimossi (dead). Aggiornare la lista: oggi pubblichiamo solo
`FB_VIRT`, `FB_PITCH`, `FB_BPP`, `CURSOR_POS`, `BLINK_COUNTER`.

## F5 — CSI `J` 0/1 + `K` 0/1/2

**File:** `kernel/src/console/fb.rs` (`csi_dispatch`)
**Severity:** 🟡 nice-to-have prima dello Step 11

Oggi `J` accetta solo `2` (clear schermo), `K` solo modo 0 implicito
(clear to EOL). Lo Step 11 (shell line editing) emetterà `\x1b[2K\r` per
ridipingere il prompt: funziona già. Ma `\x1b[0J` (clear to end-of-screen)
è comune in app ncurses-style.

**Fix:** leggere il param e gestire 0/1/2 in entrambi.

---

## ✅ CLOSED (2026-05-29)

- **F3** — chiuso de-facto. Keyboard ISR non chiama più `kprintln!` (Step
  11 T3 + Step 12 T3 retired path). Timer ISR usa solo atomics. No ISR
  contende SERIAL → nessun deadlock. Vedi CHANGELOG/100.
