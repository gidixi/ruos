use crate::vfs::error::VfsError;
use crate::vfs::file::{FileImpl, OpenFlags};

pub trait FileSystem {
    async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError>;
    async fn create(&self, path: &[&str]) -> Result<(), VfsError>;
    async fn unlink(&self, path: &[&str]) -> Result<(), VfsError>;
}

// Concrete filesystems are introduced in Task 2 (Tmpfs). Same placeholder
// pattern as FileImpl.
pub enum FsImpl {
    Placeholder,
}

impl FsImpl {
    pub async fn open(&self, _path: &[&str], _flags: OpenFlags) -> Result<FileImpl, VfsError> {
        Err(VfsError::Other)
    }
    pub async fn create(&self, _path: &[&str]) -> Result<(), VfsError> {
        Err(VfsError::Other)
    }
    pub async fn unlink(&self, _path: &[&str]) -> Result<(), VfsError> {
        Err(VfsError::Other)
    }
}
