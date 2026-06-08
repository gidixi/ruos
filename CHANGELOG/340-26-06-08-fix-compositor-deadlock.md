# 340 — Fix WM compositor deadlock on real hardware

**Data:** 2026-06-08

## Cosa
Implementato un pattern di "work stealing" in `dispatch_bands` (nel Window Manager) e in `ruos_smp_bench`. Ora il core chiamante (es. il core grafico), mentre attende in `poll_done` che gli altri core completino i job dal pool SMP, estrae attivamente i job rimanenti nella coda e li esegue localmente (`smp::pool::take()` e `run_slot()`), invece di girare a vuoto in uno `spin_loop`.

## Perché
Su hardware reale, se gli Application Processor (AP) tardavano ad attivarsi, si bloccavano, o se gli IPI di wake andavano persi/ritardati, il core grafico rimaneva bloccato in un ciclo di spin infinito in attesa del completamento dei job di composizione a bande (deadlock). Questo causava il freeze completo dell'interfaccia (Window Manager) all'avvio su macchine fisiche, a differenza delle macchine virtuali. Facendo collaborare il core in attesa, si garantisce il progresso in avanti (forward progress) e si prevengono blocchi in caso di problemi con gli altri core.

## File toccati
- kernel/src/wasm/wt/wm.rs
- kernel/src/wasm/host/smp.rs
