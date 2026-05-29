# Step 11 — Shell (shell.wasm + ruos_exec)

**Data:** 2026-05-29
**Roadmap step:** 11
**Stato:** spec approvata, da implementare

## Contesto

Roadmap step 11 originale: line editor + PATH lookup + builtin minimali
(`cd`, `pwd`, `ls`, `exit`). Step 10.5 ha portato la fiber architecture
e Step 10.5b (VFS hardening) ha aggiunto `readdir`/`stat` + fix race +
ConsoleFile keyboard wire. Tutte le pre-condizioni VFS e wasm runtime
sono pronte.

Step 11 pivot rispetto al testo roadmap: shell = `shell.wasm`
(`wasm32-wasip1`), non kernel-side. Coerenza con WASIX-first.

## Obiettivo

Userland reale interattivo dentro ruos:

- `shell.wasm` legge `/etc/init.sh` (Limine module), esegue ogni riga,
  poi entra in loop interattivo (se boot interattivo).
- `shell.wasm` supporta 4 builtin (`cd`, `pwd`, `exit`, `help`) +
  external commands (qualunque `/bin/<name>.wasm` o path assoluto).
- Per ogni external: spawn nuovo Fiber via custom host fn `ruos_exec`.
- 3 external `.wasm` consegnati come moduli Limine: `/bin/ls.wasm`,
  `/bin/cat.wasm`, `/bin/echo.wasm`.

## Decisioni strategiche (brainstorm 2026-05-29)

1. **Architettura shell**: **Opt B** — `shell.wasm` come Fiber +
   `ruos_exec` host fn. Pivot futuro a `bash.wasm` immediato (drop-in
   sostituzione).
2. **Scope smoke**: **Full** — 4 builtin + 3 external + `/etc/init.sh`
   boot script.
3. **Demo flow `make run-test`**: shell esegue init.sh non-interattivo,
   stampa `shell: init.sh complete`, exit. Sentinel HELLO =
   `shell: init.sh complete`.
4. **Keyboard ownership** (F7 Step 10.5 followup): drop `kbd_echo_task`.
   Shell.wasm è l'unico consumer della keyboard queue via FD 0
   (ConsoleFile o KbdReadChar — entrambi i path valgono, vedi nota).

## Architettura

```
Boot (sync):
  Limine → kmain → ... → modules::mount_all()
    /etc/init.sh        — boot script
    /bin/shell.wasm     — userland shell
    /bin/ls.wasm        — external
    /bin/cat.wasm       — external
    /bin/echo.wasm      — external
    /init.wasm          — esistente, vedremo se restare o spostare
    /server.wasm, /client.wasm — Step 10 demo, restano

Executor::run() spawns:
  tick_task             — esistente
  net_poll_task         — esistente
  wasm_task(/bin/shell.wasm) — UNICO consumer wasm post-boot
  (no più kbd_echo_task, no init/server/client per default)
```

Demo wasm Step 10 (`init.wasm`/`server.wasm`/`client.wasm`) sono ora
**lanciabili dal shell** invece di auto-spawnate. Dimostrazione meglio
del pattern userland.

## Host fns nuovi

### `ruos_exec` (kernel custom, non WASIX standard)

```c
ruos_exec(
    path_ptr: u32, path_len: u32,
    argv_ptr: u32, argv_len: u32,   // argv = string array, format vedi sotto
    exit_code_ptr: u32              // output: i32 child exit code
) -> i32 errno
```

`argv` in wasm memory format:
```
[u32 count]
[u32 offset_to_first_string]
[u32 length_of_first_string]
... (count times)
[u8 string_data_0]
[u8 string_data_1]
...
```

Trap `SuspendReason::Exec { path, argv, exit_code_ptr }`. Fiber dispatch:

```rust
SuspendReason::Exec { path, argv, exit_code_ptr } => {
    let bytes = match crate::vfs::read_all(&path).await {
        Ok(b) => b,
        Err(_) => return 8,  // ENOENT
    };
    let mut child = match crate::wasm::fiber::Fiber::new(&bytes) {
        Ok(f) => f,
        Err(_) => return 71, // ENOEXEC
    };
    child.set_args(argv);
    let code = child.run().await;
    let _ = self.write_to_memory(exit_code_ptr, &(code as i32).to_le_bytes());
    0
}
```

`Fiber::set_args(argv)` popola `RuntimeState.args` con i `Vec<Vec<u8>>` di
ciascuna stringa null-terminated.

### `ruos_readdir` (kernel custom)

```c
ruos_readdir(
    path_ptr: u32, path_len: u32,
    buf_ptr: u32, buf_len: u32,
    nread_ptr: u32
) -> i32 errno
```

Trap `SuspendReason::ReadDir { path, buf_ptr, buf_len, nread_ptr }`.

Dirent format scritto in `buf`:
```
[u8  kind: 0=Reg, 1=Dir, 2=Device]
[u8  reserved]
[u16 name_len]
[u64 size]      // 0 per Dir/Device
[u8; name_len name]
```

12 byte header + name. Multiplo di 1 (no padding).

Dispatch:
```rust
SuspendReason::ReadDir { path, buf_ptr, buf_len, nread_ptr } => {
    let entries = crate::vfs::readdir(&path).await.unwrap_or_default();
    let mut out = Vec::new();
    for e in entries {
        let name_bytes = e.name.as_bytes();
        let kind_byte = match e.kind {
            VfsKind::Reg => 0u8,
            VfsKind::Dir => 1u8,
            VfsKind::Device => 2u8,
        };
        let size = match crate::vfs::stat(&format!("{}/{}", path, e.name)).await {
            Ok(s) => s.size,
            Err(_) => 0,
        };
        out.push(kind_byte);
        out.push(0u8);
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(name_bytes);
    }
    let n = out.len().min(buf_len);
    self.write_to_memory(buf_ptr, &out[..n]).ok();
    self.write_u32(nread_ptr, n as u32).ok();
    0
}
```

## shell.wasm internal design

```rust
fn main() {
    // 1. Boot script
    if let Ok(script) = std::fs::read_to_string("/etc/init.sh") {
        for line in script.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            run_command(line);
        }
        println!("shell: init.sh complete");
    }
    // 2. Interactive loop (mai raggiunto in run-test; QEMU manuale)
    loop {
        print_prompt();
        match read_line() {
            Some(line) => {
                if line.trim() == "exit" { return; }
                run_command(&line);
            }
            None => return,
        }
    }
}

fn run_command(line: &str) -> i32 {
    let argv: Vec<&str> = line.split_whitespace().collect();
    if argv.is_empty() { return 0; }
    match argv[0] {
        "cd"   => builtin_cd(&argv),
        "pwd"  => builtin_pwd(),
        "exit" => std::process::exit(0),
        "help" => builtin_help(),
        cmd    => exec_external(cmd, &argv),
    }
}

fn exec_external(cmd: &str, argv: &[&str]) -> i32 {
    // PATH lookup: tries direct path first, then /bin/<cmd>.wasm
    let candidates = if cmd.contains('/') {
        vec![cmd.to_string()]
    } else {
        vec![format!("/bin/{}.wasm", cmd)]
    };
    for path in &candidates {
        if let Some(code) = try_exec(path, argv) {
            return code;
        }
    }
    eprintln!("shell: {}: not found", cmd);
    127
}

fn try_exec(path: &str, argv: &[&str]) -> Option<i32> {
    // ruos_exec custom syscall via extern "C"
    unsafe {
        let path_bytes = path.as_bytes();
        let mut argv_buf = encode_argv(argv);
        let mut exit_code: i32 = 0;
        let errno = ruos_exec(
            path_bytes.as_ptr() as u32,
            path_bytes.len() as u32,
            argv_buf.as_ptr() as u32,
            argv_buf.len() as u32,
            &mut exit_code as *mut i32 as u32,
        );
        if errno == 0 { Some(exit_code) } else { None }
    }
}
```

### Builtin `cd`

`shell.wasm` mantiene una `static mut CWD: String` (oppure `RefCell<String>`).
`cd <path>` modifica CWD. `pwd` la stampa. CWD parte da `/`.

Resolve relativo: se argv path inizia con `/` → absolute, sennò
`CWD + "/" + path`. Implementato in shell.wasm, kernel non sa nulla
di CWD.

### Builtin `help`

Stampa list di builtin + di `/bin/*.wasm` (readdir).

## Line editor (interactive)

YAGNI Step 11. Minimal:
- Read char-by-char via `stdin`
- Echo char to stdout
- `\n` → end line, run command
- `\x08` (backspace) → cancel last char (rewrite + cursor)
- Tutto il resto → append a buffer

Arrow keys, history, tab-completion: Step 12 (PTY).

## External `.wasm`: `user/ls/`, `user/cat/`, `user/echo/`

Ognuno è un crate `wasm32-wasip1`.

### `ls.wasm`

```rust
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| "/".to_string());
    // Custom syscall ruos_readdir
    let mut buf = vec![0u8; 4096];
    let mut nread: u32 = 0;
    let errno = unsafe { ruos_readdir(
        path.as_ptr() as u32, path.len() as u32,
        buf.as_mut_ptr() as u32, buf.len() as u32,
        &mut nread as *mut u32 as u32
    ) };
    if errno != 0 { eprintln!("ls: {}: errno {}", path, errno); std::process::exit(1); }
    let mut offset = 0;
    while offset < nread as usize {
        let kind = buf[offset]; offset += 2;
        let name_len = u16::from_le_bytes([buf[offset], buf[offset+1]]) as usize; offset += 2;
        let size = u64::from_le_bytes(buf[offset..offset+8].try_into().unwrap()); offset += 8;
        let name = core::str::from_utf8(&buf[offset..offset+name_len]).unwrap();
        offset += name_len;
        let mark = match kind { 1 => "/", 2 => "@", _ => "" };
        println!("{} {:8} {}{}", kind_str(kind), size, name, mark);
    }
}
fn kind_str(k: u8) -> &'static str { match k { 0 => "REG", 1 => "DIR", 2 => "DEV", _ => "???" } }
```

### `cat.wasm`

```rust
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = match args.get(1) { Some(p) => p, None => {
        eprintln!("cat: missing path"); std::process::exit(1);
    } };
    match std::fs::read_to_string(path) {
        Ok(s) => print!("{}", s),
        Err(e) => { eprintln!("cat: {}: {}", path, e); std::process::exit(1); }
    }
}
```

### `echo.wasm`

```rust
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("{}", args.join(" "));
}
```

## `/etc/init.sh` (Limine module)

```
# ruos boot script
echo hello from shell.wasm
pwd
ls /bin
echo init.sh: end
```

## Drop `kbd_echo_task` (F7 Step 10.5 fix)

`kernel/src/executor/mod.rs`: elimina `kbd_echo_task` def + spawn.
shell.wasm diventa l'unico consumer di keyboard via FD 0 (KbdReadChar
path tramite Stdin) o /dev/console (ConsoleFile path).

Per Step 11 demo non-interattivo (init.sh) la keyboard non viene mai
letta dalla shell — irrilevante che path canonico sia. Step 12 (PTY)
risolverà la confusione.

## Boot smoke contract

Test `make run-test` HELLO: `shell: init.sh complete`.

Sequence atteso (intercalato a `async tick=N`):

```
ruos: net init ok addr=127.0.0.1/8
ruos: real ping-pong (no preload)
... (boot logs)
ruos: executor up
ruos: async tick=0
hello from shell.wasm
/
REG    51234 echo.wasm
REG    87654 cat.wasm
REG    73210 ls.wasm
REG  1240000 shell.wasm
init.sh: end
shell: init.sh complete
ruos: shell.wasm exited cleanly
```

## Componenti / file toccati (riepilogo)

**Nuovi:**
- `kernel/src/wasm/host/proc.rs` (ruos_exec + ruos_readdir host fns)
- `user/shell/{Cargo.toml,src/main.rs}`
- `user/ls/{Cargo.toml,src/main.rs}`
- `user/cat/{Cargo.toml,src/main.rs}`
- `user/echo/{Cargo.toml,src/main.rs}`
- `user-bin/shell.wasm`, `ls.wasm`, `cat.wasm`, `echo.wasm`
- `user-bin/init.sh` (boot script file)

**Modificati:**
- `kernel/src/wasm/suspend.rs` (+`SuspendReason::Exec`, `ReadDir`)
- `kernel/src/wasm/fiber.rs` (+dispatch arms; +`set_args` + lifecycle args_get/args_sizes_get integration)
- `kernel/src/wasm/state.rs` (`args` populated from Fiber::set_args)
- `kernel/src/wasm/host/lifecycle.rs` (args_sizes_get + args_get reali, non più zero stubs)
- `kernel/src/wasm/host/mod.rs` (link proc module)
- `kernel/src/executor/mod.rs` (drop kbd_echo_task; spawn solo shell.wasm; drop init/server/client auto-spawn?)
- `kernel/src/main.rs` — verifica se cambia
- `limine.conf` (+5 moduli: init.sh + 4 wasms)
- `user/Cargo.toml` (workspace members)
- `Makefile` (build 4 wasm + copia init.sh)

**Eliminato:**
- `kbd_echo_task` definition + spawn

**Decisione aperta — auto-spawn Step 10 demo wasm?**:
Options:
- A: Drop spawn di init/server/client. Lanciali a mano da shell.
- B: Lascia spawn di init/server/client per non perdere i smoke
     attivi (real ping-pong test).
- C: Sposta lo "smoke step 10" dentro init.sh: `init.sh` lancia
     `init.wasm`, `server.wasm`, `client.wasm` via shell exec.

C è cleanest ma init.wasm/server.wasm/client.wasm assumono lo spawn
parallelo (server+client concorrenti). Da shell.wasm sequenziale =
`exec` blocking. Romperebbe il demo Step 10.

Quindi B per Step 11: keep auto-spawn esistenti. Aggiungiamo shell come
4° task spawnato. shell.wasm gira accanto a init/server/client al boot,
non sopra di loro.

## Decomposizione 3 task

1. **T1 — Host fns + lifecycle args**: `wasm/host/proc.rs` con
   `ruos_exec` + `ruos_readdir`. SuspendReason variants. Fiber dispatch
   arms + `set_args` API. `args_sizes_get`/`args_get` lifecycle host
   fns reali (popolano da RuntimeState.args). Smoke: nuovo `init.wasm`
   stampa argv[0] (`init.wasm`). Sentinel intermedio: `init.wasm:
   argv0=/init.wasm`. (oppure manteniamo `init.wasm: clock_rand ok`
   come T1 sentinel — nessuna feature visibile in T1).
2. **T2 — External tools**: nuovi 4 user crates (shell, ls, cat,
   echo). Build wasm32-wasip1, copia user-bin/, limine.conf modules.
   shell.wasm: only boot-script path (no interactive). init.sh.
   Smoke: HELLO → `shell: init.sh complete`.
3. **T3 — Cleanup**: drop kbd_echo_task. Final regression check
   (Step 10 demo intact, Step 10.5 sentinel intact, Step 11 sentinel
   raggiunto). Smoke unchanged.

## Out of scope

- Pipes (`|`), redirections (`>`, `<`) → Step 12 (PTY)
- Background jobs (`&`) → richiede proc_fork
- Tab completion, history, arrow keys → Step 12
- `env` / environment variables (`$VAR`) → Step 12
- Globbing (`*`)
- Subshells (`(...)`)
- Job control / signals
- `bash.wasm` upgrade — quando ABI WASIX completa
- Multi-line commands

## Open points (decisi in implementazione)

- Path encoding `argv` in wasm memory: layout dichiarato sopra (count
  + offsets + lengths + data). Versione alternativa POSIX-like
  (`*const *const u8` ordine null-terminated) testata se rust wasm
  binding compatibile.
- `cwd` model: shell-internal `static mut String`, kernel non sa nulla.
  cat/ls/echo non hanno cwd: usano absolute path. Per Step 11 sufficient.
- ruos_exec stdin/stdout/stderr inheritance: child RuntimeState parte
  con FD 0=Stdin / 1,2=StdoutConsole come default. Stessi default del
  padre. Inheritance vera (FD table dup) → Step 12+.
