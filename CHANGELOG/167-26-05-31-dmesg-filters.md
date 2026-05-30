# 167 — dmesg: flag CLI per filtri level/tag/grep + strip timestamp

**Data:** 2026-05-31

## Cosa
Esteso `user/dmesg/src/main.rs` con parsing argv minimale (solo `std::env::args()`,
nessuna crate aggiuntiva) e i seguenti flag:

- `-l <level>` — mostra solo righe a livello >= `<info|warn|err>` (case-insensitive).
  Le righe non parsabili (senza campo livello) vengono soppresse.
- `-t <tag>` — mostra solo righe con quel tag (match esatto). Ripetibile: più
  `-t` combinati in OR. Le righe non parsabili vengono soppresse.
- `-g <substr>` — substring match case-sensitive sul body del messaggio; per le
  righe non parsabili il match avviene sull'intera riga raw.
- `-T` — strippa il prefisso `[T+x.xs] ` dall'output.
- `-h` / `--help` — usage ed exit 0.

Filtri multipli combinano in AND. Flag sconosciuti o argomenti mancanti per
`-l`/`-t`/`-g` → errore su stderr, exit 2. Senza flag il comportamento è
identico a prima (dump raw del ring buffer da 32 KiB).

Parser di riga inline (~30 righe): riconosce il formato di `binfo!`/`bwarn!`
(`[T+<sec>.<ms>s] <LEVEL> <tag>  <message>`, doppio spazio tra tag e messaggio).
Tollera righe fuori formato (es. output `kprintln!` raw catturato in init)
lasciandole passare solo se nessun filtro `-l`/`-t` è attivo.

## Perché
Il ring buffer cresce in fretta (47 moduli + dispatcher SSH/PTY + watchdog
[[166-26-05-30-pty-watchdog]] producono decine di righe al boot). Per diagnosi
mirate serviva poter restringere l'output per livello, sottosistema (tag) o
keyword senza dover `dmesg | grep` ogni volta (e `grep` perde la struttura
livello/tag). `-T` torna utile quando si vuole confrontare due boot senza
rumore dai timestamp.

## Test
- `make iso` → ok
- `make run-test` → TEST_PASS (smoke battery invariata)
- Verifica funzionale: temporaneamente esteso `user-bin/smoke.sh` con
  `dmesg -h`, `dmesg -t ssh`, `dmesg -l warn`, `dmesg -T | head`,
  `dmesg -g boot`, `dmesg -t ssh -g auth`; tutti producono l'output atteso
  sul seriale (filtri AND/OR rispettati, `-T` strippa il prefisso, `-h`
  stampa l'usage). Edits a `smoke.sh` revertate prima del commit.

## File toccati
- user/dmesg/src/main.rs
