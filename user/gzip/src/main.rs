//! gzip — comprime (default). Vedi gzip-core::run_cli per flag e semantica.

fn main() {
    ruos_rt::init(); // sync libc cwd from PWD so relative paths honor the shell's cwd
    gzip_core::run_cli(false, false);
}
