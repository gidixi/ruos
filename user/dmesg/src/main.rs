//! dmesg — dump kernel ring buffer.
//!
//! Flags:
//!   -l <level>   filter: only level >= <info|warn|err>
//!   -t <tag>     filter: only this tag (repeatable, OR over tags)
//!   -g <substr>  filter: message contains <substr> (case-sensitive)
//!   -T           strip "[T+x.xs] " timestamp prefix
//!   -h, --help   usage
//!
//! Multiple filters combine with AND. Lines that don't parse fall through
//! only via -g (matched against the whole raw line); -l/-t exclude them.

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn dmesg(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Level { Info = 0, Warn = 1, Err = 2 }

impl Level {
    fn parse(s: &str) -> Option<Level> {
        // case-insensitive
        let lo: String = s.chars().map(|c| c.to_ascii_lowercase()).collect();
        match lo.as_str() {
            "info"          => Some(Level::Info),
            "warn"          => Some(Level::Warn),
            "err" | "error" => Some(Level::Err),
            _ => None,
        }
    }
}

/// Parsed view over a log line.
///
/// Format: `[T+<sec>.<ms>s] <LEVEL> <tag>  <message>` (tag and message
/// separated by two spaces). `body_start` is the byte offset within the
/// raw line where the post-`]` content begins (i.e. after `"] "`), used
/// to strip the timestamp when -T is set.
struct Parsed<'a> {
    level: Option<Level>,
    tag: Option<&'a str>,
    message: Option<&'a str>,
    /// byte index in `raw` of the first char after the "[T+...s] " prefix.
    /// `None` if no timestamp prefix was found.
    body_start: Option<usize>,
}

fn parse_line(raw: &str) -> Parsed<'_> {
    // Strip trailing \n for parsing (we keep raw intact for output).
    let line = raw.strip_suffix('\n').unwrap_or(raw);

    // Must start with "[T+" and contain "s] " to be considered structured.
    if !line.starts_with("[T+") {
        return Parsed { level: None, tag: None, message: None, body_start: None };
    }
    let close = match line.find("s] ") {
        Some(i) => i + 3, // index of first char after "s] "
        None => return Parsed { level: None, tag: None, message: None, body_start: None },
    };
    let rest = &line[close..];

    // rest = "<LEVEL> <tag>  <message>"
    let sp1 = match rest.find(' ') {
        Some(i) => i,
        None => return Parsed { level: None, tag: None, message: None, body_start: Some(close) },
    };
    let level_str = &rest[..sp1];
    let level = Level::parse(level_str);
    if level.is_none() {
        // Unknown level token — treat as unstructured but keep body_start
        // so -T still strips the timestamp.
        return Parsed { level: None, tag: None, message: None, body_start: Some(close) };
    }

    let after_level = &rest[sp1 + 1..];
    // tag is up to the first "  " (two spaces); message is after it.
    let (tag, message) = match after_level.find("  ") {
        Some(i) => (&after_level[..i], &after_level[i + 2..]),
        None => (after_level, ""),
    };

    Parsed {
        level,
        tag: Some(tag),
        message: Some(message),
        body_start: Some(close),
    }
}

fn print_usage() {
    let u = "\
usage: dmesg [-l <level>] [-t <tag>]... [-g <substr>] [-T] [-h]
  -l <level>   show only lines at level >= <info|warn|err> (case-insensitive)
  -t <tag>     show only lines with this tag (repeatable, OR over tags)
  -g <substr>  show only lines whose message contains <substr>
  -T           strip the [T+x.xs] timestamp prefix
  -h, --help   show this help
";
    print!("{}", u);
}

fn arg_value(flag: &str, it: &mut std::vec::IntoIter<String>) -> String {
    match it.next() {
        Some(v) => v,
        None => {
            eprintln!("dmesg: missing argument for {}", flag);
            std::process::exit(2);
        }
    }
}

fn main() {
    // ---- argv ----
    let argv: Vec<String> = std::env::args().collect();
    let mut min_level: Option<Level> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut grep: Option<String> = None;
    let mut strip_ts = false;

    let mut it = argv.into_iter();
    let _prog = it.next();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => { print_usage(); return; }
            "-T" => { strip_ts = true; }
            "-l" => {
                let v = arg_value("-l", &mut it);
                match Level::parse(&v) {
                    Some(l) => min_level = Some(l),
                    None => {
                        eprintln!("dmesg: invalid level '{}' (expected info|warn|err)", v);
                        std::process::exit(2);
                    }
                }
            }
            "-t" => {
                let v = arg_value("-t", &mut it);
                tags.push(v);
            }
            "-g" => {
                let v = arg_value("-g", &mut it);
                grep = Some(v);
            }
            other => {
                eprintln!("dmesg: unknown option: {}", other);
                std::process::exit(2);
            }
        }
    }

    // ---- fetch ring buffer ----
    let mut buf = vec![0u8; 32 * 1024];
    let mut used: u32 = 0;
    let errno = unsafe {
        dmesg(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno != 0 {
        eprintln!("dmesg: errno {}", errno);
        std::process::exit(1);
    }
    let n = (used as usize).min(buf.len());
    let text = match std::str::from_utf8(&buf[..n]) {
        Ok(s) => std::borrow::Cow::Borrowed(s),
        Err(_) => std::borrow::Cow::Owned(String::from_utf8_lossy(&buf[..n]).into_owned()),
    };

    // Fast path: no filters, no -T → identical to old behaviour.
    let any_filter = min_level.is_some() || !tags.is_empty() || grep.is_some();
    if !any_filter && !strip_ts {
        print!("{}", text);
        return;
    }

    // ---- per-line filtering ----
    // split_inclusive keeps the trailing '\n' on each chunk, matching the
    // old `print!` behaviour where the source lines already carry '\n'.
    for line in text.split_inclusive('\n') {
        let p = parse_line(line);

        // Level filter: -l excludes unparseable lines.
        if let Some(min) = min_level {
            match p.level {
                Some(l) if l >= min => {}
                _ => continue,
            }
        }

        // Tag filter: -t excludes unparseable lines; OR across tags.
        if !tags.is_empty() {
            match p.tag {
                Some(t) if tags.iter().any(|x| x == t) => {}
                _ => continue,
            }
        }

        // Grep filter: prefer message body, else fall back to whole raw line.
        if let Some(ref g) = grep {
            let hay = p.message.unwrap_or(line);
            if !hay.contains(g.as_str()) { continue; }
        }

        // Output (optionally stripping the timestamp prefix).
        if strip_ts {
            if let Some(bs) = p.body_start {
                print!("{}", &line[bs..]);
                continue;
            }
        }
        print!("{}", line);
    }
}
