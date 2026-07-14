/*!
Low level Windows specific APIs for reading directory entries via
`NtOpenFile` relative opens and `GetFileInformationByHandleEx`.
*/

use std::char;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::OpenOptions;
use std::io;
use std::mem;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{
    AsRawHandle, FromRawHandle, OwnedHandle, RawHandle,
};
use std::path::Path;
use std::ptr;
use std::time::{self, SystemTime};

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    NtOpenFile, FILE_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    RtlNtStatusToDosError, ERROR_DIRECTORY, ERROR_NO_MORE_FILES, HANDLE,
    NTSTATUS, STATUS_DELETE_PENDING, STATUS_PENDING, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx,
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0};

pub use crate::os::windows::stat::{lstat, stat, FileType, Metadata};

mod stat;

/// The Win32 error code mapped from [`STATUS_DELETE_PENDING`], so a pending
/// delete is not misreported as an access-denied error.
const ERROR_DELETE_PENDING: u32 = 303;

/// A heap buffer aligned to 8 bytes, as [`FILE_ID_BOTH_DIR_INFO`] requires.
#[repr(C, align(8))]
struct Align8<T>(T);

/// The enumeration buffer size. Larger than std's 1024 to cut syscalls.
const BUF_LEN: usize = 4096;

/// A low-level Windows specific directory entry.
///
/// This type corresponds as closely as possible to the [`FILE_ID_BOTH_DIR_INFO`]
/// structure reported by directory enumeration on Windows platforms. It
/// exposes the underlying file name, raw file attributes, time information and
/// file size. Notably, this is quite a bit more information than Unix APIs,
/// which typically only expose the file name, file serial number, and in most
/// cases, the file type.
///
/// All methods on this directory entry have zero cost. That is, no allocations
/// or syscalls are performed.
#[derive(Clone)]
pub struct DirEntry {
    /// The file name converted to an OsString (using WTF-8 internally).
    file_name: OsString,
    /// The raw 16-bit code units that make up a file name in Windows. This
    /// does not include a NUL terminator.
    file_name_u16: Vec<u16>,
    attr: u32,
    reparse_tag: u32,
    creation_time: u64,
    last_access_time: u64,
    last_write_time: u64,
    file_size: u64,
    file_type: FileType,
}

impl fmt::Debug for DirEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("DirEntry")
            .field("file_name", &escaped_u16s(&self.file_name_u16))
            .field("attr", &self.attr)
            .field("reparse_tag", &self.reparse_tag)
            .field("file_type", &self.file_type)
            .finish()
    }
}

impl DirEntry {
    /// Create a new empty directory entry.
    ///
    /// For an empty directory entry, the file name is empty, the file
    /// type returns `true` for `is_file` and `false` for all other public
    /// predicates, and the rest of the public API methods on a [`DirEntry`]
    /// return `0`.
    ///
    /// This is useful for creating space for using [`Dir::read_into`].
    #[inline]
    pub fn empty() -> DirEntry {
        DirEntry {
            file_name: OsString::new(),
            file_name_u16: vec![],
            attr: 0,
            reparse_tag: 0,
            creation_time: 0,
            last_access_time: 0,
            last_write_time: 0,
            file_size: 0,
            file_type: FileType::from_attr(0, 0),
        }
    }

    /// Return the raw file attributes reported in this directory entry.
    ///
    /// The value returned directly corresponds to the `FileAttributes` member
    /// of the [`FILE_ID_BOTH_DIR_INFO`] structure.
    #[inline]
    pub fn file_attributes(&self) -> u32 {
        self.attr
    }

    /// Returns true if this file is marked as hidden via the
    /// [`FILE_ATTRIBUTE_HIDDEN`] marker.
    #[inline]
    pub fn is_hidden(&self) -> bool {
        self.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
    }

    /// Return the creation time of the underlying file as a system time.
    ///
    /// If the underlying file system does not support creation time, then an
    /// error is returned.
    #[inline]
    pub fn created(&self) -> io::Result<SystemTime> {
        if self.creation_time == 0 {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "creation time is not available on this platform currently",
            ))
        } else {
            Ok(intervals_to_system_time(self.creation_time))
        }
    }

    /// Return last access time of the underlying file as a system time.
    ///
    /// If the underlying file system does not support creation time, then an
    /// error is returned.
    #[inline]
    pub fn accessed(&self) -> io::Result<SystemTime> {
        if self.last_access_time == 0 {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "last access time is not available on this platform currently",
            ))
        } else {
            Ok(intervals_to_system_time(self.last_access_time))
        }
    }

    /// Return the last modified time of the underlying file as a system time.
    ///
    /// If the underlying file system does not support creation time, then an
    /// error is returned.
    #[inline]
    pub fn modified(&self) -> io::Result<SystemTime> {
        if self.last_write_time == 0 {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "last write time is not available on this platform currently",
            ))
        } else {
            Ok(intervals_to_system_time(self.last_write_time))
        }
    }

    /// Return the file size, in bytes, of the corresponding file.
    ///
    /// This value has no meaning if this entry corresponds to a directory.
    #[inline]
    pub fn len(&self) -> u64 {
        self.file_size
    }

    /// Return the file type of this directory entry.
    #[inline]
    pub fn file_type(&self) -> FileType {
        self.file_type
    }

    /// Return the file name in this directory entry as an OS string.
    #[inline]
    pub fn file_name_os(&self) -> &OsStr {
        &self.file_name
    }

    /// Returns true if this entry is the `.` or `..` pseudo-entry.
    #[inline]
    pub fn is_dots(&self) -> bool {
        matches!(self.file_name_u16.as_slice(), [0x2E] | [0x2E, 0x2E])
    }

    /// Return the file name in this directory entry in its original form as
    /// a sequence of 16-bit code units.
    ///
    /// The sequence returned is not guaranteed to be valid UTF-16.
    #[inline]
    pub fn file_name_u16(&self) -> &[u16] {
        &self.file_name_u16
    }

    /// Consume this directory entry and return its file name as an OS string.
    #[inline]
    pub fn into_file_name_os(self) -> OsString {
        self.file_name
    }

    /// Consume this directory entry and return its file name in its original
    /// form as a sequence of 16-bit code units.
    ///
    /// The sequence returned is not guaranteed to be valid UTF-16.
    #[inline]
    pub fn into_file_name_u16(self) -> Vec<u16> {
        self.file_name_u16
    }
}

/// A handle to a directory opened for enumeration.
///
/// The handle is automatically closed when it's dropped.
pub struct Dir {
    handle: OwnedHandle,
    buf: Box<Align8<[u8; BUF_LEN]>>,
    /// The byte offset of the next record in `buf`, or [`None`] when the buffer is
    /// exhausted and a fresh enumeration call is needed.
    cursor: Option<usize>,
    /// True until the first enumeration call, which must use the restart class.
    restart: bool,
}

impl fmt::Debug for Dir {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Dir").field("handle", &self.handle).finish()
    }
}

impl AsRawHandle for Dir {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}

impl Dir {
    fn from_handle(handle: OwnedHandle) -> Dir {
        Dir {
            handle,
            buf: Box::new(Align8([0u8; BUF_LEN])),
            cursor: None,
            restart: true,
        }
    }

    /// Open a directory for enumeration, following a trailing symlink.
    pub fn open<P: AsRef<Path>>(dir_path: P) -> io::Result<Dir> {
        Dir::open_path_follow(dir_path.as_ref(), true)
    }

    /// Open `path` for enumeration with the requested follow mode.
    ///
    /// This goes through [`OpenOptions`], which prepends the `\\?\` long path
    /// prefix as needed, so it is robust to paths longer than `MAX_PATH`.
    pub fn open_path_follow(path: &Path, follow: bool) -> io::Result<Dir> {
        let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
        if !follow {
            flags |= FILE_FLAG_OPEN_REPARSE_POINT;
        }
        let file = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY)
            .custom_flags(flags)
            .open(path)?;

        // Opening with FILE_LIST_DIRECTORY succeeds even on a plain file, so
        // the type has to be verified before enumerating.
        // SAFETY: file is a valid open handle and info is filled on success.
        let attr = unsafe {
            let mut info = mem::zeroed::<BY_HANDLE_FILE_INFORMATION>();
            let res = GetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                &mut info,
            );
            if res == 0 {
                return Err(io::Error::last_os_error());
            }
            info.dwFileAttributes
        };
        if attr & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(io::Error::from_raw_os_error(ERROR_DIRECTORY as i32));
        }
        Ok(Dir::from_handle(OwnedHandle::from(file)))
    }

    /// Open a directory handle for the given directory name, where the given
    /// handle (`parent`) corresponds to the parent directory of the given name.
    ///
    /// Since Win32 has no `openat`, the child is opened relative to the parent
    /// handle with `NtOpenFile`. When `follow` is false it is opened with
    /// [`FILE_OPEN_REPARSE_POINT`], which is the Windows equivalent of `O_NOFOLLOW`,
    /// so that if the name was swapped for a reparse point it is opened as the
    /// link itself rather than being followed.
    ///
    /// `name` must be NUL terminated, and the trailing NUL is load-bearing,
    /// since per rustc#143078 some Windows builds read one `u16` past the name
    /// and the buffer is not backed by the padded enumeration buffer. An error
    /// is returned if the directory could not be opened, or if `name` contains
    /// an interior `NUL` code unit.
    pub fn openat_follow(
        parent: RawHandle,
        name: &[u16],
        follow: bool,
    ) -> io::Result<Dir> {
        if name.last() != Some(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "relative names must be NUL terminated",
            ));
        }
        if name[..name.len() - 1].contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file names on Windows cannot contain NUL code units",
            ));
        }
        let (Some(length), Some(maximum_length)) =
            ((name.len() - 1).checked_mul(2), name.len().checked_mul(2))
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "relative name is too long",
            ));
        };
        if length > u16::MAX as usize || maximum_length > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "relative name is too long",
            ));
        }

        let name = UNICODE_STRING {
            Length: length as u16,
            MaximumLength: maximum_length as u16,
            Buffer: name.as_ptr().cast_mut(),
        };
        let object = OBJECT_ATTRIBUTES {
            Length: mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: parent as HANDLE,
            ObjectName: &name,
            Attributes: 0,
            SecurityDescriptor: ptr::null(),
            SecurityQualityOfService: ptr::null(),
        };
        let access = SYNCHRONIZE | FILE_LIST_DIRECTORY;
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
        let mut options = FILE_SYNCHRONOUS_IO_NONALERT | FILE_DIRECTORY_FILE;
        if !follow {
            options |= FILE_OPEN_REPARSE_POINT;
        }

        let mut handle: HANDLE = ptr::null_mut();
        let mut io_status = IO_STATUS_BLOCK {
            Anonymous: IO_STATUS_BLOCK_0 { Status: STATUS_PENDING },
            Information: 0,
        };
        // SAFETY: object and io_status are valid pointers that live across the
        // call, and object borrows name, whose buffer is the caller's NUL
        // terminated slice and so stays valid for the duration of the call.
        let status = unsafe {
            NtOpenFile(
                &mut handle,
                access,
                &object,
                &mut io_status,
                share,
                options,
            )
        };
        if nt_success(status) {
            // SAFETY: NtOpenFile succeeded, so handle owns a fresh handle.
            let owned = unsafe { OwnedHandle::from_raw_handle(handle as _) };
            Ok(Dir::from_handle(owned))
        } else {
            Err(io::Error::from_raw_os_error(nt_error(status) as i32))
        }
    }

    /// Read the next directory entry from this handle.
    ///
    /// This returns `None` when no more directory entries could be read.
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

    /// Read the next directory entry from this handle into the given space.
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
        let off = match self.cursor {
            Some(off) => off,
            None => {
                if !self.fill()? {
                    return Ok(false);
                }
                0
            }
        };

        // Drivers are not trusted here. Real filesystems return misaligned
        // records (rust#104530) and a buggy or hostile one could return
        // offsets past the buffer, so every field is read with read_unaligned,
        // no reference is ever formed into the buffer and all offsets are
        // bounds checked before any pointer arithmetic.
        match off.checked_add(mem::size_of::<FILE_ID_BOTH_DIR_INFO>()) {
            Some(end) if end <= BUF_LEN => {}
            _ => {
                self.cursor = None;
                return Err(malformed_record());
            }
        }
        // SAFETY: the check above guarantees off leaves room for a full record
        // header, so every read below stays in bounds, and each one uses
        // unaligned access since the records are not guaranteed to be aligned.
        unsafe {
            let rec =
                self.buf.0.as_ptr().add(off) as *const FILE_ID_BOTH_DIR_INFO;
            let next_entry =
                ptr::read_unaligned(ptr::addr_of!((*rec).NextEntryOffset));
            let attr =
                ptr::read_unaligned(ptr::addr_of!((*rec).FileAttributes));
            let name_len =
                ptr::read_unaligned(ptr::addr_of!((*rec).FileNameLength))
                    as usize;
            let creation =
                ptr::read_unaligned(ptr::addr_of!((*rec).CreationTime));
            let last_access =
                ptr::read_unaligned(ptr::addr_of!((*rec).LastAccessTime));
            let last_write =
                ptr::read_unaligned(ptr::addr_of!((*rec).LastWriteTime));
            let end_of_file =
                ptr::read_unaligned(ptr::addr_of!((*rec).EndOfFile));
            // For reparse points, EaSize aliases the reparse tag in the
            // FILE_ID_BOTH_DIR_INFORMATION layout that kernel32 forwards.
            let reparse_tag = if attr & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                ptr::read_unaligned(ptr::addr_of!((*rec).EaSize))
            } else {
                0
            };
            let name_ptr = ptr::addr_of!((*rec).FileName) as *const u16;
            let name_off = name_ptr as usize - self.buf.0.as_ptr() as usize;
            match name_off.checked_add(name_len) {
                Some(end) if end <= BUF_LEN => {}
                _ => {
                    self.cursor = None;
                    return Err(malformed_record());
                }
            }

            ent.attr = attr;
            ent.reparse_tag = reparse_tag;
            ent.creation_time = creation as u64;
            ent.last_access_time = last_access as u64;
            ent.last_write_time = last_write as u64;
            ent.file_size = end_of_file as u64;
            ent.file_type = FileType::from_attr(attr, reparse_tag);

            let name_units = name_len / 2;
            ent.file_name_u16.clear();
            ent.file_name_u16.reserve(name_units);
            for i in 0..name_units {
                ent.file_name_u16.push(ptr::read_unaligned(name_ptr.add(i)));
            }

            self.cursor = if next_entry == 0 {
                None
            } else {
                match off.checked_add(next_entry as usize) {
                    Some(next) => Some(next),
                    None => {
                        self.cursor = None;
                        return Err(malformed_record());
                    }
                }
            };
        }

        ent.file_name.clear();
        decode_utf16_into(&ent.file_name_u16, &mut ent.file_name);
        Ok(true)
    }

    /// Fill the buffer with the next batch of entries.
    ///
    /// Returns false when the directory has been fully enumerated.
    fn fill(&mut self) -> io::Result<bool> {
        let class = if self.restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        // SAFETY: buf is valid for BUF_LEN bytes and outlives the call.
        let res = unsafe {
            GetFileInformationByHandleEx(
                self.handle.as_raw_handle() as HANDLE,
                class,
                self.buf.0.as_mut_ptr().cast(),
                BUF_LEN as u32,
            )
        };
        if res == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(false);
            }
            // Keep restart set so a retry after a transient error starts the
            // enumeration over instead of continuing one that never began.
            return Err(err);
        }
        self.restart = false;
        self.cursor = Some(0);
        Ok(true)
    }
}

fn malformed_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "directory entry record has out of bounds offsets",
    )
}

fn nt_success(status: NTSTATUS) -> bool {
    status >= 0
}

fn nt_error(status: NTSTATUS) -> u32 {
    if status == STATUS_DELETE_PENDING {
        ERROR_DELETE_PENDING
    } else {
        // SAFETY: RtlNtStatusToDosError has no preconditions.
        unsafe { RtlNtStatusToDosError(status) }
    }
}

/// Decode UTF-16 code units into `dst`, reusing its allocation where possible.
///
/// `dst` must be empty on entry. On invalid UTF-16 it falls back to a fresh
/// [`OsString`] that preserves the unpaired surrogates.
fn decode_utf16_into(units: &[u16], dst: &mut OsString) {
    for result in char::decode_utf16(units.iter().copied()) {
        match result {
            Ok(c) => {
                dst.push(c.encode_utf8(&mut [0; 4]));
            }
            Err(_) => {
                *dst = OsString::from_wide(units);
                return;
            }
        }
    }
}

pub(crate) fn time_as_u64(
    time: &windows_sys::Win32::Foundation::FILETIME,
) -> u64 {
    (time.dwHighDateTime as u64) << 32 | time.dwLowDateTime as u64
}

pub(crate) fn intervals_to_system_time(intervals: u64) -> SystemTime {
    const NANOS_IN_SECOND: u64 = 1_000_000_000;
    const NANOS_PER_INTERVAL: u64 = 100;
    const SECONDS_TO_UNIX: u64 = 11_644_473_600;

    let seconds_from_unix =
        (intervals / (NANOS_IN_SECOND / NANOS_PER_INTERVAL)) - SECONDS_TO_UNIX;
    let dur_from_unix = time::Duration::from_secs(seconds_from_unix);
    SystemTime::UNIX_EPOCH + dur_from_unix
}

pub(crate) fn escaped_u16s(slice: &[u16]) -> String {
    use std::char;

    let mut buf = String::with_capacity(slice.len());
    for result in char::decode_utf16(slice.iter().cloned()) {
        match result {
            Ok(ch) => buf.push(ch),
            Err(err) => {
                let bad = err.unpaired_surrogate();
                buf.push_str(&format!(r"\u{{{:X}}}", bad));
            }
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openat_rejects_invalid_names() {
        let parent = std::ptr::null_mut();
        for name in [&[][..], &[0x61][..], &[0x61, 0, 0x62, 0][..]] {
            let err = Dir::openat_follow(parent, name, false).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }

        let mut name = vec![0x61; (u16::MAX as usize / 2) + 1];
        name.push(0);
        let err = Dir::openat_follow(parent, &name, false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn escaping1() {
        let x: Vec<u16> = "foo☃bar".encode_utf16().collect();
        let escaped = escaped_u16s(&x);
        assert_eq!("foo☃bar", escaped);
    }

    #[test]
    fn escaping2() {
        let mut x = vec![];
        x.push(0xD800);
        x.extend("a".encode_utf16());
        x.push(0xDA02);
        x.extend("b".encode_utf16());
        x.push(0xDFFF);
        x.extend("c".encode_utf16());

        let escaped = escaped_u16s(&x);
        assert_eq!(r"\u{D800}a\u{DA02}b\u{DFFF}c", escaped);
    }
}
