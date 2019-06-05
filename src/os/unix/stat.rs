use std::ffi::{CStr, CString, OsString};
use std::fmt;
use std::io;
use std::mem;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use libc;

#[cfg(not(any(target_os = "linux", target_os = "android",)))]
use libc::{fstatat as fstatat64, lstat as lstat64, stat as stat64};
#[cfg(any(target_os = "linux", target_os = "android",))]
use libc::{fstatat64, lstat64, stat64};

pub struct Metadata {
    stat: stat64,
}

impl Metadata {
    pub fn file_type(&self) -> FileType {
        FileType::from_stat_mode(self.stat.st_mode as u64)
    }

    pub fn len(&self) -> u64 {
        self.stat.st_size as u64
    }

    pub fn dev(&self) -> u64 {
        self.stat.st_dev
    }

    pub fn ino(&self) -> u64 {
        self.stat.st_ino
    }

    pub fn mode(&self) -> u64 {
        self.stat.st_mode as u64
    }

    pub fn permissions(&self) -> ! {
        unimplemented!()
    }
}

#[cfg(target_os = "netbsd")]
impl Metadata {
    pub fn modified(&self) -> io::Result<SystemTime> {
        let dur = Duration::new(
            self.stat.st_mtime as u64,
            self.stat.st_mtimensec as u32,
        );
        Ok(SystemTime::UNIX_EPOCH + dur)
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        let dur = Duration::new(
            self.stat.st_atime as u64,
            self.stat.st_atimensec as u32,
        );
        Ok(SystemTime::UNIX_EPOCH + dur)
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        let dur = Duration::new(
            self.stat.st_birthtime as u64,
            self.stat.st_birthtimensec as u32,
        );
        Ok(SystemTime::UNIX_EPOCH + dur)
    }
}

#[cfg(not(target_os = "netbsd"))]
impl Metadata {
    pub fn modified(&self) -> io::Result<SystemTime> {
        let dur = Duration::new(
            self.stat.st_mtime as u64,
            self.stat.st_mtime_nsec as u32,
        );
        Ok(SystemTime::UNIX_EPOCH + dur)
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        let dur = Duration::new(
            self.stat.st_atime as u64,
            self.stat.st_atime_nsec as u32,
        );
        Ok(SystemTime::UNIX_EPOCH + dur)
    }

    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "macos",
        target_os = "ios"
    ))]
    pub fn created(&self) -> io::Result<SystemTime> {
        let dur = Duration::new(
            self.stat.st_birthtime as u64,
            self.stat.st_birthtime_nsec as u32,
        );
        Ok(SystemTime::UNIX_EPOCH + dur)
    }

    #[cfg(not(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "macos",
        target_os = "ios"
    )))]
    pub fn created(&self) -> io::Result<SystemTime> {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "creation time is not available on this platform currently",
        ))
    }
}

/// One of seven possible file types on Unix.
#[derive(Clone, Copy)]
pub struct FileType(libc::mode_t);

impl fmt::Debug for FileType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let human = if self.is_file() {
            "File"
        } else if self.is_dir() {
            "Directory"
        } else if self.is_symlink() {
            "Symbolic Link"
        } else if self.is_block_device() {
            "Block Device"
        } else if self.is_char_device() {
            "Char Device"
        } else if self.is_fifo() {
            "FIFO"
        } else if self.is_socket() {
            "Socket"
        } else {
            "Unknown"
        };
        write!(f, "FileType({})", human)
    }
}

impl FileType {
    /// Create a new file type from a directory entry's type field.
    ///
    /// If the given type is not recognized or is `DT_UNKNOWN`, then `None`
    /// is returned.
    pub fn from_dirent_type(d_type: u8) -> Option<FileType> {
        Some(FileType(match d_type {
            libc::DT_REG => libc::S_IFREG,
            libc::DT_DIR => libc::S_IFDIR,
            libc::DT_LNK => libc::S_IFLNK,
            libc::DT_BLK => libc::S_IFBLK,
            libc::DT_CHR => libc::S_IFCHR,
            libc::DT_FIFO => libc::S_IFIFO,
            libc::DT_SOCK => libc::S_IFSOCK,
            libc::DT_UNKNOWN => return None,
            _ => return None, // wat?
        }))
    }

    /// Create a new file type from a stat's `st_mode` field.
    pub fn from_stat_mode(st_mode: u64) -> FileType {
        FileType(st_mode as libc::mode_t)
    }

    /// Convert this file type to the platform independent file type.
    pub fn into_api(self) -> crate::FileType {
        crate::FileType::from(self)
    }

    /// Returns true if this file type is a regular file.
    ///
    /// This corresponds to the `S_IFREG` value on Unix.
    pub fn is_file(&self) -> bool {
        self.0 & libc::S_IFMT == libc::S_IFREG
    }

    /// Returns true if this file type is a directory.
    ///
    /// This corresponds to the `S_IFDIR` value on Unix.
    pub fn is_dir(&self) -> bool {
        self.0 & libc::S_IFMT == libc::S_IFDIR
    }

    /// Returns true if this file type is a symbolic link.
    ///
    /// This corresponds to the `S_IFLNK` value on Unix.
    pub fn is_symlink(&self) -> bool {
        self.0 & libc::S_IFMT == libc::S_IFLNK
    }

    /// Returns true if this file type is a block device.
    ///
    /// This corresponds to the `S_IFBLK` value on Unix.
    pub fn is_block_device(&self) -> bool {
        self.0 & libc::S_IFMT == libc::S_IFBLK
    }

    /// Returns true if this file type is a character device.
    ///
    /// This corresponds to the `S_IFCHR` value on Unix.
    pub fn is_char_device(&self) -> bool {
        self.0 & libc::S_IFMT == libc::S_IFCHR
    }

    /// Returns true if this file type is a FIFO.
    ///
    /// This corresponds to the `S_IFIFO` value on Unix.
    pub fn is_fifo(&self) -> bool {
        self.0 & libc::S_IFMT == libc::S_IFIFO
    }

    /// Returns true if this file type is a socket.
    ///
    /// This corresponds to the `S_IFSOCK` value on Unix.
    pub fn is_socket(&self) -> bool {
        self.0 & libc::S_IFMT == libc::S_IFSOCK
    }
}

pub fn stat<P: Into<PathBuf>>(path: P) -> io::Result<Metadata> {
    let bytes = path.into().into_os_string().into_vec();
    stat_c(&CString::new(bytes)?)
}

pub fn stat_c(path: &CStr) -> io::Result<Metadata> {
    let mut stat: stat64 = unsafe { mem::zeroed() };
    let res = unsafe { stat64(path.as_ptr(), &mut stat) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(Metadata { stat })
    }
}

pub fn lstat<P: Into<PathBuf>>(path: P) -> io::Result<Metadata> {
    let bytes = path.into().into_os_string().into_vec();
    lstat_c(&CString::new(bytes)?)
}

pub fn lstat_c(path: &CStr) -> io::Result<Metadata> {
    let mut stat: stat64 = unsafe { mem::zeroed() };
    let res = unsafe { lstat64(path.as_ptr(), &mut stat) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(Metadata { stat })
    }
}

pub fn statat<N: Into<OsString>>(
    parent_dirfd: RawFd,
    name: N,
) -> io::Result<Metadata> {
    let bytes = name.into().into_vec();
    statat_c(parent_dirfd, &CString::new(bytes)?)
}

pub fn statat_c(parent_dirfd: RawFd, name: &CStr) -> io::Result<Metadata> {
    let mut stat: stat64 = unsafe { mem::zeroed() };
    let res = unsafe { fstatat64(parent_dirfd, name.as_ptr(), &mut stat, 0) };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(Metadata { stat })
    }
}

pub fn lstatat<N: Into<OsString>>(
    parent_dirfd: RawFd,
    name: N,
) -> io::Result<Metadata> {
    let bytes = name.into().into_vec();
    lstatat_c(parent_dirfd, &CString::new(bytes)?)
}

pub fn lstatat_c(parent_dirfd: RawFd, name: &CStr) -> io::Result<Metadata> {
    let mut stat: stat64 = unsafe { mem::zeroed() };
    let res = unsafe {
        fstatat64(
            parent_dirfd,
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if res < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(Metadata { stat })
    }
}
