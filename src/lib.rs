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
}

impl<P: AsRef<Path>> WalkDirBuilder<P> {
    pub fn new(root: P) -> Self {
        WalkDirBuilder {
            root: root,
            opts: WalkDirOptions {
                follow_links: false,
                max_open: 32,
                contents_first: false,
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
}

impl<P: AsRef<Path>> IntoIterator for WalkDirBuilder<P> {
    type Item = Result<DirEntry, WalkDirError>;
    type IntoIter = WalkDir;

    fn into_iter(self) -> WalkDir {
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

        if let Some(start) = self.start.take() {
            self.push_path(start, None);
        }
        while !self.stack.is_empty() {
            let dent = match self.stack.last_mut().and_then(|v| v.next()) {
                None => {
                    if let Dir::Entry(dent) = self.pop().dir {
                        return Some(Ok(dent));
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
                    return Some(Ok(dent));
                } else {
                    let symlink = dent.path();
                    ty = walk_try!(dent, fs::metadata(&symlink)).file_type();
                    assert!(!ty.is_symlink());

                    let looperr = walk_try!(dent, self.loop_error(symlink));
                    if let Some(err) = looperr {
                        return Some(Err(err));
                    }
                }
            }
            if ty.is_dir() {
                if let Some(dent) = self.push(dent) {
                    return Some(Ok(dent));
                }
            } else {
                return Some(Ok(dent));
            }
        }
        None
    }
}

impl WalkDir {
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

#[cfg(windows)]
fn is_same_file<P, Q>(
    p1: P,
    p2: Q,
) -> io::Result<bool>
where P: AsRef<Path>, Q: AsRef<Path> {
    // My hope is that most of this gets moved into `std::sys::windows`. ---AG
    extern crate libc;

    use std::fs::File;
    use std::os::windows::prelude::*;
    use std::ptr;

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

    #[link(name = "ws2_32")]
    #[link(name = "userenv")]
    extern "system" {
        fn GetFileInformationByHandle(
            hFile: RawHandle,
            lpFileInformation: LPBY_HANDLE_FILE_INFORMATION,
        ) -> libc::BOOL;
    }

    fn file_info(h: RawHandle) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
        unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = ::std::mem::zeroed();
            if GetFileInformationByHandle(h, &mut info) == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(info)
            }
        }
    }

    fn open_read_attr<P: AsRef<Path>>(p: P) -> io::Result<RawHandle> {
        let h = unsafe {
            libc::CreateFileW(
                to_utf16(p.as_ref()).as_ptr(),
                libc::FILE_READ_ATTRIBUTES,
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
            Ok(unsafe { ::std::mem::transmute(h) })
        }
    }

    fn to_utf16(s: &Path) -> Vec<u16> {
        s.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let h1 = try!(open_read_attr(&p1));
    let h2 = try!(open_read_attr(&p2));
    let i1 = try!(file_info(h1));
    let i2 = try!(file_info(h2));
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
    Ok((md1.dev(), md1.ino()) == (md2.dev(), md2.ino()))
}
