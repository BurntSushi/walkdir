use std::ffi::{CStr, CString, OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

// Currently, these types are not exported in the public API of this crate,
// even though they (or something like them) are seemingly necessary to
// implement recursive directory traversal without superfluous allocations.
// Figuring out how to expose them is tricky, since invariably, they _aren't_
// the same type with the same API. So they wind up being a hazard if one
// accidentally tries to treat them as a platform independent type.

/// A platform dependent representation of a file path.
///
/// Unlike Rust's standard library `PathBuf`, a `RawPathBuf` uses the same
/// in-memory representation of a file path as the platform itself. Moreover,
/// the APIs of each `RawPathBuf` are also platform dependent. For example,
/// on Unix, a `RawPathBuf` can be cheaply converted between types such as
/// `Vec<u8>` and `CString`. But on Windows, since its internal representation
/// is a sequence of 16-bit integers, these conversions are not available.
#[derive(Clone)]
pub struct RawPathBuf {
    /// Buf always has length at least 1 and always ends with a zero byte.
    /// Buf only ever contains exactly 1 zero byte. (i.e., no interior NULs.)
    buf: Vec<u8>,
}

impl fmt::Debug for RawPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use crate::os::unix::escaped_bytes;

        f.debug_struct("RawPathBuf")
            .field("buf", &escaped_bytes(self.as_code_units()))
            .finish()
    }
}

impl<'a> From<&'a str> for RawPathBuf {
    fn from(s: &'a str) -> RawPathBuf {
        RawPathBuf::from(s.to_string())
    }
}

impl From<String> for RawPathBuf {
    fn from(s: String) -> RawPathBuf {
        let mut buf = s.into_bytes();
        buf.push(0);
        RawPathBuf { buf }
    }
}

impl From<CString> for RawPathBuf {
    fn from(cstr: CString) -> RawPathBuf {
        RawPathBuf { buf: cstr.into_bytes_with_nul() }
    }
}

impl From<RawPathBuf> for CString {
    fn from(rawp: RawPathBuf) -> CString {
        // SAFETY: Our internal buffer is guaranteed to end with a NUL and have
        // no interior NULs.
        unsafe { CString::from_vec_unchecked(rawp.buf) }
    }
}

impl From<OsString> for RawPathBuf {
    fn from(osstr: OsString) -> RawPathBuf {
        let mut buf = osstr.into_vec();
        buf.push(0);
        RawPathBuf { buf }
    }
}

impl From<RawPathBuf> for OsString {
    fn from(mut rawp: RawPathBuf) -> OsString {
        // SAFETY: We are dropping this raw path and converting it into an
        // OS string, which has no NUL terminator.
        unsafe {
            rawp.drop_nul();
        }
        OsString::from_vec(rawp.buf)
    }
}

impl From<PathBuf> for RawPathBuf {
    fn from(path: PathBuf) -> RawPathBuf {
        RawPathBuf::from(path.into_os_string())
    }
}

impl From<RawPathBuf> for PathBuf {
    fn from(rawp: RawPathBuf) -> PathBuf {
        PathBuf::from(OsString::from(rawp))
    }
}

impl RawPathBuf {
    /// Returns the code units (bytes) of this path without the NUL terminator.
    pub fn as_code_units(&self) -> &[u8] {
        &self.buf[..self.buf.len() - 1]
    }

    /// Returns this raw path as a C string slice.
    pub fn as_cstr(&self) -> &CStr {
        // SAFETY: buf is guaranteed to have a NUL terminator with no interior
        // NULs.
        unsafe { CStr::from_bytes_with_nul_unchecked(&self.buf) }
    }

    /// Returns this raw path as a OS string slice.
    pub fn as_os_str(&self) -> &OsStr {
        OsStr::from_bytes(self.as_code_units())
    }

    /// Return this raw path as a standard library path.
    pub fn as_path(&self) -> &Path {
        Path::new(self.as_os_str())
    }

    /// Push the given C string slice to the end of this path.
    pub fn push_cstr(&mut self, slice: &CStr) {
        // SAFETY: The internal buffer is guaranteed to have a NUL byte at
        // this point, and we always add it back below via the CStr's NUL
        // byte.
        unsafe {
            self.drop_nul();
        }
        self.buf.extend_from_slice(slice.to_bytes_with_nul());
    }

    /// Join the given C string slice to this path in place via a path
    /// separator.
    ///
    /// If this path ends with a `/`, and/or if name starts with a `/`, then
    /// only one separator will be used to join them. This otherwise does no
    /// other normalization. e.g., joining `a/b//` with `/c` will result in
    /// `a/b//c`.
    pub fn join(&mut self, name: &CStr) {
        // SAFETY: The internal buffer is guaranteed to have a NUL byte at
        // this point, and we always add it back below via the CStr's NUL
        // byte.
        unsafe {
            self.drop_nul();
        }
        if self.buf.last() != Some(&b'/') {
            self.buf.push(b'/');
        }
        if name.to_bytes().get(0) == Some(&b'/') {
            debug_assert_eq!(self.buf.last(), Some(&b'/'));
            self.buf.pop();
        }
        self.buf.extend_from_slice(name.to_bytes_with_nul());
    }

    /// Pop the last element in this path. Return true if an element was
    /// popped. An element isn't popped if the path is empty or represents
    /// a root path.
    pub fn pop(&mut self) -> bool {
        // Move backwards through the path, finding the first location that
        // ends the parent element, if one exists. Basically, we want to
        // implement the following regex:
        //
        //     ^.*?(/*[^/]+/*)$
        //
        // Where everything in the capturing group is deleted.

        // First, start by skipping through all repeated separators in reverse.
        let mut new_len = self.buf.len() - 1;
        while new_len > 0 && self.buf[new_len - 1] == b'/' {
            new_len -= 1;
        }
        // The path is either empty, or just made up of separators.
        if new_len == 0 {
            return false;
        }
        // Now find either the first preceding / or the beginning.
        while new_len > 0 && self.buf[new_len - 1] != b'/' {
            new_len -= 1;
        }
        // And now finally, remove all trailing separators.
        // But we're careful not to remove a root slash if it's present.
        while new_len > 1 && self.buf[new_len - 1] == b'/' {
            new_len -= 1;
        }
        self.buf[new_len] = 0;

        // SAFETY: This is safe because our buffer contains Copy data and
        // `new_len + 1` is guaranteed to be <= the original length of the
        // buffer. Therefore, we do not need to worry about unitialized data.
        unsafe {
            self.buf.set_len(new_len + 1);
        }
        true
    }

    /// Drop the trailing NUL byte from the internal buffer in place.
    ///
    /// # Safety
    ///
    /// This is unsafe to call because it removes the NUL byte from the buffer,
    /// which is necessary for safety in many contexts.
    ///
    /// When callers use this method, they MUST ensure that a NUL byte is
    /// added back to the internal buffer before its absence can be observed
    /// by callers.
    ///
    /// Callers must also never call this method if the NUL byte has already
    /// been removed.
    unsafe fn drop_nul(&mut self) {
        // SAFETY: This is safe since the new length is always <= than the
        // old length, and thus there are no initialization worries. Moreover,
        // since the buffer stores Copy data, there are no leaks.
        debug_assert_eq!(*self.buf.last().unwrap(), 0);
        self.buf.set_len(self.buf.len() - 1);
    }

    /// Add a trailing NUL byte to the internal buffer.
    ///
    /// # Safety
    ///
    /// This is unsafe to call because it could create an interior NUL byte
    /// if the internal buffer already ends with a NUL byte. Therefore, this
    /// must only be called when the caller knows that the buffer does not end
    /// with a NUL byte.
    unsafe fn add_nul(&mut self) {
        debug_assert_ne!(*self.buf.last().unwrap(), 0);
        self.buf.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn tostr(p: &RawPathBuf) -> &str {
        std::str::from_utf8(p.as_code_units()).unwrap()
    }

    fn cstr(s: &str) -> &CStr {
        CStr::from_bytes_with_nul(s.as_bytes()).unwrap()
    }

    #[test]
    fn push1() {
        let mut p = RawPathBuf::from("a/b");
        p.join(cstr("c\0"));
        assert_eq!("a/b/c", tostr(&p));
    }

    #[test]
    fn push2() {
        let mut p = RawPathBuf::from("a/b/");
        p.join(cstr("c\0"));
        assert_eq!("a/b/c", tostr(&p));
    }

    #[test]
    fn push3() {
        let mut p = RawPathBuf::from("a/b");
        p.join(cstr("/c\0"));
        assert_eq!("a/b/c", tostr(&p));
    }

    #[test]
    fn push4() {
        let mut p = RawPathBuf::from("a/b/");
        p.join(cstr("/c\0"));
        assert_eq!("a/b/c", tostr(&p));
    }

    #[test]
    fn push5() {
        let mut p = RawPathBuf::from("a/b//");
        p.join(cstr("/c\0"));
        assert_eq!("a/b//c", tostr(&p));
    }

    #[test]
    fn pop1() {
        let mut p = RawPathBuf::from("/foo/bar////baz/");

        assert!(p.pop());
        assert_eq!("/foo/bar", tostr(&p));

        assert!(p.pop());
        assert_eq!("/foo", tostr(&p));

        assert!(p.pop());
        assert_eq!("/", tostr(&p));

        assert!(!p.pop());
        assert_eq!("/", tostr(&p));
    }

    #[test]
    fn pop2() {
        let mut p = RawPathBuf::from("////foo/");

        assert!(p.pop());
        assert_eq!("/", tostr(&p));

        assert!(!p.pop());
        assert_eq!("/", tostr(&p));
    }

    #[test]
    fn pop3() {
        let mut p = RawPathBuf::from("foo/bar/baz");

        assert!(p.pop());
        assert_eq!("foo/bar", tostr(&p));

        assert!(p.pop());
        assert_eq!("foo", tostr(&p));

        assert!(p.pop());
        assert_eq!("", tostr(&p));

        assert!(!p.pop());
        assert_eq!("", tostr(&p));
    }

    #[test]
    fn pop4() {
        let mut p = RawPathBuf::from("////");

        assert!(!p.pop());
        assert_eq!("////", tostr(&p));
    }

    #[test]
    fn pop5() {
        let mut p = RawPathBuf::from("////a");

        assert!(p.pop());
        assert_eq!("/", tostr(&p));

        assert!(!p.pop());
        assert_eq!("/", tostr(&p));
    }

    #[test]
    fn pop6() {
        let mut p = RawPathBuf::from("foo");

        assert!(p.pop());
        assert_eq!("", tostr(&p));

        assert!(!p.pop());
        assert_eq!("", tostr(&p));
    }
}
