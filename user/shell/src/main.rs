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

    fn readdir(
        path_ptr: u32, path_len: u32,
        buf_ptr: u32, buf_len: u32,
        out_used: u32,
    ) -> i32;

    fn tcgetattr(fd: i32, ptr: u32) -> i32;
    fn tcsetattr(fd: i32, action: i32, ptr: u32) -> i32;
    fn chdir(path_ptr: u32, path_len: u32) -> i32;
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

fn tab_complete(prefix: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = vec![
        "cd".into(),
        "pwd".into(),
        "exit".into(),
        "help".into(),
    ];
    let mut buf = vec![0u8; 4096];
    let mut n: u32 = 0;
    let p = "/bin";
    let errno = unsafe {
        readdir(
            p.as_ptr() as u32,
            p.len() as u32,
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            &mut n as *mut u32 as u32,
        )
    };
    if errno == 0 {
        let mut o = 0usize;
        while o + 12 <= n as usize {
            let nlen = u16::from_le_bytes([buf[o + 2], buf[o + 3]]) as usize;
            o += 12;
            if o + nlen > n as usize {
                break;
            }
            if let Ok(name) = std::str::from_utf8(&buf[o..o + nlen]) {
                if name.ends_with(".wasm") {
                    out.push(name.trim_end_matches(".wasm").to_string());
                }
            }
            o += nlen;
        }
    }
    let pref = std::str::from_utf8(prefix).unwrap_or("");
    out.retain(|c| c.starts_with(pref));
    out
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
                let candidates = tab_complete(&buf[token_start..cursor]);
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
    println!("\x1b[1;32mruos shell ready. type 'help' for builtins.\x1b[0m");

    let saved = save_and_raw();
    loop {
        let cwd = CWD.lock().unwrap().clone();
        let prompt = format!("ruos:{}$ ", cwd);
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
    let argv: Vec<&str> = line.split_whitespace().collect();
    if argv.is_empty() {
        return;
    }
    match argv[0] {
        "cd"   => builtin_cd(&argv),
        "pwd"  => builtin_pwd(),
        "exit" => std::process::exit(0),
        "help" => builtin_help(),
        cmd    => { let _ = exec_external(cmd, &argv); }
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
    println!("ruos shell builtins: cd <path>, pwd, exit, help");
    println!("external: try 'ls /bin' to list available .wasm");
}

fn exec_external(cmd: &str, argv: &[&str]) -> i32 {
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
