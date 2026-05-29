use alloc::string::String;
use alloc::vec::Vec;

use crate::vfs::error::VfsError;
use crate::vfs::file::{FileImpl, OpenFlags};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsKind { Dir, Reg, Device }

#[derive(Debug, Clone)]
pub struct VfsDirent {
    pub name: String,
    pub kind: VfsKind,
}

#[derive(Debug, Clone, Copy)]
pub struct VfsStat {
    pub kind: VfsKind,
    pub size: u64,
}

pub trait FileSystem {
    async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError>;
    async fn create(&self, path: &[&str]) -> Result<(), VfsError>;
    async fn unlink(&self, path: &[&str]) -> Result<(), VfsError>;
    async fn readdir(&self, path: &[&str]) -> Result<Vec<VfsDirent>, VfsError>;
    async fn stat(&self, path: &[&str]) -> Result<VfsStat, VfsError>;
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
    pub async fn readdir(&self, path: &[&str]) -> Result<Vec<VfsDirent>, VfsError> {
        match self { FsImpl::Tmpfs(t) => t.readdir(path).await }
    }
    pub async fn stat(&self, path: &[&str]) -> Result<VfsStat, VfsError> {
        match self { FsImpl::Tmpfs(t) => t.stat(path).await }
    }
}
