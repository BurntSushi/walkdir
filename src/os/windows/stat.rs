use std::fmt;
use std::fs::OpenOptions;
use std::io;
use std::mem;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, RawHandle};
use std::path::Path;
use std::time::SystemTime;

use winapi::shared::minwindef::DWORD;
use winapi::um::fileapi::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
};
use winapi::um::winbase::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};

use crate::os::windows::{intervals_to_system_time, time_as_u64};

#[derive(Clone)]
pub struct Metadata {
    info: BY_HANDLE_FILE_INFORMATION,
    reparse_tag: DWORD,
}

impl Metadata {
    pub fn file_attributes(&self) -> u32 {
        self.info.dwFileAttributes
    }

    pub fn file_type(&self) -> FileType {
        FileType::from_attr(self.file_attributes(), self.reparse_tag)
    }

    pub fn is_hidden(&self) -> bool {
        use winapi::um::winnt::FILE_ATTRIBUTE_HIDDEN;
        self.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
    }

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

    pub fn len(&self) -> u64 {
        ((self.info.nFileSizeHigh as u64) << 32)
            | (self.info.nFileSizeLow as u64)
    }

    pub fn number_of_links(&self) -> u64 {
        self.info.nNumberOfLinks as u64
    }

    pub fn volume_serial_number(&self) -> u64 {
        self.info.dwVolumeSerialNumber as u64
    }

    pub fn file_index(&self) -> u64 {
        ((self.info.nFileIndexHigh as u64) << 32)
            | (self.info.nFileIndexLow as u64)
    }
}

/// File type information discoverable from the `FindNextFile` winapi routines.
///
/// Note that this does not include all possible file types on Windows.
/// Instead, this only differentiates between directories, regular files and
/// symlinks. Additional file type information (such as whether a file handle
/// is a socket) can only be retrieved via the `GetFileType` winapi routines.
/// A safe wrapper for it is
/// [available in the `winapi-util` crate](https://docs.rs/winapi-util/*/x86_64-pc-windows-msvc/winapi_util/file/fn.typ.html).
#[derive(Clone, Copy)]
pub struct FileType {
    attr: DWORD,
    reparse_tag: DWORD,
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
        write!(f, "FileType({})", human)
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
        use winapi::um::winnt::FILE_ATTRIBUTE_DIRECTORY;

        self.attr & FILE_ATTRIBUTE_DIRECTORY != 0 && !self.is_symlink()
    }

    /// Returns true if this file type is a symlink. This could be a symlink
    /// to a directory or to a file. To distinguish between them, use
    /// `is_symlink_file` and `is_symlink_dir`.
    ///
    /// This corresponds to any file that has a surrogate reparse point.
    pub fn is_symlink(&self) -> bool {
        use winapi::um::winnt::IsReparseTagNameSurrogate;

        self.reparse_tag().map_or(false, IsReparseTagNameSurrogate)
    }

    /// Returns true if this file type is a symlink to a file.
    ///
    /// This corresponds to any file that has a surrogate reparse point and
    /// is not a symlink to a directory.
    pub fn is_symlink_file(&self) -> bool {
        !self.is_symlink_dir() && self.is_symlink()
    }

    /// Returns true if this file type is a symlink to a file.
    ///
    /// This corresponds to any file that has a surrogate reparse point and has
    /// the `FILE_ATTRIBUTE_DIRECTORY` attribute.
    pub fn is_symlink_dir(&self) -> bool {
        use winapi::um::winnt::FILE_ATTRIBUTE_DIRECTORY;

        self.attr & FILE_ATTRIBUTE_DIRECTORY != 0 && self.is_symlink()
    }

    fn reparse_tag(&self) -> Option<DWORD> {
        use winapi::um::winnt::FILE_ATTRIBUTE_REPARSE_POINT;

        if self.attr & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            Some(self.reparse_tag)
        } else {
            None
        }
    }
}

pub fn stat<P: AsRef<Path>>(path: P) -> io::Result<Metadata> {
    let file = OpenOptions::new()
        // Neither read nor write permissions are needed.
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    statat(file.as_raw_handle())
}

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
    use winapi::um::winnt::FILE_ATTRIBUTE_REPARSE_POINT;

    let info: BY_HANDLE_FILE_INFORMATION = unsafe {
        let mut info = mem::zeroed();
        let res = GetFileInformationByHandle(handle, &mut info);
        if res == 0 {
            return Err(io::Error::last_os_error());
        }
        info
    };
    let reparse_tag =
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            get_reparse_tag(handle)?
        } else {
            0
        };
    Ok(Metadata { info, reparse_tag })
}

fn get_reparse_tag(handle: RawHandle) -> io::Result<DWORD> {
    use std::ptr;
    use winapi::ctypes::{c_uint, c_ushort};
    use winapi::um::ioapiset::DeviceIoControl;
    use winapi::um::winioctl::FSCTL_GET_REPARSE_POINT;
    use winapi::um::winnt::MAXIMUM_REPARSE_DATA_BUFFER_SIZE;

    #[repr(C)]
    struct REPARSE_DATA_BUFFER {
        ReparseTag: c_uint,
        ReparseDataLength: c_ushort,
        Reserved: c_ushort,
        rest: (),
    }

    let mut buf = [0; MAXIMUM_REPARSE_DATA_BUFFER_SIZE as usize];
    let res = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_GET_REPARSE_POINT,
            ptr::null_mut(),
            0,
            buf.as_mut_ptr() as *mut _,
            buf.len() as DWORD,
            &mut 0,
            ptr::null_mut(),
        )
    };
    if res == 0 {
        return Err(io::Error::last_os_error());
    }
    let data = buf.as_ptr() as *const REPARSE_DATA_BUFFER;
    Ok(unsafe { (*data).ReparseTag })
}
