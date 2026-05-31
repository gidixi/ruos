/// Fuel-exhaustion integration test binary.
///
/// Pure compute loop that makes NO host (WASI) calls after startup.
/// The wasmi fuel meter sees a budget-2_000_000_000 run with zero
/// refuel events and terminates the instance with OutOfFuel, which the
/// kernel runner translates to exit code 137 and logs:
///
///   wasm: task killed (fuel exhausted)
///
/// This wasm is not meant to be useful — its only purpose is to confirm
/// that the kernel's fuel metering kills runaway tasks without hanging.
fn main() {
    let mut x: u64 = 1;
    loop {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        core::hint::black_box(x);
    }
}
