#[cfg(walkdir_unix)]
use std::ffi::CStr;
#[cfg(not(any(walkdir_unix, windows)))]
use std::fs;
use std::io;
#[cfg(walkdir_unix)]
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};

#[cfg(walkdir_getdents)]
use crate::os::linux;
#[cfg(walkdir_unix)]
use crate::os::unix::{Dir, DirEntry as OsDirEntry};
#[cfg(windows)]
use crate::os::windows::{Dir as WindowsDir, DirEntry as WindowsDirEntry};

#[cfg(not(any(walkdir_unix, windows)))]
#[derive(Debug)]
pub struct DirList {
    depth: usize,
    path: PathBuf,
    scratch: Option<fs::DirEntry>,
    stream: Stream,
}

#[cfg(not(any(walkdir_unix, windows)))]
#[derive(Debug)]
enum Stream {
    Open(fs::ReadDir),
    Spilled(std::vec::IntoIter<io::Result<fs::DirEntry>>),
    Built(std::vec::IntoIter<crate::Result<crate::DirEntry>>),
    Failed(Option<io::Error>),
}

#[cfg(not(any(walkdir_unix, windows)))]
impl DirList {
    pub fn open_path(depth: usize, path: PathBuf) -> DirList {
        let stream = match fs::read_dir(&path) {
            Ok(iter) => Stream::Open(iter),
            Err(err) => Stream::Failed(Some(err)),
        };
        DirList { depth, path, scratch: None, stream }
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.stream, Stream::Failed(_))
    }

    pub fn into_built(
        self,
        entries: Vec<crate::Result<crate::DirEntry>>,
    ) -> DirList {
        DirList {
            depth: self.depth,
            path: self.path,
            scratch: self.scratch,
            stream: Stream::Built(entries.into_iter()),
        }
    }

    pub fn next_built(&mut self) -> Option<crate::Result<crate::DirEntry>> {
        match self.stream {
            Stream::Built(ref mut iter) => iter.next(),
            _ => None,
        }
    }

    pub fn is_built(&self) -> bool {
        matches!(self.stream, Stream::Built(_))
    }

    pub fn next(&mut self) -> Option<io::Result<()>> {
        let result = match self.stream {
            Stream::Open(ref mut iter) => iter.next()?,
            Stream::Spilled(ref mut iter) => iter.next()?,
            Stream::Built(_) => return None,
            Stream::Failed(ref mut err) => return err.take().map(Err),
        };
        match result {
            Ok(entry) => {
                self.scratch = Some(entry);
                Some(Ok(()))
            }
            Err(err) => Some(Err(err)),
        }
    }

    pub fn entry(&self) -> &fs::DirEntry {
        self.scratch.as_ref().expect("entry follows a successful read")
    }

    pub fn spill(&mut self) {
        let stream = std::mem::replace(&mut self.stream, Stream::Failed(None));
        self.stream = match stream {
            Stream::Open(iter) => {
                Stream::Spilled(iter.collect::<Vec<_>>().into_iter())
            }
            stream => stream,
        };
    }
}

#[cfg(walkdir_unix)]
#[derive(Debug)]
pub struct DirList {
    depth: usize,
    path: PathBuf,
    done: bool,
    scratch: OsDirEntry,
    stream: Stream,
}

#[cfg(walkdir_unix)]
#[derive(Debug)]
enum Stream {
    Open {
        dir: Dir,
        #[cfg(walkdir_getdents)]
        cursor: linux::DirEntryCursor,
    },
    Spilled(std::vec::IntoIter<io::Result<OsDirEntry>>),
    Built(std::vec::IntoIter<crate::Result<crate::DirEntry>>),
    Failed(Option<io::Error>),
}

#[cfg(walkdir_unix)]
impl DirList {
    fn from_open(
        depth: usize,
        path: PathBuf,
        result: io::Result<Dir>,
    ) -> DirList {
        let stream = match result {
            Ok(dir) => Stream::Open {
                dir,
                #[cfg(walkdir_getdents)]
                cursor: linux::DirEntryCursor::new(),
            },
            Err(err) => Stream::Failed(Some(err)),
        };
        DirList {
            depth,
            path,
            done: false,
            scratch: OsDirEntry::empty(),
            stream,
        }
    }

    pub fn open_path(
        depth: usize,
        path: PathBuf,
        path_c: &CStr,
        follow: bool,
    ) -> DirList {
        DirList::from_open(depth, path, Dir::open_c_follow(path_c, follow))
    }

    pub fn openat(
        depth: usize,
        parent: RawFd,
        name: &CStr,
        follow: bool,
        path: PathBuf,
    ) -> DirList {
        DirList::from_open(
            depth,
            path,
            Dir::openat_c_follow(parent, name, follow),
        )
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn parent_handle(&self) -> Option<RawFd> {
        match self.stream {
            Stream::Open { ref dir, .. } => Some(dir.as_raw_fd()),
            _ => None,
        }
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.stream, Stream::Failed(_))
    }

    pub fn into_built(
        self,
        entries: Vec<crate::Result<crate::DirEntry>>,
    ) -> DirList {
        DirList {
            depth: self.depth,
            path: self.path,
            done: false,
            scratch: self.scratch,
            stream: Stream::Built(entries.into_iter()),
        }
    }

    pub fn next_built(&mut self) -> Option<crate::Result<crate::DirEntry>> {
        match self.stream {
            Stream::Built(ref mut iter) => iter.next(),
            _ => None,
        }
    }

    pub fn is_built(&self) -> bool {
        matches!(self.stream, Stream::Built(_))
    }

    pub fn next(&mut self) -> Option<io::Result<()>> {
        match self.stream {
            Stream::Failed(ref mut err) => err.take().map(Err),
            Stream::Spilled(ref mut iter) => match iter.next()? {
                Ok(entry) => {
                    self.scratch = entry;
                    Some(Ok(()))
                }
                Err(err) => Some(Err(err)),
            },
            Stream::Built(_) => None,
            Stream::Open { .. } => self.next_open(),
        }
    }

    pub fn entry(&self) -> &OsDirEntry {
        &self.scratch
    }

    #[cfg(walkdir_getdents)]
    fn next_open(&mut self) -> Option<io::Result<()>> {
        if self.done {
            return None;
        }
        let result = match self.stream {
            Stream::Open { ref mut dir, ref mut cursor } => loop {
                if cursor.advance() {
                    cursor.current().write_to_unix(&mut self.scratch);
                    break Ok(true);
                }
                match linux::getdents(dir.as_raw_fd(), cursor) {
                    Ok(false) => break Ok(false),
                    Ok(true) => continue,
                    Err(ref err)
                        if err.kind() == io::ErrorKind::Interrupted =>
                    {
                        continue;
                    }
                    Err(err) => break Err(err),
                }
            },
            _ => unreachable!(),
        };
        self.finish_read(result)
    }

    #[cfg(not(walkdir_getdents))]
    fn next_open(&mut self) -> Option<io::Result<()>> {
        if self.done {
            return None;
        }
        let result = match self.stream {
            Stream::Open { ref mut dir } => loop {
                match dir.read_into(&mut self.scratch) {
                    Err(ref err)
                        if err.kind() == io::ErrorKind::Interrupted =>
                    {
                        continue;
                    }
                    result => break result,
                }
            },
            _ => unreachable!(),
        };
        self.finish_read(result)
    }

    fn finish_read(
        &mut self,
        result: io::Result<bool>,
    ) -> Option<io::Result<()>> {
        match result {
            Ok(true) => Some(Ok(())),
            Ok(false) => {
                self.done = true;
                None
            }
            Err(err) => {
                self.done = true;
                Some(Err(err))
            }
        }
    }

    pub fn spill(&mut self) {
        if let Stream::Open { .. } = self.stream {
            let mut entries = Vec::new();
            while let Some(result) = self.next_open() {
                entries.push(result.map(|()| self.scratch.clone()));
            }
            self.stream = Stream::Spilled(entries.into_iter());
        }
    }
}

#[cfg(windows)]
#[derive(Debug)]
pub struct DirList {
    depth: usize,
    path: PathBuf,
    done: bool,
    scratch: WindowsDirEntry,
    stream: Stream,
}

#[cfg(windows)]
#[derive(Debug)]
enum Stream {
    Open(WindowsDir),
    Spilled(std::vec::IntoIter<io::Result<WindowsDirEntry>>),
    Built(std::vec::IntoIter<crate::Result<crate::DirEntry>>),
    Failed(Option<io::Error>),
}

#[cfg(windows)]
impl DirList {
    pub fn open_path(depth: usize, path: PathBuf, follow: bool) -> DirList {
        let stream = match WindowsDir::open_path_follow(&path, follow) {
            Ok(dir) => Stream::Open(dir),
            Err(err) => Stream::Failed(Some(err)),
        };
        DirList {
            depth,
            path,
            done: false,
            scratch: WindowsDirEntry::empty(),
            stream,
        }
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.stream, Stream::Failed(_))
    }

    pub fn into_built(
        self,
        entries: Vec<crate::Result<crate::DirEntry>>,
    ) -> DirList {
        DirList {
            depth: self.depth,
            path: self.path,
            done: false,
            scratch: self.scratch,
            stream: Stream::Built(entries.into_iter()),
        }
    }

    pub fn next_built(&mut self) -> Option<crate::Result<crate::DirEntry>> {
        match self.stream {
            Stream::Built(ref mut iter) => iter.next(),
            _ => None,
        }
    }

    pub fn is_built(&self) -> bool {
        matches!(self.stream, Stream::Built(_))
    }

    pub fn next(&mut self) -> Option<io::Result<()>> {
        match self.stream {
            Stream::Failed(ref mut err) => err.take().map(Err),
            Stream::Spilled(ref mut iter) => match iter.next()? {
                Ok(entry) => {
                    self.scratch = entry;
                    Some(Ok(()))
                }
                Err(err) => Some(Err(err)),
            },
            Stream::Built(_) => None,
            Stream::Open(_) => self.next_open(),
        }
    }

    pub fn entry(&self) -> &WindowsDirEntry {
        &self.scratch
    }

    fn next_open(&mut self) -> Option<io::Result<()>> {
        if self.done {
            return None;
        }
        let result = match self.stream {
            Stream::Open(ref mut dir) => loop {
                match dir.read_into(&mut self.scratch) {
                    Err(ref err)
                        if err.kind() == io::ErrorKind::Interrupted =>
                    {
                        continue;
                    }
                    result => break result,
                }
            },
            _ => unreachable!(),
        };
        match result {
            Ok(true) => Some(Ok(())),
            Ok(false) => {
                self.done = true;
                None
            }
            Err(err) => {
                self.done = true;
                Some(Err(err))
            }
        }
    }

    pub fn spill(&mut self) {
        if let Stream::Open(_) = self.stream {
            let mut entries = Vec::new();
            while let Some(result) = self.next_open() {
                entries.push(result.map(|()| self.scratch.clone()));
            }
            self.stream = Stream::Spilled(entries.into_iter());
        }
    }
}
