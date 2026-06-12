//! mtstress — MT Fase 2 stress: 4 thread incrementano un contatore sotto un
//! `std::sync::Mutex` conteso (futex wait/notify reali) e il main fa join.
//! Valore ESATTO = prova atomicità + coerenza della memoria condivisa.
//!
//! `mtstress trap`: un thread abortisce (trap wasm `unreachable`) — il kernel
//! avvelena e uccide l'intero gruppo (exit 134); la riga `UNREACHABLE` non
//! deve MAI stamparsi e la shell deve sopravvivere.

use std::sync::{Arc, Mutex};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("trap") {
        let h = std::thread::spawn(|| {
            std::process::abort();
        });
        let _ = h.join(); // parcheggia il main: muore col kill-group
        println!("UNREACHABLE");
        return;
    }
    let n_threads = 4;
    let m = 100_000u64;
    let counter = Arc::new(Mutex::new(0u64));
    let hs: Vec<_> = (0..n_threads)
        .map(|_| {
            let c = counter.clone();
            std::thread::spawn(move || {
                for _ in 0..m {
                    *c.lock().unwrap() += 1;
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    // poll_oneoff (clock subscription): il fiber si parcheggia, core libero.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let v = *counter.lock().unwrap();
    assert_eq!(v, n_threads as u64 * m);
    println!("STRESS_MT_OK count={}", v);
}
