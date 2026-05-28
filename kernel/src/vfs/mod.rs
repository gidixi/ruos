//! Async VFS API + tmpfs in-RAM filesystem + device files.

pub mod error;
pub mod path;
pub mod file;
pub mod fs;
pub mod fd;
pub mod block_on;

pub use block_on::block_on;
pub use error::VfsError;
pub use file::{Fd, OpenFlags, Whence};

// Real API (open/close/read/write/seek/init/mount) lands in Task 3.
