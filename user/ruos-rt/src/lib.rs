//! ruos tool runtime: sync the guest libc cwd from the `PWD` env var the kernel
//! injects at exec, so a tool's RELATIVE paths resolve against the shell's
//! working directory. The kernel resolves WASI fd paths against "/" (stateless),
//! so cwd must live in the guest's libc — this is where it's seeded.
//!
//! Call `ruos_rt::init()` as the first line of `main()` (this also forces the
//! crate to link). A `.init_array` ctor alone does NOT link when nothing in the
//! binary references the crate (verified: the dependency's object is dropped).

/// Seed the libc cwd from `PWD`. No-op if `PWD` is unset or invalid. Idempotent.
pub fn init() {
    if let Ok(pwd) = std::env::var("PWD") {
        if !pwd.is_empty() {
            let _ = std::env::set_current_dir(&pwd);
        }
    }
}
