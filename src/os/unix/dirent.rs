// It turns out that `dirent` is a complete mess across different platforms.
// All POSIX defines is a "struct containing the fields d_ino and d_name" where
// d_name has an unspecified size. And not even that minimal subset is actually
// portable. For example, DragonflyBSD, FreeBSD, NetBSD and OpenBSD all use
// `d_fileno` instead of `d_ino`.
//
// Some platforms (macOS) have a `d_namlen` field indicating the number of
// bytes in `d_name`, while other platforms (Linux) only have a `d_reclen`
// field indicating the total size of the entry.
//
// Finally, not every platform (Solaris) has a `d_type` field, which is insane,
// because that means you need an extra stat call for every directory entry
// in order to do recursive directory traversal.
//
// Rebuilding all this goop that's already done in std really sucks, but if we
// want to specialize even one platform (e.g., using getdents64 on Linux), then
// we wind up needing to specialize ALL of them because std::fs::FileType is
// impossible to either cheaply construct or convert, so we wind up needing to
// roll our own.
//
// So basically what we do here is define a very thin unix-specific but
// platform independent layer on top of all the different dirent formulations
// that we support. We try to avoid costs (e.g., NUL-byte scanning) wherever
// possible.

use std::ffi::CStr;
use std::fmt;
use std::ptr::NonNull;
use std::slice;

use libc::c_char;
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
use libc::dirent;
#[cfg(any(
    target_os = "android",
    target_os = "emscripten",
    target_os = "fuchsia",
    target_os = "linux",
))]
use libc::dirent64 as dirent;

use crate::os::unix::{escaped_bytes, FileType};

/// A low-level Unix-specific but otherwise platform independent API to the
/// underlying platform's `dirent` struct.
#[derive(Clone)]
pub struct RawDirEntry(NonNull<dirent>);

impl fmt::Debug for RawDirEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("RawDirEntry")
            .field("d_name", &escaped_bytes(self.file_name().to_bytes()))
            .field("d_type", &self.file_type())
            .field("d_ino", &self.ino())
            .finish()
    }
}

impl RawDirEntry {
    /// Create a new raw directory entry from the given dirent structure.
    ///
    /// If the given entry is null, then this returns `None`.
    pub fn new(ent: *const dirent) -> Option<RawDirEntry> {
        NonNull::new(ent as *mut _).map(RawDirEntry)
    }

    /// Return the underlying dirent.
    pub fn dirent(&self) -> &dirent {
        // SAFETY: This is safe since we tie the lifetime of the returned
        // dirent to self.
        unsafe { self.0.as_ref() }
    }

    /// Return the file name in this directory entry as a C string.
    pub fn file_name(&self) -> &CStr {
        // This implementation uses namlen to determine the size of file name.
        #[cfg(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "macos",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        fn imp(ent: &RawDirEntry) -> &CStr {
            // SAFETY: This is safe given the guarantee that `d_namlen` is the
            // total number of bytes in the file name, minus the NUL
            // terminator. This is also only safe given the guarantee that
            // `d_name` contains a NUL terminator.
            unsafe {
                let bytes = slice::from_raw_parts(
                    ent.dirent().d_name.as_ptr() as *const u8,
                    ent.dirent().d_namlen as usize + 1, // +1 for NUL
                );
                CStr::from_bytes_with_nul_unchecked(bytes)
            }
        }

        // This implementation uses strlen to determine the size of the file
        // name, since these platforms don't have a `d_namlen` field. Some of
        // them *do* have a `d_reclen` field, which seems like it could help
        // us, but there is no clear documentation on how to use it properly.
        // In particular, it can include the padding between the directory
        // entries, and it's not clear how to account for that.
        #[cfg(any(
            target_os = "android",
            target_os = "emscripten",
            target_os = "fuchsia",
            target_os = "haiku",
            target_os = "hermit",
            target_os = "linux",
            target_os = "solaris",
        ))]
        fn imp(ent: &RawDirEntry) -> &CStr {
            // SAFETY: This is safe since `d_name` is guaranteed to be valid
            // and guaranteed to contain a NUL terminator.
            unsafe {
                CStr::from_ptr(ent.dirent().d_name.as_ptr() as *const c_char)
            }
        }

        imp(self)
    }

    /// Return the file type embedded in this directory entry, if one exists.
    ///
    /// If this platform doesn't support reporting the file type in the
    /// directory entry, or if the file type was reported as unknown, then
    /// this returns `None`.
    pub fn file_type(&self) -> Option<FileType> {
        // This implementation uses the `d_type` field.
        //
        // Note that this can still return None if the value of this field
        // is DT_UNKNOWN.
        #[cfg(any(
            target_os = "android",
            target_os = "emscripten",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "fuchsia",
            target_os = "linux",
            target_os = "macos",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        fn imp(ent: &RawDirEntry) -> Option<FileType> {
            FileType::from_dirent_type(ent.dirent().d_type)
        }

        // No `d_type` field is available here, so always return None.
        #[cfg(any(
            target_os = "haiku",
            target_os = "hermit",
            target_os = "solaris",
        ))]
        fn imp(ent: &RawDirEntry) -> Option<FileType> {
            None
        }

        imp(self)
    }

    /// Return the file serial number for this directory entry.
    pub fn ino(&self) -> u64 {
        // This implementation uses the d_fileno field.
        #[cfg(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        fn imp(ent: &RawDirEntry) -> u64 {
            ent.dirent().d_fileno as u64
        }

        // This implementation uses the d_ino field.
        #[cfg(any(
            target_os = "android",
            target_os = "emscripten",
            target_os = "fuchsia",
            target_os = "macos",
            target_os = "haiku",
            target_os = "hermit",
            target_os = "linux",
            target_os = "solaris",
            target_os = "dragonfly",
        ))]
        fn imp(ent: &RawDirEntry) -> u64 {
            ent.dirent().d_ino as u64
        }

        imp(self)
    }
}
