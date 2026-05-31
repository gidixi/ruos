/// Mirror of `kernel/src/wasm/host/mem.rs::check_bounds`.
///
/// This is a host-native (std) copy of the single audited guest-memory
/// boundary function.  It is kept intentionally minimal (6 lines, no
/// wasmi types) so that it can be compiled and fuzz-tested on the host
/// without pulling in the kernel's no_std / wasmi / custom-target stack.
///
/// SYNC RULE: if `check_bounds` changes in `kernel/src/wasm/host/mem.rs`,
/// update this copy in lock-step.  The function signature and semantics
/// must remain identical.

const EINVAL: i32 = 28;
const EFAULT: i32 = 21;

fn check_bounds(ptr: i32, len: i32, size: u64) -> Result<(usize, usize), i32> {
    if ptr < 0 || len < 0 { return Err(EINVAL); }
    let end = (ptr as u64).checked_add(len as u64).ok_or(EFAULT)?;
    if end > size { return Err(EFAULT); }
    Ok((ptr as usize, len as usize))
}

fn main() {
    println!("Running check_bounds adversarial tests…");
    test_negative_ptr_or_len_rejected();
    test_past_end_rejected();
    test_zero_len_ok_at_boundary();
    test_in_range_ok();
    test_never_panics_exhaustive_small();
    println!("All tests passed.");
}

fn test_negative_ptr_or_len_rejected() {
    assert_eq!(check_bounds(-1, 0, 100), Err(EINVAL),   "ptr=-1");
    assert_eq!(check_bounds(0, -1, 100), Err(EINVAL),   "len=-1");
    assert_eq!(check_bounds(i32::MIN, 0, 100), Err(EINVAL), "ptr=i32::MIN");
    assert_eq!(check_bounds(0, i32::MIN, 100), Err(EINVAL), "len=i32::MIN");
    println!("  [PASS] negative_ptr_or_len_rejected");
}

fn test_past_end_rejected() {
    assert_eq!(check_bounds(90, 20, 100), Err(EFAULT), "90+20>100");
    assert_eq!(check_bounds(100, 1, 100), Err(EFAULT), "100+1>100");
    assert_eq!(check_bounds(i32::MAX, i32::MAX, 10), Err(EFAULT), "MAX+MAX>10");
    println!("  [PASS] past_end_rejected");
}

fn test_zero_len_ok_at_boundary() {
    assert_eq!(check_bounds(100, 0, 100), Ok((100, 0)), "zero-len at exact end");
    assert_eq!(check_bounds(0, 0, 0),     Ok((0, 0)),   "zero-len zero-size");
    println!("  [PASS] zero_len_ok_at_boundary");
}

fn test_in_range_ok() {
    assert_eq!(check_bounds(0, 100, 100), Ok((0, 100)),   "full range");
    assert_eq!(check_bounds(50, 50, 100), Ok((50, 50)),   "mid range");
    println!("  [PASS] in_range_ok");
}

/// Exhaustive grid over small values — the function must NEVER panic.
fn test_never_panics_exhaustive_small() {
    for ptr in -5i32..200 {
        for len in -5i32..200 {
            for size in 0u64..200 {
                let _ = check_bounds(ptr, len, size);
            }
        }
    }
    println!("  [PASS] never_panics_exhaustive_small (205×205×200 = 8 405 000 calls)");
}
