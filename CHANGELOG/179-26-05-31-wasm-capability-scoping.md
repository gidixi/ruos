# 179 — WASM capability-scoped path grants (enforced against / grant)

**Data:** 2026-05-31

## Cosa

Aggiunge il meccanismo di capability-scoping per i path WASI: ogni task ha un
campo `root: String` (default `"/"`) che definisce il prefisso di percorso
assoluto a cui il task ha accesso. Viene eseguito un controllo canonicalize +
prefix in ogni handler di path WASI prima di qualsiasi operazione VFS.

### 1. Campo `root` in `RuntimeState` (`kernel/src/wasm/state.rs`)

```rust
/// Capability grant: absolute path prefix this task may access. Default "/"
/// (full FS, no behavior change). Narrowed for spawned tools later.
pub root: String,
```

Inizializzato a `String::from("/")` in `RuntimeState::new()`.

### 2. Helper `RuntimeState::grants(abs: &str) -> bool`

```rust
pub fn grants(&self, abs: &str) -> bool {
    if self.root == "/" { return true; }
    let root = self.root.trim_end_matches('/');
    abs == root || abs.starts_with(&alloc::format!("{}/", root))
}
```

Con `root == "/"` (valore unico impostato in questo task) `grants()` restituisce
sempre `true` → nessuna variazione di comportamento. Il punto è che il codice di
enforcement è presente e opera sul path canonicalizzato reale.

### 3. Enforcement in tutti gli handler path (`kernel/src/wasm/host/path.rs` e `proc.rs`)

Il controllo viene inserito nel layer host function, subito dopo che
`resolve_at`/`resolve_cwd` produce il percorso assoluto canonicalizzato, prima
del trap `Err(Error::host(...))`. Ritorna errno 76 (ENOTCAPABLE) nella
convenzione `Ok(76)` usata da tutte le host fns WASI di questo kernel.

Operazioni coperte:

| Operazione     | File                     | Dopo risoluzione in        | Errno convention |
|----------------|--------------------------|----------------------------|------------------|
| path_open      | host/path.rs             | `resolve_at` → `path`      | `Ok(76)`         |
| OpenDir        | host/path.rs (path_open) | stesso check prima del fork | `Ok(76)`         |
| chdir          | host/proc.rs             | `resolve_cwd` → `new_cwd`  | `Ok(76)`         |
| filestat       | host/path.rs             | `read_path` → `path`       | `Ok(76)`         |
| unlink         | host/path.rs             | `read_path` → `path`       | `Ok(76)`         |
| mkdir          | host/path.rs             | `read_path` → `path`       | `Ok(76)`         |
| rmdir          | host/path.rs             | `read_path` → `path`       | `Ok(76)`         |
| rename src+dst | host/path.rs             | `read_path` × 2 (src, dst) | `Ok(76)` × 2     |

Nota: `path_open` e `OpenDir` condividono lo stesso check — il controllo avviene
prima del branch `oflags & OFLAGS_DIRECTORY`, quindi copre entrambi i casi con
una sola riga.

## Perché

Oggi ogni modulo WASM può aprire qualsiasi path (unico preopen globale `"/"`).
Un bug o modulo ostile può leggere `/mnt/passwd` o scrivere su file di sistema.
Questa task fa atterrare il meccanismo di enforcement (campo grant + canonical
prefix check su ogni operazione path) verificato contro il grant no-op `"/"`,
in modo che un futuro `proc_spawn` possa passare un grant ristretto e il
traversal `../` sia già sconfitto.

## File toccati

- kernel/src/wasm/state.rs
- kernel/src/wasm/host/path.rs
- kernel/src/wasm/host/proc.rs
- CHANGELOG/179-26-05-31-wasm-capability-scoping.md
