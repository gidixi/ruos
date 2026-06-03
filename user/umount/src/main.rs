//! umount -- unmount a filesystem (e.g. /mnt).
//!
//! Unmounting `/mnt` drops the FsImpl behind it, which releases its backing
//! SATA port once no open file still holds a ref. That is what lets `install`
//! proceed onto a disk that M1 auto-mounted: the install `/mnt` guard refuses
//! while a filesystem is mounted there, so `umount /mnt` first, then `install`.
//!
//! Usage:
//!   umount <path>    unmount the filesystem mounted at <path>

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn umount(ptr: i32, len: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("umount: usage: umount <path>");
        std::process::exit(1);
    }
    let p = &args[1];
    let r = unsafe { umount(p.as_ptr() as i32, p.len() as i32) };
    match r {
        0 => println!("umount: {p} unmounted"),
        -2 => {
            eprintln!("umount: cannot unmount {p}");
            std::process::exit(1);
        }
        -3 => {
            eprintln!("umount: {p} busy (close open files first)");
            std::process::exit(1);
        }
        _ => {
            eprintln!("umount: {p} not mounted");
            std::process::exit(1);
        }
    }
}
