use crate::vfs::error::VfsError;
use crate::vfs::file::{FileImpl, OpenFlags};

pub trait FileSystem {
    async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError>;
    async fn create(&self, path: &[&str]) -> Result<(), VfsError>;
    async fn unlink(&self, path: &[&str]) -> Result<(), VfsError>;
}

use crate::vfs::tmpfs::Tmpfs;

pub enum FsImpl {
    Tmpfs(Tmpfs),
}

impl FsImpl {
    pub async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError> {
        match self { FsImpl::Tmpfs(t) => t.open(path, flags).await }
    }
    pub async fn create(&self, path: &[&str]) -> Result<(), VfsError> {
        match self { FsImpl::Tmpfs(t) => t.create(path).await }
    }
    pub async fn unlink(&self, path: &[&str]) -> Result<(), VfsError> {
        match self { FsImpl::Tmpfs(t) => t.unlink(path).await }
    }
}
