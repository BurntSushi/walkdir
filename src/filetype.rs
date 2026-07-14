use std::fs;

/// File type yielded by a [`crate::DirEntry`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileType(fs::FileType);

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

impl From<fs::FileType> for FileType {
    fn from(file_type: fs::FileType) -> FileType {
        FileType(file_type)
    }
}
