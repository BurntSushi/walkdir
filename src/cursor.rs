use std::cmp;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use crate::os::unix as os;
#[cfg(windows)]
use crate::os::windows as os;

#[derive(Debug)]
pub struct Cursor {
    options: Options,
    stack: Vec<DirCursor>,

    root: bool,
    current: PathBuf,
    file_type: Option<FileType>,
}

impl Cursor {
    pub fn new<P: Into<PathBuf>>(root: P) -> Cursor {
        Cursor {
            options: Options::default(),
            stack: vec![],
            root: true,
            current: root.into(),
            file_type: None,
        }
    }

    pub fn reset<P: Into<PathBuf>>(root: P) {
        unimplemented!()
    }

    pub fn read(&mut self) -> io::Result<Option<CursorEntry>> {
        if let Some(ft) = self.file_type.take() {
            if !ft.is_dir() {
                self.current.pop();
            }
        } else {
            let ft = os::stat(self.current.clone())?.file_type().into_api();
            if ft.is_dir() {
                self.push();
            }
            self.file_type = Some(ft);
            return Ok(Some(CursorEntry { cursor: self }));
        }
        while !self.stack.is_empty() {
            let dcur = self.stack.last_mut().unwrap();
            match dcur.read() {
                None => {
                    self.stack.pop().unwrap();
                    // If the stack is empty, then we've reached the root.
                    // At this point, `current` is just the original root path,
                    // so we should not pop anything from it.
                    if !self.stack.is_empty() {
                        self.current.pop();
                    }
                }
                Some(Err(err)) => return Err(err),
                Some(Ok(dent)) => {
                    let name = dent.file_name_os();
                    if name == "." || name == ".." {
                        continue;
                    }
                    self.current.push(name);
                    self.file_type =
                        Some(dent.file_type().unwrap().into_api());
                    if dent.file_type().unwrap().is_dir() {
                        self.push();
                    }
                    return Ok(Some(CursorEntry { cursor: self }));
                }
            }
        }
        Ok(None)
    }

    fn push(&mut self) {
        let res = os::Dir::open(self.current.clone());
        self.stack.push(DirCursor(res.map_err(Some)));
    }
}

#[derive(Debug)]
struct DirCursor(Result<os::Dir, Option<io::Error>>);

impl DirCursor {
    fn read(&mut self) -> Option<io::Result<os::DirEntry>> {
        match self.0 {
            Err(ref mut err) => err.take().map(Err),
            Ok(ref mut dir) => dir.read(),
        }
    }
}

#[derive(Debug)]
pub struct CursorEntry<'a> {
    cursor: &'a mut Cursor,
}

impl<'a> CursorEntry<'a> {
    pub fn path(&self) -> &Path {
        &self.cursor.current
    }

    pub fn file_type(&self) -> FileType {
        self.cursor.file_type.unwrap()
    }
}

#[derive(Debug)]
struct Options {
    follow_links: bool,
    max_open: usize,
    min_depth: usize,
    max_depth: usize,
    sorter: Option<Sorter>,
    contents_first: bool,
    same_file_system: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            follow_links: false,
            max_open: 10,
            min_depth: 0,
            max_depth: std::usize::MAX,
            sorter: None,
            contents_first: false,
            same_file_system: false,
        }
    }
}

struct Sorter(
    Box<FnMut(&DirEntry, &DirEntry) -> cmp::Ordering + Send + Sync + 'static>,
);

impl fmt::Debug for Sorter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<sort function>")
    }
}

#[derive(Debug)]
pub struct DirEntry {
    os: os::DirEntry,
    file_type: FileType,
}

impl DirEntry {}

#[derive(Clone, Copy, Debug)]
pub struct FileType(os::FileType);

impl FileType {
    pub fn is_file(&self) -> bool {
        self.0.is_file()
    }

    pub fn is_dir(&self) -> bool {
        self.0.is_dir()
    }

    pub fn is_symlink(&self) -> bool {
        self.0.is_symlink()
    }
}

impl From<os::FileType> for FileType {
    fn from(osft: os::FileType) -> FileType {
        FileType(osft)
    }
}
