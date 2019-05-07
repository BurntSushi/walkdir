/*!
Low level Unix specific APIs for reading directory entries via `readdir`.
*/

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io;
use std::mem;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::path::PathBuf;
use std::ptr::NonNull;

use libc;
#[cfg(any(
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "haiku",
    target_os = "hermit",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "newlib",
    target_os = "openbsd",
    target_os = "solaris",
))]
use libc::readdir;
#[cfg(any(
    target_os = "android",
    target_os = "emscripten",
    target_os = "fuchsia",
    target_os = "linux",
))]
use libc::readdir64 as readdir;

#[cfg(target_os = "linux")]
use crate::os::linux::DirEntry as LinuxDirEntry;
use crate::os::unix::dirent::RawDirEntry;

mod dirent;
pub(crate) mod errno;

/// A low-level Unix specific directory entry.
///
/// This type corresponds as closely as possible to the `dirent` structure
/// found on Unix-like platforms. It exposes the underlying file name, file
/// serial number, and, on platforms that support it, the file type.
///
/// All methods on this directory entry have zero cost. That is, no allocations
/// or syscalls are performed.
#[derive(Clone)]
pub struct DirEntry {
    /// A copy of the file name contents from the raw dirent, represented as a
    /// NUL terminated C string. We use a Vec<u8> here instead of a `CString`
    /// because it makes it easier to correctly amortize allocation, and keep
    /// track of the correct length of the string without needing to recompute
    /// it.
    ///
    /// Note that this has to be a copy since the lifetime of `d_name` from
    /// `struct dirent *` is not guaranteed to last beyond the next call to
    /// `readdir`.
    file_name: Vec<u8>,
    /// The file type, as is, from the raw dirent.
    file_type: Option<FileType>,
    /// The file serial number, as is, from the raw dirent.
    ino: u64,
}

impl fmt::Debug for DirEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("DirEntry")
            .field("file_name", &escaped_bytes(self.file_name_bytes()))
            .field("file_type", &self.file_type)
            .field("ino", &self.ino)
            .finish()
    }
}

impl DirEntry {
    /// Read the contents of the given raw Unix/POSIX directory entry into this
    /// entry.
    #[inline]
    fn from_unix_raw(&mut self, raw: &RawDirEntry) {
        self.file_type = raw.file_type();
        self.ino = raw.ino();

        let bytes = raw.file_name().to_bytes_with_nul();
        self.file_name.resize(bytes.len(), 0);
        self.file_name.copy_from_slice(bytes);
    }

    /// Read the contents of the given raw Unix/POSIX directory entry into this
    /// entry.
    #[cfg(target_os = "linux")]
    #[inline]
    pub(crate) fn from_linux_raw(&mut self, raw: &LinuxDirEntry) {
        self.file_type = raw.file_type();
        self.ino = raw.ino();

        let bytes = raw.file_name().to_bytes_with_nul();
        self.file_name.resize(bytes.len(), 0);
        self.file_name.copy_from_slice(bytes);
    }

    /// Create a new empty directory entry.
    ///
    /// For an empty directory entry, the file name is empty, the file type is
    /// `None` and the inode number is `0`.
    ///
    /// This is useful for creating space for using `Dir::read_into`.
    #[inline]
    pub fn empty() -> DirEntry {
        DirEntry { file_name: vec![0], file_type: None, ino: 0 }
    }

    /// Return the file name in this directory entry as a C string.
    #[inline]
    pub fn file_name(&self) -> &CStr {
        // SAFETY: file_name is always a normal NUL terminated C string.
        // We just represent it as a Vec<u8> to make amortizing allocation
        // easier.
        unsafe { CStr::from_bytes_with_nul_unchecked(&self.file_name) }
    }

    /// Consume this directory entry and return the underlying C string.
    #[inline]
    pub fn into_file_name(mut self) -> CString {
        // SAFETY: file_name is always a normal NUL terminated C string.
        // We just represent it as a Vec<u8> to make amortizing allocation
        // easier.
        unsafe {
            // There's no way to build a CString from a Vec with zero overhead.
            // Namely, from_vec_unchecked actually adds a NUL byte. Since we
            // already have one, pop it.
            //
            // FIXME: CString really should have a from_vec_with_nul_unchecked
            // routine like CStr.
            self.file_name.pop().expect("a NUL byte");
            CString::from_vec_unchecked(self.file_name)
        }
    }

    /// Return the file name in this directory entry as raw bytes without
    /// a `NUL` terminator.
    #[inline]
    pub fn file_name_bytes(&self) -> &[u8] {
        &self.file_name[..self.file_name.len() - 1]
    }

    /// Consume this directory entry and return the underlying bytes without
    /// a `NUL` terminator.
    #[inline]
    pub fn into_file_name_bytes(mut self) -> Vec<u8> {
        self.file_name.pop().expect("a NUL terminator");
        self.file_name
    }

    /// Return the file name in this directory entry as an OS string. The
    /// string returned does not contain a `NUL` terminator.
    #[inline]
    pub fn file_name_os(&self) -> &OsStr {
        OsStr::from_bytes(self.file_name_bytes())
    }

    /// Consume this directory entry and return its file name as an OS string
    /// without a `NUL` terminator.
    #[inline]
    pub fn into_file_name_os(self) -> OsString {
        OsString::from_vec(self.into_file_name_bytes())
    }

    /// Return the file type of this directory entry, if one exists.
    ///
    /// A file type may not exist if the underlying file system reports an
    /// unknown file type in the directory entry, or if the platform does not
    /// support reporting the file type in the directory entry at all.
    #[inline]
    pub fn file_type(&self) -> Option<FileType> {
        self.file_type
    }

    /// Returns the underlying file serial number for this directory entry.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.ino
    }
}

/// A file descriptor opened as a directory.
///
/// The file descriptor is automatically closed when it's dropped.
#[derive(Debug)]
pub struct DirFd(RawFd);

unsafe impl Send for DirFd {}

impl Drop for DirFd {
    fn drop(&mut self) {
        unsafe {
            // Explicitly ignore the error here if one occurs. To get an error
            // when closing, use DirFd::close.
            libc::close(self.0);
        }
    }
}

impl AsRawFd for DirFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl IntoRawFd for DirFd {
    fn into_raw_fd(self) -> RawFd {
        let fd = self.0;
        mem::forget(self);
        fd
    }
}

impl FromRawFd for DirFd {
    unsafe fn from_raw_fd(fd: RawFd) -> DirFd {
        DirFd(fd)
    }
}

impl io::Seek for DirFd {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        let mut file = unsafe { File::from_raw_fd(self.0) };
        let res = file.seek(pos);
        file.into_raw_fd();
        res
    }
}

impl DirFd {
    /// Open a file descriptor for the given directory path.
    ///
    /// If there was a problem opening the directory, or if the given path
    /// contains a `NUL` byte, then an error is returned.
    ///
    /// If possible, prefer using `openat` since it is generally faster.
    pub fn open<P: Into<PathBuf>>(dir_path: P) -> io::Result<DirFd> {
        let bytes = dir_path.into().into_os_string().into_vec();
        DirFd::open_c(&CString::new(bytes)?)
    }

    /// Open a file descriptor for the given directory path.
    ///
    /// This is just like `DirFd::open`, except it accepts a pre-made C string.
    /// As such, this only returns an error when opening the directory fails.
    pub fn open_c(dir_path: &CStr) -> io::Result<DirFd> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        // SAFETY: This is safe since we've guaranteed that cstr has no
        // interior NUL bytes and is terminated by a NUL.
        let fd = unsafe { libc::open(dir_path.as_ptr(), flags) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(DirFd(fd))
        }
    }

    /// Open a file descriptor for the given directory name, where the given
    /// file descriptor (`parent_dirfd`) corresponds to the parent directory
    /// of the given name.
    ///
    /// One should prefer using this in lieu of `open` when possible, since it
    /// should generally be faster (but does of course require having an open
    /// file descriptor to the parent directory).
    ///
    /// If there was a problem opening the directory, or if the given path
    /// contains a `NUL` byte, then an error is returned.
    pub fn openat<D: Into<OsString>>(
        parent_dirfd: RawFd,
        dir_name: D,
    ) -> io::Result<DirFd> {
        DirFd::openat_c(
            parent_dirfd,
            &CString::new(dir_name.into().into_vec())?,
        )
    }

    /// Open a file descriptor for the given directory name, where the given
    /// file descriptor (`parent_dirfd`) corresponds to the parent directory
    /// of the given name.
    ///
    /// This is just like `DirFd::openat`, except it accepts a pre-made C
    /// string. As such, this only returns an error when opening the directory
    /// fails.
    pub fn openat_c(
        parent_dirfd: RawFd,
        dir_name: &CStr,
    ) -> io::Result<DirFd> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        // SAFETY: This is safe since we've guaranteed that cstr has no
        // interior NUL bytes and is terminated by a NUL.
        let fd =
            unsafe { libc::openat(parent_dirfd, dir_name.as_ptr(), flags) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(DirFd(fd))
        }
    }

    /// Close this directory file descriptor and return an error if closing
    /// failed.
    ///
    /// Note that this does not need to be called explicitly. This directory
    /// file descriptor will be closed automatically when it is dropped (and
    /// if an error occurs, it is ignored). This routine is only useful if you
    /// want to explicitly close the directory file descriptor and check the
    /// error.
    pub fn close(self) -> io::Result<()> {
        let res = if unsafe { libc::close(self.0) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        };
        // Don't drop DirFd after we've explicitly closed the dir stream to
        // avoid running close again.
        mem::forget(self);
        res
    }
}

/// A handle to a directory stream.
///
/// The handle is automatically closed when it's dropped.
#[derive(Debug)]
pub struct Dir(NonNull<libc::DIR>);

unsafe impl Send for Dir {}

impl Drop for Dir {
    fn drop(&mut self) {
        unsafe {
            // Explicitly ignore the error here if one occurs. To get an error
            // when closing, use Dir::close.
            libc::closedir(self.0.as_ptr());
        }
    }
}

impl AsRawFd for Dir {
    fn as_raw_fd(&self) -> RawFd {
        // It's possible for this to return an error according to POSIX, but I
        // guess we just ignore it. In particular, it looks like common
        // implementations (e.g., Linux) do not actually ever return an error.
        unsafe { libc::dirfd(self.0.as_ptr()) }
    }
}

impl IntoRawFd for Dir {
    fn into_raw_fd(self) -> RawFd {
        let fd = self.as_raw_fd();
        mem::forget(self);
        fd
    }
}

impl FromRawFd for Dir {
    unsafe fn from_raw_fd(fd: RawFd) -> Dir {
        match NonNull::new(unsafe { libc::fdopendir(fd) }) {
            Some(dir) => Dir(dir),
            None => panic!(
                "failed to create libc::DIR from file descriptor: {}",
                io::Error::last_os_error()
            ),
        }
    }
}

impl Dir {
    /// Open a handle to a directory stream for the given directory path.
    ///
    /// If there was a problem opening the directory stream, or if the given
    /// path contains a `NUL` byte, then an error is returned.
    ///
    /// If possible, prefer using `openat` since it is generally faster.
    pub fn open<P: Into<PathBuf>>(dir_path: P) -> io::Result<Dir> {
        let bytes = dir_path.into().into_os_string().into_vec();
        Dir::open_c(&CString::new(bytes)?)
    }

    /// Open a handle to a directory stream for the given directory path.
    ///
    /// This is just like `Dir::open`, except it accepts a pre-made C string.
    /// As such, this only returns an error when opening the directory stream
    /// fails.
    pub fn open_c(dir_path: &CStr) -> io::Result<Dir> {
        // SAFETY: This is safe since we've guaranteed that cstr has no
        // interior NUL bytes and is terminated by a NUL.
        match NonNull::new(unsafe { libc::opendir(dir_path.as_ptr()) }) {
            None => Err(io::Error::last_os_error()),
            Some(dir) => Ok(Dir(dir)),
        }
    }

    /// Open a handle to a directory stream for the given directory name, where
    /// the file descriptor corresponds to the parent directory of the given
    /// name.
    ///
    /// One should prefer using this in lieu of `open` when possible, since it
    /// should generally be faster (but does of course require having an open
    /// file descriptor to the parent directory).
    ///
    /// If there was a problem opening the directory stream, or if the given
    /// path contains a `NUL` byte, then an error is returned.
    pub fn openat<D: Into<OsString>>(
        parent_dirfd: RawFd,
        dir_name: D,
    ) -> io::Result<Dir> {
        Dir::openat_c(parent_dirfd, &CString::new(dir_name.into().into_vec())?)
    }

    /// Open a handle to a directory stream for the given directory name, where
    /// the file descriptor corresponds to the parent directory of the given
    /// name.
    ///
    /// This is just like `Dir::openat`, except it accepts a pre-made C string
    /// for the directory name. As such, this only returns an error when
    /// opening the directory stream fails.
    pub fn openat_c(parent_dirfd: RawFd, dir_name: &CStr) -> io::Result<Dir> {
        let dirfd = DirFd::openat_c(parent_dirfd, dir_name)?;
        // SAFETY: fd is a valid file descriptor, per the above check.
        match NonNull::new(unsafe { libc::fdopendir(dirfd.into_raw_fd()) }) {
            None => Err(io::Error::last_os_error()),
            Some(dir) => Ok(Dir(dir)),
        }
    }

    /// Read the next directory entry from this stream.
    ///
    /// This returns `None` when no more directory entries could be read.
    ///
    /// If there was a problem reading the next directory entry, then an error
    /// is returned. When an error occurs, callers can still continue to read
    /// subsequent directory entries.
    ///
    /// Note that no filtering of entries (such as `.` and `..`) is performed.
    pub fn read(&mut self) -> Option<io::Result<DirEntry>> {
        let mut ent = DirEntry::empty();
        match self.read_into(&mut ent) {
            Ok(true) => Some(Ok(ent)),
            Ok(false) => None,
            Err(err) => Some(Err(err)),
        }
    }

    /// Read the next directory entry from this stream into the given space.
    ///
    /// This returns false when no more directory entries could be read.
    ///
    /// If there was a problem reading the next directory entry, then an error
    /// is returned. When an error occurs, callers can still continue to read
    /// subsequent directory entries.
    ///
    /// The contents of `ent` when the end of the stream has been reached or
    /// when an error occurs are unspecified.
    ///
    /// Note that no filtering of entries (such as `.` and `..`) is performed.
    pub fn read_into(&mut self, ent: &mut DirEntry) -> io::Result<bool> {
        // We need to clear the errno because it's the only way to
        // differentiate errors and end-of-stream. (Since both return a NULL
        // dirent.)
        //
        // TODO: It might be worth experimenting with readdir_r, but note that
        // it is deprecated on Linux, and is presumably going to be deprecated
        // in POSIX. The idea is that readdir is supposed to be reentrant these
        // days. readdir_r does have some of its own interesting problems
        // associated with it. See readdir_r(3) on Linux.
        errno::clear();
        match RawDirEntry::new(unsafe { readdir(self.0.as_ptr()) }) {
            Some(rawent) => {
                ent.from_unix_raw(&rawent);
                Ok(true)
            }
            None => {
                if errno::errno() != 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Rewind this directory stream such that it restarts back at the
    /// beginning of the directory.
    pub fn rewind(&mut self) {
        unsafe {
            libc::rewinddir(self.0.as_ptr());
        }
    }

    /// Close this directory stream and return an error if closing failed.
    ///
    /// Note that this does not need to be called explicitly. This directory
    /// stream will be closed automatically when it is dropped (and if an error
    /// occurs, it is ignored). This routine is only useful if you want to
    /// explicitly close the directory stream and check the error.
    pub fn close(self) -> io::Result<()> {
        let res = if unsafe { libc::closedir(self.0.as_ptr()) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        };
        // Don't drop Dir after we've explicitly closed the dir stream to
        // avoid running close again.
        mem::forget(self);
        res
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

/// Return a convenience ASCII-only debug representation of the given bytes.
/// In essence, non-ASCII and non-printable bytes are escaped.
pub(crate) fn escaped_bytes(bytes: &[u8]) -> String {
    use std::ascii::escape_default;

    bytes.iter().cloned().flat_map(escape_default).map(|b| b as char).collect()
}
