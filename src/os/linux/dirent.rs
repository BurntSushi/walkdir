use std::ffi::CStr;
use std::fmt;

use libc::c_char;

use crate::os::unix::FileType;

/// A raw directory entry used to read entries from Linux's getdents64 syscall.
///
/// Note that this type is very much not safe to use because `d_name` is a
/// flexible array member. That is, the *size* of values of this type are
/// usually larger than size_of::<RawDirEntry>, since the file name will extend
/// beyond the end of the struct. Therefore, values of this type should only be
/// read when they exist in their original buffer.
///
/// We expose this by making it not safe to ask for the name in this
/// entry, since its `NUL` terminator scan could result in a buffer overrun
/// when used incorrectly.
#[derive(Clone)]
#[repr(C)]
pub struct RawDirEntry {
    d_ino: u64,
    d_off: u64,
    d_reclen: u16,
    d_type: u8,
    d_name: [u8; 0],
}

impl fmt::Debug for RawDirEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("RawDirEntry")
            .field("d_ino", &self.ino())
            .field("d_off", &self.d_off)
            .field("d_reclen", &self.d_reclen)
            .field("d_type", &self.file_type())
            // Reading the name is not safe, and we can't guarantee its
            // safety in the context of this Debug impl unfortunately. We
            // *could* use a small fixed size array instead of [u8; 0] as the
            // representation, which would at least let us safely read a prefix
            // to show here, but it's not clear what cost that would have
            // (probably none?) or whether it's worth it.
            .field("d_name", &"<N/A>")
            .finish()
    }
}

impl RawDirEntry {
    /// Return the file name in this directory entry as a C string.
    ///
    /// This computes the length of the name in this entry by scanning for a
    /// `NUL` terminator.
    ///
    /// # Safety
    ///
    /// This is not safe because callers who call this function must guarantee
    /// that the `RawDirEntry` is still within its original buffer. Otherwise,
    /// it's possible for a buffer overrun to occur.
    pub unsafe fn file_name(&self) -> &CStr {
        CStr::from_ptr(self.d_name.as_ptr() as *const c_char)
    }

    /// Return the file type of this directory entry, if one exists.
    ///
    /// A file type may not exist if the underlying file system reports an
    /// unknown file type in the directory entry.
    pub fn file_type(&self) -> Option<FileType> {
        FileType::from_dirent_type(self.d_type)
    }

    /// Returns the underlying file serial number for this directory entry.
    pub fn ino(&self) -> u64 {
        self.d_ino
    }

    /// Returns the total length (including padding), in bytes, of this
    /// directory entry.
    pub fn record_len(&self) -> usize {
        self.d_reclen as usize
    }
}
