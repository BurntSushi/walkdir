use std::fmt;
use std::fs::OpenOptions;
use std::io;
use std::mem;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, RawHandle};
use std::path::Path;
use std::time::SystemTime;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    FileAttributeTagInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};

use crate::os::windows::{intervals_to_system_time, time_as_u64};

/// The name-surrogate bit set on reparse tags for symlinks and junctions.
const IO_REPARSE_TAG_NAME_SURROGATE_BIT: u32 = 0x2000_0000;

/// Metadata for a file, queried from an open handle.
#[derive(Clone)]
pub struct Metadata {
    info: BY_HANDLE_FILE_INFORMATION,
    reparse_tag: u32,
}

impl Metadata {
    /// The raw file attributes, as in `dwFileAttributes`.
    pub fn file_attributes(&self) -> u32 {
        self.info.dwFileAttributes
    }

    /// The file type.
    pub fn file_type(&self) -> FileType {
        FileType::from_attr(self.file_attributes(), self.reparse_tag)
    }

    /// Returns true if this file is marked hidden.
    pub fn is_hidden(&self) -> bool {
        self.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
    }

    /// The creation time, when available.
    pub fn created(&self) -> io::Result<SystemTime> {
        let intervals = time_as_u64(&self.info.ftCreationTime);
        if intervals == 0 {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "creation time is not available on this platform currently",
            ))
        } else {
            Ok(intervals_to_system_time(intervals))
        }
    }

    /// The last access time, when available.
    pub fn accessed(&self) -> io::Result<SystemTime> {
        let intervals = time_as_u64(&self.info.ftLastAccessTime);
        if intervals == 0 {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "last access time is not available on this platform currently",
            ))
        } else {
            Ok(intervals_to_system_time(intervals))
        }
    }

    /// The last modification time, when available.
    pub fn modified(&self) -> io::Result<SystemTime> {
        let intervals = time_as_u64(&self.info.ftLastWriteTime);
        if intervals == 0 {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "last write time is not available on this platform currently",
            ))
        } else {
            Ok(intervals_to_system_time(intervals))
        }
    }

    /// The file size in bytes.
    pub fn len(&self) -> u64 {
        ((self.info.nFileSizeHigh as u64) << 32)
            | (self.info.nFileSizeLow as u64)
    }

    /// The number of hard links to the file.
    pub fn number_of_links(&self) -> u64 {
        self.info.nNumberOfLinks as u64
    }

    /// The serial number of the volume the file resides on.
    pub fn volume_serial_number(&self) -> u64 {
        self.info.dwVolumeSerialNumber as u64
    }

    /// The 64-bit file index identifying the file within its volume.
    pub fn file_index(&self) -> u64 {
        ((self.info.nFileIndexHigh as u64) << 32)
            | (self.info.nFileIndexLow as u64)
    }
}

/// File type information discoverable from a Windows directory entry or handle.
///
/// Note that this does not include all possible file types on Windows.
/// Instead, this only differentiates between directories, regular files and
/// symlinks. Additional file type information (such as whether a file handle
/// is a socket) can only be retrieved via the `GetFileType` winapi routines.
/// A safe wrapper for it is
/// [available in the `winapi-util` crate](https://docs.rs/winapi-util/*/x86_64-pc-windows-msvc/winapi_util/file/fn.typ.html).
#[derive(Clone, Copy)]
pub struct FileType {
    attr: u32,
    reparse_tag: u32,
}

// Compare only the classification (directory/file/symlink and its target kind)
// rather than the raw attributes, since two plain files that differ only in
// HIDDEN or READONLY are the same type.
impl PartialEq for FileType {
    fn eq(&self, other: &FileType) -> bool {
        self.discriminant() == other.discriminant()
    }
}

impl Eq for FileType {}

impl std::hash::Hash for FileType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.discriminant().hash(state);
    }
}

impl fmt::Debug for FileType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let human = if self.is_file() {
            "File"
        } else if self.is_dir() {
            "Directory"
        } else if self.is_symlink_file() {
            "Symbolic Link (File)"
        } else if self.is_symlink_dir() {
            "Symbolic Link (Directory)"
        } else {
            "Unknown"
        };
        write!(f, "FileType({human})")
    }
}

impl FileType {
    /// Create a file type from its raw winapi components.
    ///
    /// `attr`  should be a file attribute bitset, corresponding to the
    /// `dwFileAttributes` member of file information structs.
    ///
    /// `reparse_tag` should be a valid reparse tag value when the
    /// `FILE_ATTRIBUTE_REPARSE_POINT` bit is set in `attr`. If the bit isn't
    /// set or if the tag is not available, then the tag can be any value.
    pub fn from_attr(attr: u32, reparse_tag: u32) -> FileType {
        FileType { attr, reparse_tag }
    }

    /// Convert this file type to the platform independent file type.
    pub fn into_api(self) -> crate::FileType {
        crate::FileType::from(self)
    }

    /// Returns true if this file type is a regular file.
    ///
    /// This corresponds to any file that is neither a symlink nor a directory.
    pub fn is_file(&self) -> bool {
        !self.is_dir() && !self.is_symlink()
    }

    /// Returns true if this file type is a directory.
    ///
    /// This corresponds to any file that has the `FILE_ATTRIBUTE_DIRECTORY`
    /// attribute and is not a symlink.
    pub fn is_dir(&self) -> bool {
        self.attr & FILE_ATTRIBUTE_DIRECTORY != 0 && !self.is_symlink()
    }

    /// Returns true if this file type is a symlink. This could be a symlink
    /// to a directory or to a file. To distinguish between them, use
    /// `is_symlink_file` and `is_symlink_dir`.
    ///
    /// This corresponds to any file that has a surrogate reparse point.
    pub fn is_symlink(&self) -> bool {
        self.reparse_tag()
            .is_some_and(|tag| tag & IO_REPARSE_TAG_NAME_SURROGATE_BIT != 0)
    }

    /// Returns true if this file type is a symlink to a file.
    ///
    /// This corresponds to any file that has a surrogate reparse point and
    /// is not a symlink to a directory.
    pub fn is_symlink_file(&self) -> bool {
        !self.is_symlink_dir() && self.is_symlink()
    }

    /// Returns true if this file type is a symlink to a directory.
    ///
    /// This corresponds to any file that has a surrogate reparse point and has
    /// the `FILE_ATTRIBUTE_DIRECTORY` attribute.
    pub fn is_symlink_dir(&self) -> bool {
        self.attr & FILE_ATTRIBUTE_DIRECTORY != 0 && self.is_symlink()
    }

    /// A small classification code so equal-classified types compare equal.
    fn discriminant(&self) -> u8 {
        if self.is_symlink() {
            if self.is_symlink_dir() {
                3
            } else {
                2
            }
        } else if self.is_dir() {
            1
        } else {
            0
        }
    }

    fn reparse_tag(&self) -> Option<u32> {
        if self.attr & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            Some(self.reparse_tag)
        } else {
            None
        }
    }
}

/// Open a handle and query metadata, following a trailing symlink.
pub fn stat<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    let file = OpenOptions::new()
        // Neither read nor write permissions are needed.
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    statat(file.as_raw_handle())
}

/// Like stat but does not follow a trailing symlink.
pub fn lstat<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    let file = OpenOptions::new()
        // Neither read nor write permissions are needed.
        .access_mode(0)
        .custom_flags(
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .open(path)?;
    statat(file.as_raw_handle())
}

fn statat(handle: RawHandle) -> io::Result<Metadata> {
    // SAFETY: handle is a valid open handle for the duration of this call and
    // info is fully written by GetFileInformationByHandle on success.
    let info: BY_HANDLE_FILE_INFORMATION = unsafe {
        let mut info = mem::zeroed();
        let res = GetFileInformationByHandle(handle as HANDLE, &mut info);
        if res == 0 {
            return Err(io::Error::last_os_error());
        }
        info
    };
    let reparse_tag =
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            reparse_tag(handle)?
        } else {
            0
        };
    Ok(Metadata { info, reparse_tag })
}

/// Read the reparse tag via `FileAttributeTagInfo`.
fn reparse_tag(handle: RawHandle) -> io::Result<u32> {
    // SAFETY: handle is valid and the buffer size matches the info class.
    let info: FILE_ATTRIBUTE_TAG_INFO = unsafe {
        let mut info: FILE_ATTRIBUTE_TAG_INFO = mem::zeroed();
        let res = GetFileInformationByHandleEx(
            handle as HANDLE,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        );
        if res == 0 {
            return Err(io::Error::last_os_error());
        }
        info
    };
    Ok(info.ReparseTag)
}
