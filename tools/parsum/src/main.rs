//! parsum — MT Fase 2 end-to-end test: somma parallela con rayon su
//! `wasm32-wasip1-threads`. Stampa `PARSUM_OK threads=N sum=S speedup_x100=X`;
//! `threads` prova che RAYON_NUM_THREADS (iniettato dal kernel) è arrivato,
//! `sum` uguale tra serial e parallel prova la coerenza della memoria shared.

use rayon::prelude::*;

fn main() {
    let n: u64 = 50_000_000;
    let t0 = std::time::Instant::now();
    let serial: u64 = (0..n).map(|x| x ^ (x >> 3)).sum();
    let t_ser = t0.elapsed();
    let t1 = std::time::Instant::now();
    let parallel: u64 = (0..n).into_par_iter().map(|x| x ^ (x >> 3)).sum();
    let t_par = t1.elapsed();
    assert_eq!(serial, parallel);
    let speedup_x100 = (t_ser.as_micros() * 100 / t_par.as_micros().max(1)) as u64;
    println!(
        "PARSUM_OK threads={} sum={} speedup_x100={}",
        rayon::current_num_threads(),
        parallel,
        speedup_x100
    );
}
