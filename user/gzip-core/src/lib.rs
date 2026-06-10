//! gzip-core — formato gzip (RFC 1952) + deflate via miniz_oxide.
//! Logica condivisa dai tool gzip/gunzip/zcat.

mod cli;
mod crc32;
mod format;

pub use cli::run_cli;
pub use format::{compress, decompress, GzError};
