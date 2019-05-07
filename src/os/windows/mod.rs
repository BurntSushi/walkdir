/*!
Low level Windows specific APIs for reading directory entries via
`FindNextFile`.
*/

use std::char;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::mem;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use winapi::shared::minwindef::{DWORD, FILETIME};
use winapi::shared::winerror::ERROR_NO_MORE_FILES;
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::fileapi::{FindClose, FindFirstFileW, FindNextFileW};
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::minwinbase::WIN32_FIND_DATAW;
use winapi::um::winnt::HANDLE;

/// A low-level Windows specific directory entry.
///
/// This type corresponds as closely as possible to the `WIN32_FIND_DATA`
/// structure found on Windows platforms. It exposes the underlying file name,
/// raw file attributions, time information and file size. Notably, this is
/// quite a bit more information than Unix APIs, which typically only expose
/// the file name, file serial number, and in most cases, the file type.
///
/// All methods on this directory entry have zero cost. That is, no allocations
/// or syscalls are performed.
#[derive(Clone, Debug)]
pub struct DirEntry {
    attr: DWORD,
    creation_time: u64,
    last_access_time: u64,
    last_write_time: u64,
    file_size: u64,
    file_type: FileType,
    /// The file name converted to an OsString (using WTF-8 internally).
    file_name: OsString,
    /// The raw 16-bit code units that make up a file name in Windows. This
    /// does not include the NUL terminator.
    file_name_u16: Vec<u16>,
}

impl DirEntry {
    #[inline]
    fn from_find_data(&mut self, fd: &FindData) {
        self.attr = fd.0.dwFileAttributes;
        self.creation_time = fd.creation_time();
        self.last_access_time = fd.last_access_time();
        self.last_write_time = fd.last_write_time();
        self.file_size = fd.file_size();
        self.file_type = FileType::from_attr(self.attr, fd.0.dwReserved0);

        self.file_name.clear();
        self.file_name_u16.clear();
        fd.decode_file_names_into(
            &mut self.file_name,
            &mut self.file_name_u16,
        );
    }

    /// Create a new empty directory entry.
    ///
    /// For an empty directory entry, the file name is empty, the file
    /// type returns `true` for `is_file` and `false` for all other public
    /// predicates, and the rest of the public API methods on a `DirEntry`
    /// return `0`.
    ///
    /// This is useful for creating for using `FindHandle::read_into`.
    #[inline]
    pub fn empty() -> DirEntry {
        DirEntry {
            attr: 0,
            creation_time: 0,
            last_access_time: 0,
            last_write_time: 0,
            file_size: 0,
            file_type: FileType::from_attr(0, 0),
            file_name: OsString::new(),
            file_name_u16: vec![],
        }
    }

    /// Return the raw file attributes reported in this directory entry.
    ///
    /// The value returned directly corresponds to the `dwFileAttributes`
    /// member of the `WIN32_FIND_DATA` structure.
    #[inline]
    pub fn file_attributes(&self) -> u32 {
        self.attr
    }

    /// Return a 64-bit representation of the creation time of the underlying
    /// file.
    ///
    /// The 64-bit value returned is equivalent to winapi's `FILETIME`
    /// structure, which represents the number of 100-nanosecond intervals
    /// since January 1, 1601 (UTC).
    ///
    /// If the underlying file system does not support creation time, then
    /// `0` is returned.
    #[inline]
    pub fn creation_time(&self) -> u64 {
        self.creation_time
    }

    /// Return a 64-bit representation of the last access time of the
    /// underlying file.
    ///
    /// The 64-bit value returned is equivalent to winapi's `FILETIME`
    /// structure, which represents the number of 100-nanosecond intervals
    /// since January 1, 1601 (UTC).
    ///
    /// If the underlying file system does not support last access time, then
    /// `0` is returned.
    #[inline]
    pub fn last_access_time(&self) -> u64 {
        self.last_access_time
    }

    /// Return a 64-bit representation of the last write time of the
    /// underlying file.
    ///
    /// The 64-bit value returned is equivalent to winapi's `FILETIME`
    /// structure, which represents the number of 100-nanosecond intervals
    /// since January 1, 1601 (UTC).
    ///
    /// If the underlying file system does not support last write time, then
    /// `0` is returned.
    #[inline]
    pub fn last_write_time(&self) -> u64 {
        self.last_write_time
    }

    /// Return the file size, in bytes, of the corresponding file.
    ///
    /// This value has no meaning if this entry corresponds to a directory.
    #[inline]
    pub fn file_size(&self) -> u64 {
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
        FileType { attr: attr, reparse_tag: reparse_tag }
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

/// A handle to a directory stream.
///
/// The handle is automatically closed when it's dropped.
#[derive(Debug)]
pub struct FindHandle {
    handle: HANDLE,
    first: Option<FindData>,
}

unsafe impl Send for FindHandle {}

impl Drop for FindHandle {
    fn drop(&mut self) {
        unsafe {
            // Explicitly ignore the error here if one occurs. To get an error
            // when closing, use FindHandle::close.
            FindClose(self.handle);
        }
    }
}

impl FindHandle {
    /// Open a handle for listing files in the given directory.
    ///
    /// If there was a problem opening the handle, then an error is returned.
    pub fn open<P: AsRef<Path>>(dir_path: P) -> io::Result<FindHandle> {
        let dir_path = dir_path.as_ref();
        let mut buffer = Vec::with_capacity(dir_path.as_os_str().len() / 2);
        FindHandle::open_buffer(dir_path, &mut buffer)
    }

    /// Open a handle for listing files in the given directory.
    ///
    /// This is like `open`, except it permits the caller to provide a buffer
    /// that's used for converting the given directory path to UTF-16, as
    /// required by the underlying Windows API.
    pub fn open_buffer<P: AsRef<Path>>(
        dir_path: P,
        buffer: &mut Vec<u16>,
    ) -> io::Result<FindHandle> {
        let dir_path = dir_path.as_ref();

        // Convert the given path to UTF-16, and then add a wild-card to the
        // end of it. Yes, this is how we list files in a directory on Windows.
        // Canonical example:
        // https://docs.microsoft.com/en-us/windows/desktop/FileIO/listing-the-files-in-a-directory
        buffer.clear();
        to_utf16(dir_path, buffer)?;
        if !buffer.ends_with(&['\\' as u16]) {
            buffer.push('\\' as u16);
        }
        buffer.push('*' as u16);
        buffer.push(0);

        let mut first: WIN32_FIND_DATAW = unsafe { mem::zeroed() };
        let handle = unsafe { FindFirstFileW(buffer.as_ptr(), &mut first) };
        if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(FindHandle { handle, first: Some(FindData(first)) })
        }
    }

    /// Read the next directory entry from this handle.
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
        if let Some(first) = self.first.take() {
            ent.from_find_data(&first);
            return Ok(true);
        }
        let mut data: WIN32_FIND_DATAW = unsafe { mem::zeroed() };
        let res = unsafe { FindNextFileW(self.handle, &mut data) };
        if res == 0 {
            return if unsafe { GetLastError() } == ERROR_NO_MORE_FILES {
                Ok(false)
            } else {
                Err(io::Error::last_os_error())
            };
        }
        ent.from_find_data(&FindData(data));
        Ok(true)
    }

    /// Close this find handle and return an error if closing failed.
    ///
    /// Note that this does not need to be called explicitly. This directory
    /// stream will be closed automatically when it is dropped (and if an error
    /// occurs, it is ignored). This routine is only useful if you want to
    /// explicitly close the directory stream and check the error.
    pub fn close(self) -> io::Result<()> {
        let res = if unsafe { FindClose(self.handle) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        };
        // Don't drop FindHandle after we've explicitly closed the dir stream
        // to avoid running close again.
        mem::forget(self);
        res
    }
}

struct FindData(WIN32_FIND_DATAW);

impl fmt::Debug for FindData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FindData")
            .field("dwFileAttributes", &self.0.dwFileAttributes)
            .field("ftCreationTime", &self.0.ftCreationTime)
            .field("ftLastAccessTime", &self.0.ftLastAccessTime)
            .field("ftLastWriteTime", &self.0.ftLastWriteTime)
            .field("nFileSizeHigh", &self.0.nFileSizeHigh)
            .field("nFileSizeLow", &self.0.nFileSizeLow)
            .field("dwReserved0", &self.0.dwReserved0)
            .field("dwReserved1", &self.0.dwReserved1)
            .field("cFileName", &self.file_name())
            .field(
                "cAlternateFileName",
                &OsString::from_wide(&truncate_utf16(
                    &self.0.cAlternateFileName,
                )),
            )
            .finish()
    }
}

impl FindData {
    fn creation_time(&self) -> u64 {
        time_as_u64(&self.0.ftCreationTime)
    }

    fn last_access_time(&self) -> u64 {
        time_as_u64(&self.0.ftLastAccessTime)
    }

    fn last_write_time(&self) -> u64 {
        time_as_u64(&self.0.ftLastWriteTime)
    }

    fn file_size(&self) -> u64 {
        (self.0.nFileSizeHigh as u64) << 32 | self.0.nFileSizeLow as u64
    }

    /// Return an owned copy of the underlying file name as an OS string.
    fn file_name(&self) -> OsString {
        let file_name = truncate_utf16(&self.0.cFileName);
        OsString::from_wide(file_name)
    }

    /// Read the contents of the underlying file name into the given OS string.
    /// If the allocation can be reused, then it will be, otherwise it will be
    /// overwritten with a fresh OsString.
    ///
    /// The second buffer provided will have the raw 16-bit code units of the
    /// file name pushed to it.
    fn decode_file_names_into(
        &self,
        dst_os: &mut OsString,
        dst_16: &mut Vec<u16>,
    ) {
        // This implementation is a bit weird, but basically, there is no way
        // to amortize OsString allocations in the general case, since the only
        // API to build an OsString from a &[u16] is OsStringExt::from_wide,
        // which returns an OsString.
        //
        // However, in the vast majority of cases, the underlying file name
        // will be valid UTF-16, which we can transcode to UTF-8 and then
        // push to a pre-existing OsString. It's not the best solution, but
        // it permits reusing allocations!
        let file_name = truncate_utf16(&self.0.cFileName);
        dst_16.extend_from_slice(file_name);
        for result in char::decode_utf16(file_name.iter().cloned()) {
            match result {
                Ok(c) => {
                    dst_os.push(c.encode_utf8(&mut [0; 4]));
                }
                Err(_) => {
                    *dst_os = OsString::from_wide(file_name);
                    return;
                }
            }
        }
    }
}

fn time_as_u64(time: &FILETIME) -> u64 {
    (time.dwHighDateTime as u64) << 32 | time.dwLowDateTime as u64
}

fn to_utf16<T: AsRef<OsStr>>(t: T, buf: &mut Vec<u16>) -> io::Result<()> {
    for cu16 in t.as_ref().encode_wide() {
        if cu16 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file paths on Windows cannot contain NUL bytes",
            ));
        }
        buf.push(cu16);
    }
    Ok(())
}

fn truncate_utf16(slice: &[u16]) -> &[u16] {
    match slice.iter().position(|c| *c == 0) {
        Some(i) => &slice[..i],
        None => slice,
    }
}
