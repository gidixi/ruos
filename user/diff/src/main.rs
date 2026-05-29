//! Naive line-by-line diff. Not LCS-based — adequate for small text files.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("diff: usage: diff <a> <b>");
        std::process::exit(2);
    }
    let a = match std::fs::read_to_string(&args[1]) {
        Ok(s) => s,
        Err(e) => { eprintln!("diff: {}: {}", args[1], e); std::process::exit(2); }
    };
    let b = match std::fs::read_to_string(&args[2]) {
        Ok(s) => s,
        Err(e) => { eprintln!("diff: {}: {}", args[2], e); std::process::exit(2); }
    };
    let al: Vec<&str> = a.lines().collect();
    let bl: Vec<&str> = b.lines().collect();
    let n = al.len().max(bl.len());
    let mut differ = false;
    for i in 0..n {
        let av = al.get(i).copied();
        let bv = bl.get(i).copied();
        if av != bv {
            differ = true;
            println!("@@ line {} @@", i + 1);
            if let Some(s) = av { println!("< {}", s); }
            if let Some(s) = bv { println!("> {}", s); }
        }
    }
    std::process::exit(if differ { 1 } else { 0 });
}
