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
}

fn main() {
    *CWD.lock().unwrap() = "/".to_string();
    if let Ok(script) = fs::read_to_string("/etc/init.sh") {
        for line in script.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            run_command(line);
        }
        println!("shell: init.sh complete");
    } else {
        println!("shell: /etc/init.sh not found");
    }
    loop {
        print_prompt();
        match read_line() {
            Some(line) => {
                let line = line.trim();
                if line == "exit" { return; }
                if line.is_empty() { continue; }
                run_command(line);
            }
            None => return,
        }
    }
}

fn print_prompt() {
    let cwd = CWD.lock().unwrap().clone();
    print!("ruos:{}$ ", cwd);
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn read_line() -> Option<String> {
    use std::io::Read;
    let mut buf = String::new();
    loop {
        let mut byte = [0u8; 1];
        match std::io::stdin().read(&mut byte) {
            Ok(0) => return if buf.is_empty() { None } else { Some(buf) },
            Ok(_) => {
                let c = byte[0];
                if c == b'\n' || c == b'\r' {
                    println!();
                    return Some(buf);
                }
                if c == 8 || c == 127 {
                    if !buf.is_empty() {
                        buf.pop();
                        print!("\x08 \x08");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                    continue;
                }
                buf.push(c as char);
                let mut tmp = [0u8; 4];
                let s = (c as char).encode_utf8(&mut tmp);
                print!("{}", s);
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            Err(_) => return None,
        }
    }
}

fn run_command(line: &str) {
    let argv: Vec<&str> = line.split_whitespace().collect();
    if argv.is_empty() { return; }
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
        s
    } else {
        let mut s = cwd.clone();
        if !s.ends_with('/') { s.push('/'); }
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
    let mut blob: Vec<u8> = Vec::with_capacity(table_size + argv.iter().map(|s| s.len()).sum::<usize>());
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
            path_bytes.as_ptr() as u32, path_bytes.len() as u32,
            blob.as_ptr() as u32, blob.len() as u32,
            &mut exit_code as *mut i32 as u32,
        )
    };
    if errno == 0 { Some(exit_code) } else { None }
}
