# 153 — Task 3: SSH authorized_keys parser

**Data:** 2026-05-30

## Cosa

`kernel/src/ssh/authkeys.rs`:
- `load(path)`: vfs read full file, parse line-by-line (skip blank /
  `#`-prefixed comments), `binfo!("loaded N keys")`, missing file =
  bwarn + empty Vec (server starts but rejects all logins)
- `parse_line`: split whitespace, require `ssh-ed25519`, decode b64
- `parse_blob`: RFC 4253 §6.6 `[u32 BE len]b"ssh-ed25519"[u32 BE len]
  [32 raw pubkey]`
- `base64_decode`: inline RFC 4648 decoder (~30 LoC, no dep)

`server.rs::spawn` ora chiama `authkeys::load` dopo host key.

`Makefile`: pre-popola `/mnt/auth.key` con header commentato in
`disk.img`. Utenti possono mcopy una pubkey reale post-build.

## Test

`make run-test`:
```
[T+6.361s] INFO ssh  host key generated at /mnt/host.key
[T+6.449s] INFO ssh  host key fingerprint 658cc0de…bf0c
[T+6.522s] INFO ssh  loaded 0 authorized key(s) from /mnt/auth.key
[T+6.586s] WARN ssh  transport pending Tasks 4-5
```

Test parse manuale post-merge: aggiungere una pubkey al file via
`mcopy -o -i build/disk.img id_ed25519.pub ::/auth.key` e
verificare `loaded 1`.

## File toccati

- kernel/src/ssh/authkeys.rs (full impl)
- kernel/src/ssh/server.rs (chiama authkeys::load)
- Makefile (seed auth.key in disk.img)
- CHANGELOG/153-26-05-30-ssh-authkeys.md (questo)
