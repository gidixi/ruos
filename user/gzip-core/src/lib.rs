//! gzip-core — formato gzip (RFC 1952) + deflate via miniz_oxide.
//! Logica condivisa dai tool gzip/gunzip/zcat e dal kernel (unpack bin.bgz).
//! `no_std` di default sotto kernel; feature `std` abilita la CLI userland.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod crc32;
mod format;
pub mod pack;

#[cfg(feature = "std")]
mod cli;
#[cfg(feature = "std")]
pub use cli::run_cli;

pub use format::{compress, decompress, GzError};
