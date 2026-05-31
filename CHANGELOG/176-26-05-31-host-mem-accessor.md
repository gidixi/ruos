# 176 — Single audited guest-memory accessor for all host fns

**Data:** 2026-05-31

## Cosa

Introdotto `kernel/src/wasm/host/mem.rs` — modulo boundary unico per ogni
accesso alla memoria lineare guest. Tutte le 9 funzioni host che leggono o
scrivono la memoria del guest passano ora per questo modulo; nessun accesso raw
a `mem.read`/`mem.write` al di fuori di `mem.rs`.

### Nuovo modulo `host/mem.rs`

- `check_bounds(ptr, len, size)` — puro bound-check senza tipi wasmi, testabile
  in isolamento (Task 6 può fuzz-testarlo).
- `guest_read(&Caller, ptr, len)` → `Result<Vec<u8>, i32>` — legge `len` byte
  dalla memoria guest con verifica limiti; errore = errno WASI.
- `guest_read_into(&Caller, ptr, buf)` → `Result<(), i32>` — lettura
  di dimensione nota in buffer pre-allocato.
- `guest_write(&mut Caller, ptr, bytes)` → `Result<(), i32>` — scrittura
  bounds-checked; empty = no-op.
- `guest_write_u32(&mut Caller, ptr, val)` → `Result<(), i32>` — helper
  per parametri out-param scalari u32 (pattern frequente).
- Costanti `EINVAL = 28`, `EFAULT = 21` (errnos WASI usati al boundary).

### Migrazione — 43 siti in 9 file

Ogni `mem.read`/`mem.write` diretto nei file host è stato sostituito con la
chiamata accessor corrispondente. I siti precedentemente non verificabili ora
ritornano `Ok(EFAULT)` o `Ok(EINVAL)` invece di un panic/trap arbitrario.

File migrati (siti per file):

- `clock.rs` (2): `clock_time_get`, `clock_res_get`
- `random.rs` (1): `random_get` (loop a chunk da 256 B)
- `term.rs` (2): `tcgetattr` write, `tcsetattr` read
- `path.rs` (2): `path_open` inline read, helper `read_path`
- `service.rs` (5): `ruos_service_list`×2, `ruos_service_status`×2, helper `read_name`
- `sysinfo.rs` (7): helper `write_bytes_and_len`×2, `ruos_meminfo`, `ruos_dmesg`×2, `ruos_proc_list`×2
- `lifecycle.rs` (6): `args_sizes_get`×2, `args_get`×2, `environ_sizes_get`×2,
  `poll_oneoff` read; helper `write_u32`/`read_u32` aggiornati per usare
  accessor internamente (retrocompatibili con chiamanti in `fd.rs`)
- `proc.rs` (11): `ruos_exec`×2, `ruos_chdir`, `ruos_exec_pipeline`, `ruos_readdir`,
  `ruos_pci_list`×2, `ruos_net_iface`×2, `ruos_time_get`×7 (macro interna),
  `ruos_tcp_dial`
- `fd.rs` (11): socket/vfs `fd_write` read×2, console loop read,
  `fd_filestat_get`×3, `fd_fdstat_get`, `fd_prestat_get`, `fd_prestat_dir_name`

### Audit grep risultato

```
grep -rn 'mem\.read\|mem\.write\|\.read(&caller\|\.write(&mut caller' \
  kernel/src/wasm/host/ | grep -v 'host/mem.rs'
```
→ nessun output (clean).

## Perché

Ogni host fn che accede alla memoria guest-controllata è un potenziale vettore
di lettura/scrittura arbitraria in ring 0. Centralizzare in `mem.rs` — con un
solo `check_bounds` verificato — rende ogni host fn corrente e futura
sicura per costruzione al boundary guest↔kernel. Un errore in `check_bounds`
è un unico punto da correggere; prima era distribuito su decine di siti.

## File toccati

- kernel/src/wasm/host/mem.rs (nuovo)
- kernel/src/wasm/host/mod.rs (aggiunto `pub mod mem`)
- kernel/src/wasm/host/clock.rs
- kernel/src/wasm/host/random.rs
- kernel/src/wasm/host/term.rs
- kernel/src/wasm/host/path.rs
- kernel/src/wasm/host/service.rs
- kernel/src/wasm/host/sysinfo.rs
- kernel/src/wasm/host/lifecycle.rs
- kernel/src/wasm/host/proc.rs
- kernel/src/wasm/host/fd.rs
