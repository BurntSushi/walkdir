#[cfg(walkdir_unix)]
use crate::os::unix::FileType as OsFileType;
#[cfg(not(walkdir_unix))]
use std::fs::FileType as OsFileType;

/// File type yielded by a [`crate::DirEntry`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileType(OsFileType);

impl FileType {
    /// Returns true if this entry is a regular file.
    pub fn is_file(&self) -> bool {
        self.0.is_file()
    }

    /// Returns true if this entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.0.is_dir()
    }

    /// Returns true if this entry is a symbolic link.
    pub fn is_symlink(&self) -> bool {
        self.0.is_symlink()
    }
}

impl From<OsFileType> for FileType {
    fn from(file_type: OsFileType) -> FileType {
        FileType(file_type)
    }
}
