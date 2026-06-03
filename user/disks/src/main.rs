//! disks -- list the SATA disks (index, model, size) as a clean table.
//!
//! Thin front-end over the kernel host fn `ruos::sata_list`, which formats one
//! disk per line as `<idx>\t<model>\t<size>` (the model is a `{:?}`-quoted
//! IDENTIFY string). Pick a disk here, then `install <n>` to install onto it.
//!
//! Usage:
//!   disks    list the SATA disks (wipes nothing)

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn sata_list(ptr: i32, cap: i32) -> i32;
}

fn main() {
    let mut buf = [0u8; 1024];
    let n = unsafe { sata_list(buf.as_mut_ptr() as i32, buf.len() as i32) };
    if n < 0 {
        eprintln!("disks: listing too large");
        std::process::exit(1);
    }
    if n == 0 {
        println!("disks: no SATA disks found");
        return;
    }
    println!("IDX  MODEL                             SIZE");
    let text = String::from_utf8_lossy(&buf[..n as usize]);
    for line in text.lines() {
        let mut f = line.split('\t'); // idx \t model \t size
        let idx = f.next().unwrap_or("?");
        // The model arrives as a {:?} debug string with surrounding quotes.
        let model = f.next().unwrap_or("?").trim_matches('"');
        let size = f.next().unwrap_or("?");
        println!("{:>3}  {:<32}  {}", idx, model, size);
    }
}
