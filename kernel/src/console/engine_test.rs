//! In-kernel self-test della console engine. Stampa un marker su seriale,
//! asserito da `make run-console-test`. Ogni task aggiunge asserzioni qui.

use crate::kprintln;

/// Esegue tutte le asserzioni. Stampa `CONSOLE_TEST: OK` se tutte passano,
/// altrimenti `CONSOLE_TEST: FAIL:<id>` al primo fallimento e ritorna.
pub fn run() {
    if let Err(id) = run_inner() {
        kprintln!("CONSOLE_TEST: FAIL:{}", id);
        return;
    }
    kprintln!("CONSOLE_TEST: OK");
}

fn run_inner() -> Result<(), u32> {
    // T1: harness vivo.
    check(1, 1 + 1 == 2)?;
    Ok(())
}

#[inline]
fn check(id: u32, cond: bool) -> Result<(), u32> {
    if cond { Ok(()) } else { Err(id) }
}
