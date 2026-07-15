#[cfg(walkdir_unix)]
use crate::os::unix::FileType as OsFileType;
#[cfg(windows)]
use crate::os::windows::FileType as OsFileType;
#[cfg(not(any(walkdir_unix, windows)))]
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

    /// Returns true if this entry is a symbolic link to a directory.
    #[cfg(windows)]
    pub fn is_symlink_dir(&self) -> bool {
        self.0.is_symlink_dir()
    }

    /// Returns true if this entry is a symbolic link to a file.
    #[cfg(windows)]
    pub fn is_symlink_file(&self) -> bool {
        self.0.is_symlink_file()
    }
}

impl From<OsFileType> for FileType {
    fn from(file_type: OsFileType) -> FileType {
        FileType(file_type)
    }
}
