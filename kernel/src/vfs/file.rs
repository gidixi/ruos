use crate::vfs::error::VfsError;

pub type Fd = u32;

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone)]
    pub struct OpenFlags: u32 {
        const READ     = 1 << 0;
        const WRITE    = 1 << 1;
        const CREATE   = 1 << 2;
        const TRUNCATE = 1 << 3;
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Whence { Set, Cur, End }

pub trait File {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError>;
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError>;
    async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError>;
}

// Concrete file types are introduced in Task 2 (tmpfs + devices).
// FileImpl variants below will be filled in then; for Task 1 we declare an
// empty placeholder so other modules can name the type. The variants are
// constructed only by Task 2 code, so no real values flow through yet.
pub enum FileImpl {
    Placeholder,
}

impl FileImpl {
    pub async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Err(VfsError::Other)
    }
    pub async fn write(&mut self, _buf: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::Other)
    }
    pub async fn seek(&mut self, _off: i64, _whence: Whence) -> Result<u64, VfsError> {
        Err(VfsError::Other)
    }
}
