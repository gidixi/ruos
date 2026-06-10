//! gunzip — = gzip -d (decomprime). Vedi gzip-core::run_cli.

fn main() {
    ruos_rt::init(); // sync libc cwd from PWD so relative paths honor the shell's cwd
    gzip_core::run_cli(true, false);
}
