//! wt-readline — eco di stdin per provare lo stdin del runtime Wasmtime.
//! Legge righe (cooked line discipline del PTY) e stampa `STDIN_ECHO:<riga>`.
//! Esce su EOF (Ctrl-D / hangup) o sulla riga `quit`.

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    println!("WT_READLINE_READY");
    let _ = out.flush();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break, // I/O error → termina
        };
        if line.trim() == "quit" {
            println!("WT_READLINE_BYE");
            break;
        }
        println!("STDIN_ECHO:{}", line);
        let _ = out.flush();
    }
}
