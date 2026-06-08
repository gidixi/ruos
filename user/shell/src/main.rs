use std::fs;
use std::sync::Mutex;

static CWD: Mutex<String> = Mutex::new(String::new());

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn exec(
        path_ptr: u32, path_len: u32,
        argv_ptr: u32, argv_len: u32,
        exit_code_ptr: u32,
    ) -> i32;

    fn exec_pipeline(buf_ptr: u32, buf_len: u32, exit_code_ptr: u32) -> i32;

    fn readdir(
        path_ptr: u32, path_len: u32,
        buf_ptr: u32, buf_len: u32,
        out_used: u32,
    ) -> i32;

    fn tcgetattr(fd: i32, ptr: u32) -> i32;
    fn tcsetattr(fd: i32, action: i32, ptr: u32) -> i32;
    fn chdir(path_ptr: u32, path_len: u32) -> i32;
    fn poweroff() -> !;
    fn reboot() -> !;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    cc: [u8; 32],
    ispeed: u32,
    ospeed: u32,
}

impl Termios {
    fn zero() -> Self {
        Self {
            iflag: 0,
            oflag: 0,
            cflag: 0,
            lflag: 0,
            cc: [0; 32],
            ispeed: 0,
            ospeed: 0,
        }
    }
}

const ICANON: u32 = 0o0002;
const ECHO: u32   = 0o0010;
const ISIG: u32   = 0o0001;

static HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn save_and_raw() -> Termios {
    let mut saved = Termios::zero();
    unsafe { tcgetattr(0, &mut saved as *mut _ as u32); }
    let mut raw = saved;
    raw.lflag &= !(ICANON | ECHO | ISIG);
    unsafe { tcsetattr(0, 0, &raw as *const _ as u32); }
    saved
}

fn restore(t: &Termios) {
    unsafe { tcsetattr(0, 0, t as *const _ as u32); }
}

fn read_byte() -> Option<u8> {
    use std::io::Read;
    let mut b = [0u8; 1];
    match std::io::stdin().read(&mut b) {
        Ok(1) => Some(b[0]),
        _ => None,
    }
}

fn redraw_line(prompt: &str, buf: &[u8], cursor: usize) {
    use std::io::Write;
    print!("\r\x1b[2K{}{}", prompt, std::str::from_utf8(buf).unwrap_or(""));
    if cursor < buf.len() {
        print!("\x1b[{}D", buf.len() - cursor);
    }
    std::io::stdout().flush().ok();
}

/// Read a directory and return (name, is_dir) entries.
fn readdir_entries(path: &str) -> Vec<(String, bool)> {
    let mut buf = vec![0u8; 8192];
    let mut n: u32 = 0;
    let errno = unsafe {
        readdir(
            path.as_ptr() as u32,
            path.len() as u32,
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            &mut n as *mut u32 as u32,
        )
    };
    let mut out = Vec::new();
    if errno != 0 { return out; }
    let mut o = 0usize;
    while o + 12 <= n as usize {
        let kind = buf[o];
        let nlen = u16::from_le_bytes([buf[o + 2], buf[o + 3]]) as usize;
        o += 12;
        if o + nlen > n as usize { break; }
        if let Ok(name) = std::str::from_utf8(&buf[o..o + nlen]) {
            out.push((name.to_string(), kind == 1)); // kind 1 = Dir
        }
        o += nlen;
    }
    out
}

/// Command completion: builtins + /bin/*.wasm + /mnt/bin/*.wasm (stripped
/// suffix). `/bin` is the live tmpfs; `/mnt/bin` is where the command tools
/// live on the installed SSD's data partition, so installed-SSD tools also
/// tab-complete. The `/mnt/bin` listing is best-effort: `readdir_entries`
/// returns an empty list when it is absent (the live ISO has no `/mnt/bin`),
/// so completion never breaks. Names present in both dirs are de-duplicated.
fn complete_command(prefix: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = vec![
        "cd".into(),
        "pwd".into(),
        "exit".into(),
        "help".into(),
        "poweroff".into(),
        "reboot".into(),
        "source".into(),
    ];
    for dir in ["/bin", "/mnt/bin"] {
        for (name, _) in readdir_entries(dir) {
            let cmd = if name.ends_with(".wasm") {
                Some(name.trim_end_matches(".wasm").to_string())
            } else if name.ends_with(".cwasm") {
                Some(name.trim_end_matches(".cwasm").to_string())
            } else {
                None
            };
            if let Some(cmd) = cmd {
                if !out.contains(&cmd) {
                    out.push(cmd);
                }
            }
        }
    }
    let pref = std::str::from_utf8(prefix).unwrap_or("");
    out.retain(|c| c.starts_with(pref));
    out
}

/// Path completion: split token at last `/` → (dir, name_prefix).
/// Readdir dir (or "." if no slash), filter entries by name_prefix,
/// return candidates with directory prefix preserved + trailing "/" on
/// dirs. Each candidate starts with the original prefix bytes so the
/// suffix-insert logic in `read_line_raw` works correctly.
fn complete_path(prefix: &[u8]) -> Vec<String> {
    let s = std::str::from_utf8(prefix).unwrap_or("");
    let (dir, name_prefix) = match s.rfind('/') {
        Some(idx) => (&s[..idx + 1], &s[idx + 1..]),
        None => ("", s),
    };
    let listing = if dir.is_empty() { "." } else {
        // strip trailing / for readdir (it tolerates both, but be tidy)
        dir.trim_end_matches('/')
    };
    let listing = if listing.is_empty() { "/" } else { listing };
    let mut out = Vec::new();
    for (name, is_dir) in readdir_entries(listing) {
        if name.starts_with(name_prefix) {
            let mut c = String::from(dir);
            c.push_str(&name);
            if is_dir { c.push('/'); }
            out.push(c);
        }
    }
    out
}

/// Top-level completion dispatcher.
/// `first_token` = true when completing the command name (no whitespace
/// before the cursor in the current logical line). Otherwise the prefix
/// is treated as a filesystem path.
fn tab_complete(first_token: bool, prefix: &[u8]) -> Vec<String> {
    if first_token {
        complete_command(prefix)
    } else {
        complete_path(prefix)
    }
}

fn read_line_raw(prompt: &str) -> Option<String> {
    use std::io::Write;
    let mut buf: Vec<u8> = Vec::new();
    let mut cursor: usize = 0;
    let mut hist_idx: Option<usize> = None;
    let history_snapshot = HISTORY.lock().unwrap().clone();
    redraw_line(prompt, &buf, cursor);
    loop {
        let b = read_byte()?;
        match b {
            b'\n' | b'\r' => {
                println!();
                if !buf.is_empty() {
                    HISTORY.lock().unwrap().push(String::from_utf8_lossy(&buf).into_owned());
                }
                return Some(String::from_utf8_lossy(&buf).into_owned());
            }
            0x7F | 0x08 => {
                if cursor > 0 {
                    buf.remove(cursor - 1);
                    cursor -= 1;
                    redraw_line(prompt, &buf, cursor);
                }
            }
            0x01 => {
                cursor = 0;
                redraw_line(prompt, &buf, cursor);
            }
            0x05 => {
                cursor = buf.len();
                redraw_line(prompt, &buf, cursor);
            }
            0x0C => {
                print!("\x1b[2J\x1b[H");
                std::io::stdout().flush().ok();
                redraw_line(prompt, &buf, cursor);
            }
            0x03 => {
                println!("^C");
                return Some(String::new());
            }
            b'\t' => {
                let start_rev = (0..cursor)
                    .rev()
                    .take_while(|&i| !buf[i].is_ascii_whitespace())
                    .count();
                let token_start = cursor - start_rev;
                // First token if nothing but whitespace precedes the
                // current token. Subsequent tokens get path completion.
                let first_token = (0..token_start)
                    .all(|i| buf[i].is_ascii_whitespace());
                let candidates = tab_complete(first_token, &buf[token_start..cursor]);
                if candidates.len() == 1 {
                    let comp = candidates[0].as_bytes();
                    let prefix_len = cursor - token_start;
                    if comp.len() > prefix_len {
                        let suffix = &comp[prefix_len..];
                        for (i, &c) in suffix.iter().enumerate() {
                            buf.insert(cursor + i, c);
                        }
                        cursor += suffix.len();
                        redraw_line(prompt, &buf, cursor);
                    }
                } else if candidates.len() > 1 {
                    println!();
                    for c in &candidates {
                        print!("{}  ", c);
                    }
                    println!();
                    redraw_line(prompt, &buf, cursor);
                }
            }
            0x1B => {
                let _ = read_byte()?; // '['
                let arrow = read_byte()?;
                match arrow {
                    b'A' => {
                        // Up — older history
                        let idx = match hist_idx {
                            None => history_snapshot.len(),
                            Some(i) => i,
                        };
                        if idx > 0 {
                            hist_idx = Some(idx - 1);
                            buf.clear();
                            buf.extend_from_slice(history_snapshot[idx - 1].as_bytes());
                            cursor = buf.len();
                            redraw_line(prompt, &buf, cursor);
                        }
                    }
                    b'B' => {
                        // Down — newer history
                        if let Some(i) = hist_idx {
                            let next = i + 1;
                            if next < history_snapshot.len() {
                                hist_idx = Some(next);
                                buf.clear();
                                buf.extend_from_slice(history_snapshot[next].as_bytes());
                                cursor = buf.len();
                            } else {
                                hist_idx = None;
                                buf.clear();
                                cursor = 0;
                            }
                            redraw_line(prompt, &buf, cursor);
                        }
                    }
                    b'C' => {
                        // Right
                        if cursor < buf.len() {
                            cursor += 1;
                            print!("\x1b[C");
                            std::io::stdout().flush().ok();
                        }
                    }
                    b'D' => {
                        // Left
                        if cursor > 0 {
                            cursor -= 1;
                            print!("\x1b[D");
                            std::io::stdout().flush().ok();
                        }
                    }
                    _ => {}
                }
            }
            c if c >= 0x20 => {
                buf.insert(cursor, c);
                cursor += 1;
                redraw_line(prompt, &buf, cursor);
            }
            _ => {}
        }
    }
}

fn main() {
    *CWD.lock().unwrap() = "/".to_string();

    // `--no-init` (passed by the SSH server when spawning a session shell)
    // skips the boot script + banner so an SSH login lands straight at a
    // clean prompt instead of replaying /etc/init.sh.
    let no_init = std::env::args().any(|a| a == "--no-init");

    if !no_init {
        if let Ok(script) = fs::read_to_string("/etc/init.sh") {
            for line in script.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                run_command(line);
            }
            println!("shell: init.sh complete");
        } else {
            println!("shell: /etc/init.sh not found");
        }

        std::thread::sleep(std::time::Duration::from_millis(1000));
        use std::io::Write;
        print!("\x1b[2J\x1b[H");
        std::io::stdout().flush().ok();
    }
    println!("\x1b[1;32mruOS shell ready. type 'help' for builtins.\x1b[0m");

    let saved = save_and_raw();
    loop {
        let cwd = CWD.lock().unwrap().clone();
        let prompt = format!("ruOS:{}$ ", cwd);
        match read_line_raw(&prompt) {
            Some(line) => {
                let trimmed = line.trim();
                if trimmed == "exit" {
                    break;
                }
                if trimmed.is_empty() {
                    continue;
                }
                run_command(trimmed);
            }
            None => break,
        }
    }
    restore(&saved);
}

fn run_command(line: &str) {
    // Detect a pipeline: split on `|` outside quotes BEFORE tokenising.
    let segments = split_pipeline(line);
    if segments.len() > 1 {
        let _ = run_pipeline(&segments);
        return;
    }

    // Single command — existing path unchanged.
    let argv: Vec<&str> = line.split_whitespace().collect();
    if argv.is_empty() {
        return;
    }
    match argv[0] {
        "cd"       => builtin_cd(&argv),
        "pwd"      => builtin_pwd(),
        "exit"     => std::process::exit(0),
        "help"     => builtin_help(),
        "poweroff" => unsafe { poweroff() },
        "reboot"   => unsafe { reboot() },
        "source" | "." => builtin_source(&argv),
        cmd        => { let _ = exec_external(cmd, &argv); }
    }
}

/// Read a shell script from path and run each non-empty, non-comment
/// line through `run_command`. Same semantics as the boot loop on
/// `/etc/init.sh`. Recursive (a sourced script can `source` another).
fn builtin_source(argv: &[&str]) {
    let path = match argv.get(1) {
        Some(p) => *p,
        None => {
            eprintln!("source: missing path");
            return;
        }
    };
    match fs::read_to_string(path) {
        Ok(script) => {
            for line in script.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                run_command(line);
            }
        }
        Err(e) => eprintln!("source: {}: {}", path, e),
    }
}

fn builtin_pwd() {
    println!("{}", CWD.lock().unwrap());
}

fn builtin_cd(argv: &[&str]) {
    let target = argv.get(1).copied().unwrap_or("/");
    // Kernel-side CWD update (so child processes spawned via exec inherit
    // the right directory). Path resolution (relative, .., .) is handled
    // by the host fn.
    let errno = unsafe { chdir(target.as_ptr() as u32, target.len() as u32) };
    if errno != 0 {
        eprintln!("cd: {}: errno {}", target, errno);
        return;
    }
    // Mirror kernel CWD locally so the prompt and history look right.
    let mut cwd = CWD.lock().unwrap();
    let new = if target.starts_with('/') {
        target.to_string()
    } else if target == "." {
        cwd.clone()
    } else if target == ".." {
        let mut s = cwd.clone();
        if s.len() > 1 {
            if let Some(idx) = s.rfind('/') {
                s.truncate(idx.max(1));
            }
        }
        if s.is_empty() { s.push('/'); }
        s
    } else {
        let mut s = cwd.clone();
        if !s.ends_with('/') {
            s.push('/');
        }
        s.push_str(target);
        s
    };
    *cwd = new;
}

fn builtin_help() {
    println!("ruos shell builtins:");
    println!("  cd <path>   change directory");
    println!("  pwd         print working directory");
    println!("  exit        exit shell");
    println!("  help        this list");
    println!("  poweroff    halt the system");
    println!("  reboot      restart the system");
    println!("  source <p>  run script (alias: . <p>)");
    println!("external: try 'ls /bin' to list available .wasm");
}

/// Resolve a command name to an absolute path (e.g. `ls` → `/bin/ls.wasm`).
///
/// When the command already contains a `/` it is taken verbatim (let the
/// kernel report the error). Otherwise the bare command is looked up as a
/// `.wasm` in `/bin` first, then `/mnt/bin` — returning the FIRST path that
/// actually exists. On the installed SSD the command tools live on the data
/// partition (`/mnt/bin`), not the slim ESP, so this lets them load on-demand
/// from the FAT. The live ISO (all tools in `/bin` tmpfs) is unaffected since
/// `/bin` is tried first. Returns `None` when neither candidate exists, so the
/// caller prints "command not found".
fn resolve_path(cmd: &str) -> Option<String> {
    if cmd.contains('/') {
        return Some(cmd.to_string());
    }
    // Try `.wasm` (wasmi) first, then `.cwasm` (Wasmtime AOT), in /bin then
    // /mnt/bin. A tool present only as `.cwasm` runs on the Wasmtime runtime.
    for dir in ["/bin", "/mnt/bin"] {
        for ext in ["wasm", "cwasm"] {
            let p = format!("{}/{}.{}", dir, cmd, ext);
            if std::fs::metadata(&p).is_ok() {
                return Some(p);
            }
        }
    }
    None
}

fn exec_external(cmd: &str, argv: &[&str]) -> i32 {
    match resolve_path(cmd) {
        Some(path) => {
            if let Some(code) = try_exec(&path, argv) {
                return code;
            }
            eprintln!("shell: {}: not found", cmd);
            127
        }
        None => {
            eprintln!("shell: {}: not found", cmd);
            127
        }
    }
}

fn try_exec(path: &str, argv: &[&str]) -> Option<i32> {
    let count = argv.len() as u32;
    let table_size = 4 + (argv.len() * 8);
    let mut blob: Vec<u8> = Vec::with_capacity(
        table_size + argv.iter().map(|s| s.len()).sum::<usize>(),
    );
    blob.extend_from_slice(&count.to_le_bytes());
    let mut data_offset = table_size as u32;
    for s in argv {
        blob.extend_from_slice(&data_offset.to_le_bytes());
        blob.extend_from_slice(&(s.len() as u32).to_le_bytes());
        data_offset += s.len() as u32;
    }
    for s in argv {
        blob.extend_from_slice(s.as_bytes());
    }
    let path_bytes = path.as_bytes();
    let mut exit_code: i32 = 0;
    let errno = unsafe {
        exec(
            path_bytes.as_ptr() as u32,
            path_bytes.len() as u32,
            blob.as_ptr() as u32,
            blob.len() as u32,
            &mut exit_code as *mut i32 as u32,
        )
    };
    if errno == 0 { Some(exit_code) } else { None }
}

/// Split `line` on `|` characters that are outside single/double quotes.
fn split_pipeline(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq) = (false, false);
    for c in line.chars() {
        match c {
            '\'' if !dq => { sq = !sq; cur.push(c); }
            '"'  if !sq => { dq = !dq; cur.push(c); }
            '|'  if !sq && !dq => { out.push(cur.trim().to_string()); cur.clear(); }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() { out.push(cur.trim().to_string()); }
    out
}

/// Serialize a list of pipeline stages into a binary blob understood by the
/// kernel's `exec_pipeline` host function.
///
/// Format (all integers little-endian u32):
///   nstages
///   per stage: path_len, path bytes, argc, (arg_len, arg bytes) × argc
fn serialize_pipeline(stages: &[(String, Vec<Vec<u8>>)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(stages.len() as u32).to_le_bytes());
    for (path, argv) in stages {
        let p = path.as_bytes();
        b.extend_from_slice(&(p.len() as u32).to_le_bytes());
        b.extend_from_slice(p);
        b.extend_from_slice(&(argv.len() as u32).to_le_bytes());
        for a in argv {
            b.extend_from_slice(&(a.len() as u32).to_le_bytes());
            b.extend_from_slice(a);
        }
    }
    b
}

/// Returns true if `name` is a shell builtin that cannot participate in a
/// pipeline (builtins do not fork, so they cannot be piped).
fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "pwd" | "exit" | "help" | "poweroff" | "reboot" | "source" | ".")
}

/// Run a multi-stage pipeline. Each segment has already been split on `|`.
/// Returns the exit code of the last stage, or 127 on setup errors.
fn run_pipeline(segments: &[String]) -> i32 {
    let mut stages: Vec<(String, Vec<Vec<u8>>)> = Vec::with_capacity(segments.len());
    for seg in segments {
        let argv: Vec<&str> = seg.split_whitespace().collect();
        if argv.is_empty() {
            eprintln!("shell: empty pipeline segment");
            return 1;
        }
        let cmd = argv[0];
        if is_builtin(cmd) {
            eprintln!("shell: builtin '{}' not allowed in a pipeline", cmd);
            return 1;
        }
        let path = match resolve_path(cmd) {
            Some(p) => p,
            None => {
                eprintln!("shell: {}: not found", cmd);
                return 127;
            }
        };
        let argv_bytes: Vec<Vec<u8>> = argv.iter().map(|s| s.as_bytes().to_vec()).collect();
        stages.push((path, argv_bytes));
    }
    let blob = serialize_pipeline(&stages);
    let mut exit_code: i32 = 0;
    let errno = unsafe {
        exec_pipeline(
            blob.as_ptr() as u32,
            blob.len() as u32,
            &mut exit_code as *mut i32 as u32,
        )
    };
    match errno {
        0 => exit_code,
        7 => { eprintln!("shell: pipeline too long (max 4)"); 1 }
        n => { eprintln!("shell: exec_pipeline errno {}", n); 1 }
    }
}
