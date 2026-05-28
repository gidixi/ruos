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
    Closed,
    NoSpace,
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
            VfsError::Closed        => "closed",
            VfsError::NoSpace       => "no space",
            VfsError::Other         => "other",
        };
        f.write_str(s)
    }
}
