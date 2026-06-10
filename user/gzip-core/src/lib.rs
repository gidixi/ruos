//! gzip-core — formato gzip (RFC 1952) + deflate via miniz_oxide.
//! Logica condivisa dai tool gzip/gunzip/zcat.

mod crc32;
mod format;

pub use format::{compress, GzError};
