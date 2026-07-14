use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct DirList {
    depth: usize,
    path: PathBuf,
    scratch: Option<fs::DirEntry>,
    stream: Stream,
}

#[derive(Debug)]
enum Stream {
    Open(fs::ReadDir),
    Spilled(std::vec::IntoIter<io::Result<fs::DirEntry>>),
    Built(std::vec::IntoIter<crate::Result<crate::DirEntry>>),
    Failed(Option<io::Error>),
}

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
