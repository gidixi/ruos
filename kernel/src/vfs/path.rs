use alloc::vec::Vec;
use crate::vfs::error::VfsError;

/// Split an absolute path into canonical components. Rejects empty paths,
/// missing leading '/', empty components, '.', and '..'.
///
/// `"/"`         -> `Ok(vec![])`
/// `"/dev"`      -> `Ok(vec!["dev"])`
/// `"/dev/null"` -> `Ok(vec!["dev", "null"])`
pub fn split(path: &str) -> Result<Vec<&str>, VfsError> {
    let rest = path.strip_prefix('/').ok_or(VfsError::InvalidPath)?;
    let trimmed = rest.trim_end_matches('/');
    if trimmed.is_empty() { return Ok(Vec::new()); }
    let mut parts = Vec::new();
    for c in trimmed.split('/') {
        if c.is_empty() || c == "." || c == ".." {
            return Err(VfsError::InvalidPath);
        }
        parts.push(c);
    }
    Ok(parts)
}
