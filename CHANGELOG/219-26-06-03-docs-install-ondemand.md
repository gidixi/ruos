# 219 — docs: install disk-select + on-demand tools

**Data:** 2026-06-03

## Cosa
Aggiornati README + `docs/ARCHITECTURE.md` ai due feature recenti che le docs
precedevano:
- `install` ora **elenca i dischi** (`install` senza arg) e installa su quello
  scelto (`install <n>`), non più solo sul primo SATA.
- Sistema **installato su SSD**: ESP slim (kernel + shell + init + rete/SSH) +
  i ~50 comandi sulla **partizione dati** (`/mnt/bin`), caricati **on-demand**
  dalla shell (`resolve_path`: /bin → /mnt/bin). La ISO live resta invariata.

## Perché
Le docs (README + ARCHITECTURE, dalla pulizia della sessione parallela)
descrivevano il vecchio modello "tutti i tool come moduli Limine sotto /bin" e
l'`install` auto-target. Allineate al codice merge­ato (`b204eac` disk-select,
`1d40a2a` on-demand).

## File toccati
- README.md (riga install + sezione "Userspace tools")
- docs/ARCHITECTURE.md (Shell / Tools / Self-install / Lifecycle)
