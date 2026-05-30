# 161 — SSH password auth (PBKDF2-HMAC-SHA256, on by default)

**Data:** 2026-05-30

## Cosa
SSH server (`kernel/src/ssh/`) ora accetta **anche** password auth in
parallelo al pubkey. Implementazione:

- **`kernel/src/ssh/password.rs`** (nuovo): parser per
  `/mnt/passwd` formato `pbkdf2-sha256:<iter>:<salt-hex>:<hash-hex>`,
  verifica PBKDF2-HMAC-SHA256 con confronto constant-time. Hex e
  costanti-time scritti inline (no deps extra). Iterazioni < 1000
  rifiutate per evitare config triviali.
- **`kernel/src/ssh/mod.rs`**: aggiunto `passwd_path: "/mnt/passwd"`
  nel `Config` statico.
- **`kernel/src/ssh/server.rs`**: carica `/mnt/passwd` (opzionale —
  missing = solo pubkey) e lo passa via `Arc<PasswordHash>` al
  session task.
- **`kernel/src/ssh/sunset_io.rs`**: `FirstAuth` ora chiama
  `enable_password_auth(passwd.is_some())` — il metodo password
  appare nella `userauth_methods` solo se è stato configurato un
  hash valido. `PasswordAuth` event chiama `password::verify` su
  PBKDF2.
- **`kernel/Cargo.toml`**: aggiunta dep `pbkdf2 = { version =
  "0.12", default-features = false, features = ["hmac"] }`.
  Il `sha2` già presente per ed25519-dalek copre la PRF.
- **`Makefile`**: la regola `$(DISK_IMG)` ora **semina `/passwd`
  di default** durante la creazione di disk.img — password = valore
  di `RUOS_PASSWORD` (default `ruos`). Quindi `make iso && make
  disk` produce direttamente un sistema con SSH password attivo,
  senza step manuali. Aggiunto target `passwd-on-disk` (per
  ri-stampare l'hash su un disk esistente con password diversa) e
  `run-passwd-test`. Tutto via Python stdlib (`hashlib.pbkdf2_hmac`).
- **`tests/ssh-passwd-test.sh`** (nuovo): boot QEMU + sshpass con
  `PreferredAuthentications=password` → verifica `auth ok user=root
  (password)` in serial + `pwd` ritorna `/`.

## Perché
La UX pubkey richiedeva di iniettare manualmente la propria
`.ssh/id_ed25519.pub` in `disk.img` (conversione VDI per
VirtualBox) prima di potersi collegare. Per uso interattivo da
VirtualBox o test rapidi questo è scomodo. La password è
oggettivamente meno sicura (`docs/superpowers/specs/2026-05-30-rust-step16-ssh-design.md`
e la roadmap raccomandavano pubkey-only) — ma su LAN/VirtualBox
per scopo demo è un trade-off accettabile, lasciato opt-in: se
`/mnt/passwd` non esiste, password resta disabilitato (il metodo
non viene nemmeno offerto al client).

## Test
- `make run-ssh-test` → `TEST_PASS_SSH` (regression pubkey OK)
- `make run-passwd-test` → `TEST_PASS_PASSWD` (auth password OK)

## Come usarlo
```
make iso disk            # disk.img già contiene /passwd con password 'ruos'
# avvia ruos (VirtualBox con disk.vdi convertito da disk.img, o make run)
ssh root@<ip>            # password: ruos
```

Per cambiare la password:
```
make disk RUOS_PASSWORD=hunter2          # rebuild from scratch
# OR
make passwd-on-disk RUOS_PASSWORD=hunter2   # patch existing disk.img
```

## File toccati
- kernel/Cargo.toml
- kernel/src/ssh/mod.rs
- kernel/src/ssh/password.rs
- kernel/src/ssh/server.rs
- kernel/src/ssh/sunset_io.rs
- Makefile
- tests/ssh-passwd-test.sh
