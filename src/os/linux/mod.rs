/*!
Low level Linux specific APIs for reading directory entries via `getdents64`.
*/

use std::alloc::{alloc_zeroed, dealloc, handle_alloc_error, Layout};
use std::ffi::{CStr, CString, OsStr};
use std::fmt;
use std::io;
use std::mem;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::ptr::NonNull;

use libc::{syscall, SYS_getdents64};

use crate::os::linux::dirent::RawDirEntry;
use crate::os::unix::{
    errno, escaped_bytes, DirEntry as UnixDirEntry, DirFd, FileType,
};

mod dirent;

/// A safe function for calling Linux's `getdents64` API.
///
/// The basic idea of `getdents` is that it executes a single syscall but
/// returns potentially many directory entries in a single buffer. This can
/// provide a small speed boost when compared with the typical `readdir` POSIX
/// API, depending on your platform's implementation.
///
/// This routine will read directory entries from the given file descriptor
/// into the given cursor. The cursor can then be used to cheaply and safely
/// iterate over the directory entries that were read.
///
/// When all directory entries have been read from the given file descriptor,
/// then this function will return `false`. Otherwise, it returns `true`.
///
/// If there was a problem calling the underlying `getdents64` syscall, then
/// an error is returned.
pub fn getdents(fd: RawFd, cursor: &mut DirEntryCursor) -> io::Result<bool> {
    cursor.clear();
    let res = unsafe {
        syscall(
            SYS_getdents64,
            fd,
            cursor.raw.as_ptr() as *mut RawDirEntry,
            cursor.capacity,
        )
    };
    match res {
        -1 => Err(io::Error::last_os_error()),
        0 => Ok(false),
        nwritten => {
            cursor.len = nwritten as usize;
            Ok(true)
        }
    }
}

/// A Linux specific directory entry.
///
/// This directory entry is just like the Unix `DirEntry`, except its file
/// name is borrowed from a `DirEntryCursor`'s internal buffer. This makes
/// it possible to iterate over directory entries on Linux by reusing the
/// cursor's internal buffer with no additional allocations or copying.
///
/// In practice, if one needs an owned directory entry, then convert it to a
/// Unix `DirEntry` either via the Unix methods on this `DirEntry`, or by
/// simply reading a Unix `DirEntry` directly from `DirEntryCursor`.
#[derive(Clone)]
pub struct DirEntry<'a> {
    /// A borrowed version of the `d_name` field found in the raw directory
    /// entry. This field is the only reason why this type exists, otherwise
    /// we'd just expose `RawDirEntry` directly to users. The issue with
    /// exposing the raw directory entry is that its size isn't correct (since
    /// the file name may extend beyond the end of the struct).
    ///
    /// This borrow ties this entry to the `DirEntryBuffer`.
    file_name: &'a CStr,
    /// The file type, as is, from the raw dirent.
    file_type: Option<FileType>,
    /// The file serial number, as is, from the raw dirent.
    ino: u64,
}

impl<'a> fmt::Debug for DirEntry<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use crate::os::unix::escaped_bytes;

        f.debug_struct("DirEntry")
            .field("file_name", &escaped_bytes(self.file_name_bytes()))
            .field("file_type", &self.file_type)
            .field("ino", &self.ino)
            .finish()
    }
}

impl<'a> DirEntry<'a> {
    /// Return the file name in this directory entry as a C string.
    #[inline]
    pub fn file_name(&self) -> &CStr {
        self.file_name
    }

    /// Return the file name in this directory entry as raw bytes without
    /// a `NUL` terminator.
    #[inline]
    pub fn file_name_bytes(&self) -> &[u8] {
        self.file_name.to_bytes()
    }

    /// Return the file name in this directory entry as an OS string without
    /// a `NUL` terminator.
    #[inline]
    pub fn file_name_os(&self) -> &OsStr {
        OsStr::from_bytes(self.file_name_bytes())
    }

    /// Return the file type of this directory entry, if one exists.
    ///
    /// A file type may not exist if the underlying file system reports an
    /// unknown file type in the directory entry.
    #[inline]
    pub fn file_type(&self) -> Option<FileType> {
        self.file_type
    }

    /// Returns the underlying file serial number for this directory entry.
    #[inline]
    pub fn ino(&self) -> u64 {
        self.ino
    }

    /// Convert this directory entry into an owned Unix `DirEntry`. If you
    /// want to be able to reuse allocations, then use `write_to_unix` instead.
    #[inline]
    pub fn to_unix(&self) -> UnixDirEntry {
        let mut ent = UnixDirEntry::empty();
        self.write_to_unix(&mut ent);
        ent
    }

    /// Write this directory entry into the given Unix `DirEntry`. This makes
    /// it possible to amortize allocation.
    #[inline]
    pub fn write_to_unix(&self, unix_dirent: &mut UnixDirEntry) {
        unix_dirent.from_linux_raw(self)
    }
}

/// A cursor for reading directory entries from a `getdents` buffer.
///
/// This cursor allocates space internally for storing one or more Linux
/// directory entries, and exposes an API for cheaply iterating over those
/// directory entries.
///
/// A cursor can and should be reused across multiple calls to `getdents`. A
/// cursor is not tied to any one particular directory.
#[derive(Clone, Debug)]
pub struct DirEntryCursor {
    /// Spiritually, this is a *mut RawDirEntry. Unfortunately, this doesn't
    /// quite make sense since a value with type `RawDirEntry` does not
    /// actually have a size of `size_of::<RawDirEntry>()` due to the way in
    /// which the entry's name is stored in a flexible array member.
    ///
    /// With that said, we do transmute bytes in this buffer to a
    /// `RawDirEntry`, which lets us read the members of the struct (including
    /// the flexible array member) correctly. However, because of that, we need
    /// to make sure our memory has the correct alignment. Hence, this is why
    /// we use a raw `*mut u8` created by the std::alloc APIs. If there was an
    /// easy way to control alignment with a `Vec<u8>`, then we could use that
    /// instead. (It is indeed possible, but seems fragile.)
    ///
    /// Since a `RawDirEntry` is inherently unsafe to use because of its
    /// flexible array member, it is converted to a `DirEntry` (cheaply,
    /// without allocation) before being exposed to the caller.
    raw: NonNull<u8>,
    /// The lenth, in bytes, of all valid entries in `raw`.
    len: usize,
    /// The lenth, in bytes, of `raw`.
    capacity: usize,
    /// The current position of this buffer as a pointer into `raw`.
    cursor: NonNull<u8>,
    /// Whether the cursor has been advanced at least once.
    advanced: bool,
}

impl Drop for DirEntryCursor {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.raw.as_ptr(), layout(self.capacity));
        }
    }
}

/// Returns the allocation layout used for constructing the getdents buffer
/// with the given capacity (in bytes).
///
/// This panics if the given length isn't a multiple of the alignment of
/// `RawDirEntry` or is `0`.
fn layout(capacity: usize) -> Layout {
    let align = mem::align_of::<RawDirEntry>();
    assert!(capacity > 0, "capacity must be greater than 0");
    assert!(capacity % align == 0, "capacity must be a multiple of alignment");
    Layout::from_size_align(capacity, align).expect("failed to create Layout")
}

impl DirEntryCursor {
    /// Create a new cursor for reading directory entries.
    ///
    /// It is beneficial to reuse a cursor in multiple calls to `getdents`. A
    /// cursor can be used with any number of directories.
    pub fn new() -> DirEntryCursor {
        DirEntryCursor::with_capacity(32 * (1 << 10))
    }

    /// Create a new cursor with the specified capacity. The capacity given
    /// should be in bytes, and must be a multiple of the alignment of a raw
    /// directory entry.
    fn with_capacity(capacity: usize) -> DirEntryCursor {
        // TODO: It would be nice to expose a way to control the capacity to
        // the caller, but we'd really like the capacity to be a multiple of
        // the alignment. (Technically, the only restriction is that
        // the capacity and the alignment have a least common multiple that
        // doesn't overflow `usize::MAX`. But requiring the size to be a
        // multiple of alignment just seems like good sense in this case.)
        //
        // Anyway, exposing raw capacity to the caller is weird, because they
        // shouldn't need to care about the alignment of an internal type.
        // We *could* expose capacity in "units" of `RawDirEntry` itself, but
        // even this is somewhat incorrect because the size of `RawDirEntry`
        // is smaller than what it typically is, since the size doesn't account
        // for file names. We could just pick a fixed approximate size for
        // file names and add that to the size of `RawDirEntry`. But let's wait
        // for a more concrete use case to emerge before exposing anything.
        let lay = layout(capacity);
        let raw = match NonNull::new(unsafe { alloc_zeroed(lay) }) {
            Some(raw) => raw,
            None => handle_alloc_error(lay),
        };
        DirEntryCursor { raw, len: 0, capacity, cursor: raw, advanced: false }
    }

    /// Read the next directory entry from this cursor. If the cursor has been
    /// exhausted, then return `None`.
    ///
    /// The returned directory entry contains a file name that is borrowed from
    /// this cursor's internal buffer. In particular, no allocation is
    /// performed by this routine. If you need an owned directory entry, then
    /// use `read_unix` or `read_unix_into`.
    ///
    /// Note that no filtering of entries (such as `.` and `..`) is performed.
    pub fn read<'a>(&'a mut self) -> Option<DirEntry<'a>> {
        if !self.advance() {
            return None;
        }
        Some(self.current())
    }

    /// Advance this cursor to the next directory entry. If there are no more
    /// directory entries to read, then this returns false.
    ///
    /// Calling `current()` after `advance` is guaranteed to panic when this
    /// returns false. Conversely, calling `current()` after `advance` is
    /// guaranteed not to panic when this returns true.
    pub fn advance(&mut self) -> bool {
        if self.is_done() {
            return false;
        }
        if !self.advanced {
            self.advanced = true;
            return true;
        }
        // SAFETY: This is safe by the assumption that `d_reclen` on the raw
        // dirent is correct.
        self.cursor = unsafe {
            let raw = self.current_raw();
            let next = self.cursor.as_ptr().add(raw.record_len());
            NonNull::new_unchecked(next)
        };
        !self.is_done()
    }

    /// Return the current directory entry in this cursor.
    ///
    /// This panics is the cursor has been exhausted, or if the cursor has not
    /// yet had `advance` called.
    ///
    /// Calling `current()` after `advance` is guaranteed to panic when this
    /// returns false. Conversely, calling `current()` after `advance` is
    /// guaranteed not to panic when this returns true.
    pub fn current<'a>(&'a self) -> DirEntry<'a> {
        let raw = self.current_raw();
        DirEntry {
            // SAFETY: This is safe since we are asking for the file name on a
            // `RawDirEntry` that resides in its original buffer.
            file_name: unsafe { raw.file_name() },
            file_type: raw.file_type(),
            ino: raw.ino(),
        }
    }

    fn current_raw(&self) -> &RawDirEntry {
        assert!(self.advanced);
        assert!(!self.is_done());
        // SAFETY: This is safe by the contract of getdents64. Namely, that it
        // writes structures of type `RawDirEntry` to `raw`. The lifetime of
        // this raw dirent is also tied to this buffer via the type signature
        // of this method, which prevents use-after-free. Moreover, our
        // allocation layout guarantees that the cursor is correctly aligned
        // for RawDirEntry. Finally, we assert that self.cursor has not
        // reached the end yet, and since the cursor is only ever incremented
        // by correct amounts, we know it points to the beginning of a valid
        // directory entry.
        unsafe { &*(self.cursor.as_ptr() as *const RawDirEntry) }
    }

    fn is_done(&self) -> bool {
        self.cursor.as_ptr() >= self.raw.as_ptr().wrapping_add(self.len)
    }

    /// Read the next directory entry from this cursor as an owned Unix
    /// `DirEntry`. If the cursor has been exhausted, then return `None`.
    ///
    /// This will allocate new space to store the file name in the directory
    /// entry. To reuse a previous allocation, use `read_unix_into` instead.
    ///
    /// Note that no filtering of entries (such as `.` and `..`) is performed.
    pub fn read_unix(&mut self) -> Option<UnixDirEntry> {
        self.read().map(|ent| ent.to_unix())
    }

    /// Read the next directory entry from this cursor into the given Unix
    /// `DirEntry`. If the cursor has been exhausted, then return `false`.
    /// Otherwise return `true`.
    ///
    /// Note that no filtering of entries (such as `.` and `..`) is performed.
    pub fn read_unix_into(&mut self, unix_dirent: &mut UnixDirEntry) -> bool {
        match self.read() {
            None => false,
            Some(dent) => {
                dent.write_to_unix(unix_dirent);
                true
            }
        }
    }

    /// Rewind this cursor such that it points to the first directory entry.
    pub fn rewind(&mut self) {
        self.cursor = self.raw;
    }

    /// Clear this cursor such that it has no entries.
    fn clear(&mut self) {
        self.cursor = self.raw;
        self.len = 0;
        self.advanced = false;
    }
}
