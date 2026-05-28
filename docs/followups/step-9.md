# Step 9 — followups

Followup non-blocking emersi dal per-task review e dal whole-implementation
review di Step 9 (async executor cooperativo). Aperti al merge di
`feature/async-executor` → `main`. Nessuno blocca lo Step 10; affrontare
opportunisticamente o quando il codice intorno viene toccato.

## F1 — `WAKE_PENDING` init-true comment

**File:** `kernel/src/executor/mod.rs` (decl di `WAKE_PENDING`)
**Severity:** 🟡 doc

L'atomic è inizializzato a `true` di proposito (forza il primo `poll()`
prima di considerare `hlt`). Aggiungere comment esplicito; oggi lo si
deduce solo leggendo il loop.

## F2 — `WAKE_PENDING` ordering

**File:** `kernel/src/executor/mod.rs` (store/load di `WAKE_PENDING`)
**Severity:** 🟡 cosmetic on x86

SeqCst è overkill per un singolo atomic UP. Su x86 il codegen è identico
a Relaxed (il vero memory barrier è il `cli`/`sti`), ma allineare l'intent
del codice all'architettura. `Relaxed` su store/load.

## F3 — 0xE0 extended-scancode latching

**File:** `kernel/src/keyboard/mod.rs` (ISR)
**Severity:** 🟡 visibile in Step 11

Oggi `if scancode < 0x80` filtra `0xE0` come "key release", e il make-code
successivo (arrow up = 0x48, ecc.) viene decodificato come tasto normale
del keypad. Risultato: arrow up = `'8'`, arrow left = `'4'`, ecc.

**Fix:** static `AtomicBool EXTENDED`; quando scancode == 0xE0, set + return;
prossimo byte → consultare tabella estesa (o skip).

## F4 — Dead `< 0x80` clamp in kbd ISR

**File:** `kernel/src/keyboard/mod.rs:83` (circa)
**Severity:** 🟢 dead code

`let b = if ch < 0x80 { ch } else { b'?' }` è unreachable: tutte le entry
in `SCANCODE_MAP` sono già `< 0x80`. Drop o convertire in `debug_assert!`.

## F5 — `DROPPED` counter reader API

**File:** `kernel/src/keyboard/queue.rs`
**Severity:** 🟡 missing API

`pub static DROPPED: AtomicU64` ma nessun reader. Aggiungere
`pub fn dropped() -> u64` per esporre via futuro `stat`/diagnostic.

## F6 — `read_char` single-consumer doc

**File:** `kernel/src/keyboard/queue.rs` (doc su `ReadChar` o `read_char`)
**Severity:** 🟡 documentation

`State` ha un solo `Option<Waker>`: due task in attesa concorrente si
sovrascriverebbero il waker (uno dei due perderebbe il wake). Documentare
"single-consumer queue" esplicitamente.

## F7 — Delay slot panic policy

**File:** `kernel/src/executor/delay.rs` (`panic!("delay slots exhausted")`)
**Severity:** 🟡 fragile per Step 11+

Oggi 8 slot bastano (≤3 task con Delay). Quando arriveranno SSH/GUI tasks
in Step 13/15, il panic diventa un cliff. Opzioni: crescere lista, oppure
Poll::Pending + wait queue. Da rivisitare quando il numero di task scala.

## F8 — Spec sync to as-built design

**File:** `docs/superpowers/specs/2026-05-28-rust-async-executor-design.md`
**Severity:** 🟡 spec drift

Lo spec descrive `static EXECUTOR: embassy_executor::Executor` con idle
embassy-managed; l'as-built usa `raw::Executor` in `UnsafeCell<MaybeUninit>`
+ outer loop manuale + `sti; hlt` esplicito. Tre "Open points" risolti
nel codice ma ancora listed come aperti.

**Fix:** sync lo spec all'as-built shape. Marcare i tre Open points
("embassy 0.6 vs 0.5", "HLT path", "KbdQueue lock-free vs Mutex") come
risolti.

## F9 — `delay::timer_tick` wake outside lock

**File:** `kernel/src/executor/delay.rs` (`timer_tick`)
**Severity:** 🟡 mirror pattern

Oggi `entry.waker.wake()` viene chiamato *dentro* il `try_lock` guard di
`SLOTS_LIST`. Safe perché `__pender` non lokka — ma fragile: se mai un
wake path indiretto toccasse `SLOTS_LIST`, deadlock. La keyboard queue
applica già il pattern corretto ("take waker, drop lock, then wake").

**Fix:** collezionare i waker due in un piccolo array on-stack sotto il
lock, rilasciare il guard, poi chiamare wake() su ciascuno.
