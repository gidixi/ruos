use core::fmt;

#[derive(Debug, Copy, Clone)]
pub enum VfsError {
    NotFound,
    AlreadyExists,
    NotDirectory,
    IsDirectory,
    BadFd,
    NotPermitted,
    InvalidPath,
    Invalid,
    Closed,
    NoSpace,
    IoError,
    Unsupported,
    Other,
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            VfsError::NotFound      => "not found",
            VfsError::AlreadyExists => "already exists",
            VfsError::NotDirectory  => "not directory",
            VfsError::IsDirectory   => "is directory",
            VfsError::BadFd         => "bad fd",
            VfsError::NotPermitted  => "not permitted",
            VfsError::InvalidPath   => "invalid path",
            VfsError::Invalid       => "invalid",
            VfsError::Closed        => "closed",
            VfsError::NoSpace       => "no space",
            VfsError::IoError       => "io error",
            VfsError::Unsupported   => "unsupported",
            VfsError::Other         => "other",
        };
        f.write_str(s)
    }
}
