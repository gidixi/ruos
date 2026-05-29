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
    async fn mkdir(&self, path: &[&str]) -> Result<(), VfsError>;
    async fn rmdir(&self, path: &[&str]) -> Result<(), VfsError>;
    async fn rename(&self, src: &[&str], dst: &[&str]) -> Result<(), VfsError>;
}

use crate::vfs::tmpfs::Tmpfs;
use crate::vfs::fat32::Fat32Fs;

pub enum FsImpl {
    Tmpfs(Tmpfs),
    Fat32(Fat32Fs),
}

impl FsImpl {
    pub async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError> {
        match self {
            FsImpl::Tmpfs(t) => t.open(path, flags).await,
            FsImpl::Fat32(f) => f.open(path, flags).await,
        }
    }
    pub async fn create(&self, path: &[&str]) -> Result<(), VfsError> {
        match self {
            FsImpl::Tmpfs(t) => t.create(path).await,
            FsImpl::Fat32(f) => f.create(path).await,
        }
    }
    pub async fn unlink(&self, path: &[&str]) -> Result<(), VfsError> {
        match self {
            FsImpl::Tmpfs(t) => t.unlink(path).await,
            FsImpl::Fat32(f) => f.unlink(path).await,
        }
    }
    pub async fn readdir(&self, path: &[&str]) -> Result<Vec<VfsDirent>, VfsError> {
        match self {
            FsImpl::Tmpfs(t) => t.readdir(path).await,
            FsImpl::Fat32(f) => f.readdir(path).await,
        }
    }
    pub async fn stat(&self, path: &[&str]) -> Result<VfsStat, VfsError> {
        match self {
            FsImpl::Tmpfs(t) => t.stat(path).await,
            FsImpl::Fat32(f) => f.stat(path).await,
        }
    }
    pub async fn mkdir(&self, path: &[&str]) -> Result<(), VfsError> {
        match self {
            FsImpl::Tmpfs(t) => <Tmpfs as FileSystem>::mkdir(t, path).await,
            FsImpl::Fat32(f) => <Fat32Fs as FileSystem>::mkdir(f, path).await,
        }
    }
    pub async fn rmdir(&self, path: &[&str]) -> Result<(), VfsError> {
        match self {
            FsImpl::Tmpfs(t) => <Tmpfs as FileSystem>::rmdir(t, path).await,
            FsImpl::Fat32(f) => <Fat32Fs as FileSystem>::rmdir(f, path).await,
        }
    }
    pub async fn rename(&self, src: &[&str], dst: &[&str]) -> Result<(), VfsError> {
        match self {
            FsImpl::Tmpfs(t) => <Tmpfs as FileSystem>::rename(t, src, dst).await,
            FsImpl::Fat32(f) => <Fat32Fs as FileSystem>::rename(f, src, dst).await,
        }
    }
}
