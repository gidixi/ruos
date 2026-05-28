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

use crate::vfs::tmpfs::TmpfsFile;
use crate::vfs::devices::{ConsoleFile, NullFile, ZeroFile};

pub enum FileImpl {
    Tmp(TmpfsFile),
    Console(ConsoleFile),
    Null(NullFile),
    Zero(ZeroFile),
}

impl FileImpl {
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        match self {
            FileImpl::Tmp(f)     => f.read(buf).await,
            FileImpl::Console(f) => f.read(buf).await,
            FileImpl::Null(f)    => f.read(buf).await,
            FileImpl::Zero(f)    => f.read(buf).await,
        }
    }
    pub async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        match self {
            FileImpl::Tmp(f)     => f.write(buf).await,
            FileImpl::Console(f) => f.write(buf).await,
            FileImpl::Null(f)    => f.write(buf).await,
            FileImpl::Zero(f)    => f.write(buf).await,
        }
    }
    pub async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError> {
        match self {
            FileImpl::Tmp(f)     => f.seek(off, whence).await,
            FileImpl::Console(f) => f.seek(off, whence).await,
            FileImpl::Null(f)    => f.seek(off, whence).await,
            FileImpl::Zero(f)    => f.seek(off, whence).await,
        }
    }
}
