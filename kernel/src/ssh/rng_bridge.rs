//! Bridge `getrandom::register_custom_getrandom!` to our kernel CSPRNG.
//!
//! getrandom uses this for ed25519-dalek key generation, ephemeral DH
//! scalars in sunset's KEX, and any other randomness inside the SSH stack.

use getrandom::register_custom_getrandom;

fn ruos_getrandom(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    crate::rng::fill(buf);
    Ok(())
}

register_custom_getrandom!(ruos_getrandom);
