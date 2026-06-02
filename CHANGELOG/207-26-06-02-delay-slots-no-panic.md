# 207 — Delay: 64 slot + esaurimento non-fatale (fix freeze multi-rtop)

**Data:** 2026-06-02

## Cosa
Avviare un secondo rtop (es. uno sulla console locale + uno via SSH) **bloccava
l'intera macchina**. Causa: la lista degli slot del `Delay` (timer cooperativo)
aveva solo **8 slot** e in caso di esaurimento faceva **`panic!`** → halt del
kernel → sistema congelato (su HW reale senza seriale = freeze muto).

I task di background tengono ~4-5 Delay in continuo (net_poll, tick, pty_watchdog,
ssh_dispatcher). Ogni app interattiva che fa race read-vs-timer (l'auto-refresh di
rtop via `poll_stdin`, nano) ne tiene 1. Due rtop + transienti (respawn shell,
sleep iniziale) → >8 → panic → freeze.

## Fix
- `executor/delay.rs`: `SLOTS` 8 → **64** (ampio margine per sessioni multiple).
- Esaurimento **non più fatale**: invece di `panic!`, il `Delay` si risolve
  subito (`Poll::Ready`) — il task procede un filo in anticipo e si ri-arma al
  giro dopo. Mai più halt. Con 64 slot praticamente non si raggiunge.

## Perché
Un freeze totale del kernel per esaurimento di una tabella interna è un DoS. 8
slot erano troppo pochi per un sistema multi-sessione (locale + SSH).

## Nota sull'altro sintomo (Ctrl+C "cross-PTY")
L'utente ha riferito anche che Ctrl+C sulla console locale ha fatto uscire rtop
su SSH. rtop gira in raw mode con ISIG disattivato → Ctrl+C è un byte normale che
rtop legge e interpreta come quit; il routing per-PTY è separato (i comandi SSH
girano sulla loro PTY). Probabilmente era starvation/caos cooperativo in
prossimità dell'esaurimento slot, culminato nella panic. Da riverificare dopo
questo fix; se ricapita (output SSH sulla console locale) è un bug di routing
term_pts separato da indagare con un repro.

## File toccati
- kernel/src/executor/delay.rs (SLOTS=64, esaurimento → Poll::Ready)

## Verifica
make run-test TEST_PASS; make run-rtop-test TEST_PASS_RTOP (rtop + delay system).
