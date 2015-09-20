#![allow(dead_code, unused_variables, unused_imports)]

use std::cmp::min;
use std::borrow::Cow;
use std::error;
use std::fmt;
use std::fs::{self, DirEntry, Metadata, ReadDir};
use std::io;
use std::path::{Path, PathBuf};
use std::vec;

pub struct WalkDirBuilder<P> {
    root: P,
    opts: WalkDirOptions,
}

struct WalkDirOptions {
    follow_links: bool,
    max_open: usize,
    contents_first: bool,
    min_depth: usize,
    max_depth: usize,
}

impl<P: AsRef<Path>> WalkDirBuilder<P> {
    pub fn new(root: P) -> Self {
        WalkDirBuilder {
            root: root,
            opts: WalkDirOptions {
                follow_links: false,
                max_open: 32,
                contents_first: false,
                min_depth: 0,
                max_depth: ::std::usize::MAX,
            }
        }
    }

    pub fn max_open(mut self, mut n: usize) -> Self {
        // A value of 0 is nonsensical and will prevent the file walker from
        // working in any meaningful sense. So just set the limit to 1.
        if n == 0 {
            n = 1;
        }
        self.opts.max_open = n;
        self
    }

    pub fn follow_links(mut self, yes: bool) -> Self {
        self.opts.follow_links = yes;
        self
    }

    pub fn contents_first(mut self, yes: bool) -> Self {
        self.opts.contents_first = yes;
        self
    }

    pub fn min_depth(mut self, depth: usize) -> Self {
        self.opts.min_depth = depth;
        self
    }

    pub fn max_depth(mut self, depth: usize) -> Self {
        self.opts.max_depth = depth;
        self
    }
}

impl<P: AsRef<Path>> IntoIterator for WalkDirBuilder<P> {
    type Item = Result<DirEntry, WalkDirError>;
    type IntoIter = WalkDir;

    fn into_iter(self) -> WalkDir {
        assert!(self.opts.min_depth <= self.opts.max_depth);
        WalkDir {
            start: Some(self.root.as_ref().to_path_buf()),
            stack: vec![],
            oldest_opened: 0,
            opts: self.opts,
        }
    }
}

pub struct WalkDir {
    start: Option<PathBuf>,
    stack: Vec<StackEntry>,
    oldest_opened: usize,
    opts: WalkDirOptions,
}

struct StackEntry {
    dir: Dir,
    list: DirList,
}

enum Dir {
    Path(PathBuf),
    Entry(DirEntry),
}

enum DirList {
    Opened(Result<ReadDir, Option<WalkDirError>>),
    Closed(vec::IntoIter<Result<DirEntry, WalkDirError>>),
}

impl Iterator for WalkDir {
    type Item = Result<DirEntry, WalkDirError>;

    fn next(&mut self) -> Option<Result<DirEntry, WalkDirError>> {
        macro_rules! walk_try {
            ($dent:expr, $e:expr) => {
                match $e {
                    Ok(v) => v,
                    Err(err) => {
                        let err = WalkDirError::from_io($dent.path(), err);
                        return Some(Err(err));
                    }
                }
            }
        }

        macro_rules! skip {
            ($walkdir:expr, $depth:expr, $ret:expr) => {{
                let d = $depth;
                if d < $walkdir.opts.min_depth || d > $walkdir.opts.max_depth {
                    continue;
                } else {
                    return $ret;
                }
            }}
        }

        if let Some(start) = self.start.take() {
            self.push_path(start, None);
        }
        while !self.stack.is_empty() {
            let depth = self.stack.len() - 1;
            let dent = match self.stack.last_mut().and_then(|v| v.next()) {
                None => {
                    if let Dir::Entry(dent) = self.pop().dir {
                        skip!(self, depth - 1, Some(Ok(dent)));
                    } else {
                        continue;
                    }
                }
                Some(Err(err)) => return Some(Err(err)),
                Some(Ok(dent)) => dent,
            };
            // On both Windows and most unixes, this should not require a
            // syscall. But it's not guaranteed, so only call it once. ---AG
            let mut ty = walk_try!(dent, dent.file_type());
            if ty.is_symlink() {
                if !self.opts.follow_links {
                    skip!(self, depth, Some(Ok(dent)));
                } else {
                    let p = dent.path();
                    ty = walk_try!(dent, fs::metadata(&p)).file_type();
                    assert!(!ty.is_symlink());
                    // The only way a symlink can cause a loop is if it points
                    // to a directory. Otherwise, it always points to a leaf
                    // and we can omit any loop checks.
                    if ty.is_dir() {
                        let looperr = walk_try!(dent, self.loop_error(p));
                        if let Some(err) = looperr {
                            return Some(Err(err));
                        }
                    }
                }
            }
            if ty.is_dir() {
                if depth == self.opts.max_depth {
                    // Don't descend into this directory, just return it.
                    // Since min_depth <= max_depth, we don't need to check
                    // if we're skipping here.
                    return Some(Ok(dent));
                } else if let Some(dent) = self.push(dent) {
                    skip!(self, depth, Some(Ok(dent)));
                }
            } else {
                skip!(self, depth, Some(Ok(dent)));
            }
        }
        None
    }
}

impl WalkDir {
    pub fn skip_current_dir(&mut self) {
        if !self.stack.is_empty() {
            self.stack.pop();
        }
    }

    fn depth(&self) -> usize {
        self.stack.len().saturating_sub(1)
    }

    fn push(&mut self, dent: DirEntry) -> Option<DirEntry> {
        self.push_path(dent.path(), Some(dent))
    }

    fn push_path(
        &mut self,
        p: PathBuf,
        dent: Option<DirEntry>,
    ) -> Option<DirEntry> {
        // Make room for another open file descriptor if we've hit the max.
        if self.stack.len() - self.oldest_opened == self.opts.max_open {
            self.stack[self.oldest_opened].close();
            self.oldest_opened = self.oldest_opened.checked_add(1).unwrap();
        }
        // Open a handle to reading the directory's entries.
        let list = DirList::Opened(fs::read_dir(&p).map_err(|err| {
            Some(WalkDirError::from_io(&p, err))
        }));
        // If we have a dir entry (the only time we don't is when pushing the
        // initial path) and we are enumerating the contents of a directory
        // before the directory itself, then we need to hang on to that dir
        // entry in the stack. Otherwise, we pass the dir entry back to the
        // caller and hang on to a path to the directory instead.
        if self.opts.contents_first && dent.is_some() {
            self.stack.push(StackEntry {
                dir: Dir::Entry(dent.expect("DirEntry")),
                list: list,
            });
            None
        } else {
            self.stack.push(StackEntry {
                dir: Dir::Path(p),
                list: list,
            });
            dent
        }
    }

    fn pop(&mut self) -> StackEntry {
        let ent = self.stack.pop().expect("cannot pop from empty stack");
        // If everything in the stack is already closed, then there is
        // room for at least one more open descriptor and it will
        // always be at the top of the stack.
        self.oldest_opened = min(self.oldest_opened, self.stack.len());
        ent
    }

    fn loop_error(&self, child: PathBuf) -> io::Result<Option<WalkDirError>> {
        for ent in self.stack.iter().rev() {
            let ancestor = ent.dir.path();
            if try!(is_same_file(&ancestor, &child)) {
                return Ok(Some(WalkDirError::Loop {
                    ancestor: ancestor.into_owned(),
                    child: child,
                }));
            }
        }
        Ok(None)
    }
}

impl StackEntry {
    fn close(&mut self) {
        if let DirList::Opened(_) = self.list {
            self.list = DirList::Closed(self.collect::<Vec<_>>().into_iter());
        } else {
            unreachable!("BUG: entry already closed");
        }
    }
}

impl Dir {
    fn path(&self) -> Cow<Path> {
        match *self {
            Dir::Path(ref p) => Cow::Borrowed(p),
            Dir::Entry(ref dent) => Cow::Owned(dent.path()),
        }
    }
}

impl Iterator for StackEntry {
    type Item = Result<DirEntry, WalkDirError>;

    fn next(&mut self) -> Option<Result<DirEntry, WalkDirError>> {
        match self.list {
            DirList::Closed(ref mut it) => it.next(),
            DirList::Opened(ref mut rd) => match *rd {
                Err(ref mut err) => err.take().map(Err),
                Ok(ref mut rd) => match rd.next() {
                    None => None,
                    Some(Ok(dent)) => Some(Ok(dent)),
                    Some(Err(err)) => {
                        let p = self.dir.path().to_path_buf();
                        Some(Err(WalkDirError::from_io(p, err)))
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum WalkDirError {
    Io { path: PathBuf, err: io::Error },
    Loop { ancestor: PathBuf, child: PathBuf },
}

impl WalkDirError {
    fn from_io<P: AsRef<Path>>(p: P, err: io::Error) -> Self {
        WalkDirError::Io {
            path: p.as_ref().to_path_buf(),
            err: err,
        }
    }

    pub fn path(&self) -> &Path {
        match *self {
            WalkDirError::Io { ref path, .. } => path,
            WalkDirError::Loop { ref child, .. } => child,
        }
    }
}

impl error::Error for WalkDirError {
    fn description(&self) -> &str {
        match *self {
            WalkDirError::Io { ref err, .. } => err.description(),
            WalkDirError::Loop { .. } => "file system loop found",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            WalkDirError::Io { ref err, .. } => Some(err),
            WalkDirError::Loop { .. } => None,
        }
    }
}

impl fmt::Display for WalkDirError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            WalkDirError::Io { ref path, ref err } => {
                write!(f, "IO error for operation on {}: {}",
                       path.display(), err)
            }
            WalkDirError::Loop { ref ancestor, ref child } => {
                write!(f, "File system loop found: \
                           {} points to an ancestor {}",
                       child.display(), ancestor.display())
            }
        }
    }
}

// Below are platform specific functions for testing the equality of two
// files. Namely, we want to know whether the two paths points to precisely
// the same underlying file object.
//
// In our particular use case, the paths should only be directories. If we're
// assuming that directories cannot be hard linked, then it seems like equality
// could be determined by canonicalizing both paths.
//
// ---AG

#[cfg(windows)]
fn is_same_file<P, Q>(
    p1: P,
    p2: Q,
) -> io::Result<bool>
where P: AsRef<Path>, Q: AsRef<Path> {
    // My hope is that most of this gets moved/deleted by reusing code in
    // `sys::windows`.
    extern crate libc;

    use std::fs::File;
    use std::mem;
    use std::ops::{Deref, Drop};
    use std::os::windows::prelude::*;
    use std::ptr;

    struct Handle(RawHandle);

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { let _ = libc::CloseHandle(mem::transmute(self.0)); }
        }
    }

    impl Deref for Handle {
        type Target = RawHandle;
        fn deref(&self) -> &RawHandle { &self.0 }
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct BY_HANDLE_FILE_INFORMATION {
        dwFileAttributes: libc::DWORD,
        ftCreationTime: libc::FILETIME,
        ftLastAccessTime: libc::FILETIME,
        ftLastWriteTime: libc::FILETIME,
        dwVolumeSerialNumber: libc::DWORD,
        nFileSizeHigh: libc::DWORD,
        nFileSizeLow: libc::DWORD,
        nNumberOfLinks: libc::DWORD,
        nFileIndexHigh: libc::DWORD,
        nFileIndexLow: libc::DWORD,
    }

    #[allow(non_camel_case_types)]
    type LPBY_HANDLE_FILE_INFORMATION = *mut BY_HANDLE_FILE_INFORMATION;

    fn file_info(h: &Handle) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
        #[link(name = "ws2_32")]
        #[link(name = "userenv")]
        extern "system" {
            fn GetFileInformationByHandle(
                hFile: RawHandle,
                lpFileInformation: LPBY_HANDLE_FILE_INFORMATION,
            ) -> libc::BOOL;
        }

        unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = ::std::mem::zeroed();
            if GetFileInformationByHandle(**h, &mut info) == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(info)
            }
        }
    }

    fn open_read_attr<P: AsRef<Path>>(p: P) -> io::Result<Handle> {
        // Can openfully use OpenOptions in sys::windows. ---AG
        // All of these options should be the default as per
        // sys::windows::fs::OpenOptions except for `flags_and_attributes`.
        // In particular, according to MSDN, `FILE_FLAG_BACKUP_SEMANTICS`
        // must be set in order to get a handle to a directory:
        // https://msdn.microsoft.com/en-us/library/windows/desktop/aa363858(v=vs.85).aspx
        let h = unsafe {
            libc::CreateFileW(
                to_utf16(p.as_ref()).as_ptr(),
                0,
                libc::FILE_SHARE_READ
                | libc::FILE_SHARE_WRITE
                | libc::FILE_SHARE_DELETE,
                ptr::null_mut(),
                libc::OPEN_EXISTING,
                libc::FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut())
        };
        if h == libc::INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Handle(unsafe { mem::transmute(h) }))
        }
    }

    fn to_utf16(s: &Path) -> Vec<u16> {
        s.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    // For correctness, it is critical that both file handles remain open
    // while their attributes are checked for equality. In particular,
    // the file index numbers are not guaranteed to remain stable over time.
    //
    // See the docs and remarks on MSDN:
    // https://msdn.microsoft.com/en-us/library/windows/desktop/aa363788(v=vs.85).aspx
    //
    // It gets worse. It appears that the index numbers are not always
    // guaranteed to be unqiue. Namely, ReFS uses 128 bit numbers for unique
    // identifiers. This requires a distinct syscall to get `FILE_ID_INFO`
    // documented here:
    // https://msdn.microsoft.com/en-us/library/windows/desktop/hh802691(v=vs.85).aspx
    //
    // It seems straight-forward enough to modify this code to use
    // `FILE_ID_INFO` when available (minimum Windows Server 2012), but
    // I don't have access to such Windows machines.
    //
    // Two notes.
    //
    // 1. Java's NIO uses the approach implemented here and appears to ignore
    //    `FILE_ID_INFO` altogether. So Java's NIO and this code are
    //    susceptible to bugs when running on a file system where
    //    `nFileIndex{Low,High}` are not unique.
    //
    // 2. LLVM has a bug where they fetch the id of a file and continue to use
    //    it even after the file has been closed, so that uniqueness is no
    //    longer guaranteed (when `nFileIndex{Low,High}` are unique).
    //    bug report: http://lists.llvm.org/pipermail/llvm-bugs/2014-December/037218.html
    //
    // All said and done, checking whether two files are the same on Windows
    // seems quite tricky. Moreover, even if the code is technically incorrect,
    // it seems like the chances of actually observing incorrect behavior are
    // extremely small.
    let h1 = try!(open_read_attr(&p1));
    let h2 = try!(open_read_attr(&p2));
    let i1 = try!(file_info(&h1));
    let i2 = try!(file_info(&h2));
    Ok((i1.dwVolumeSerialNumber, i1.nFileIndexHigh, i1.nFileIndexLow)
       == (i2.dwVolumeSerialNumber, i2.nFileIndexHigh, i2.nFileIndexLow))
}

#[cfg(unix)]
fn is_same_file<P, Q>(
    p1: P,
    p2: Q,
) -> io::Result<bool>
where P: AsRef<Path>, Q: AsRef<Path> {
    use std::os::unix::fs::MetadataExt;
    let md1 = try!(fs::metadata(p1));
    let md2 = try!(fs::metadata(p2));
    Ok((md1.ino(), md1.dev()) == (md2.ino(), md2.dev()))
}
