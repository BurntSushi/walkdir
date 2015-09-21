use std::cmp::min;
use std::borrow::Cow;
use std::error;
use std::fmt;
use std::fs::{self, DirEntry, ReadDir};
use std::io;
use std::path::{Path, PathBuf};
use std::vec;

use same_file::is_same_file;

mod same_file;

/// Create an iterator to recursively walk a directory.
pub struct WalkDir<P> {
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

impl<P: AsRef<Path>> WalkDir<P> {
    pub fn new(root: P) -> Self {
        WalkDir {
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

impl<P: AsRef<Path>> IntoIterator for WalkDir<P> {
    type Item = Result<DirEntry, WalkDirError>;
    type IntoIter = WalkDirIter;

    fn into_iter(self) -> WalkDirIter {
        assert!(self.opts.min_depth <= self.opts.max_depth);
        WalkDirIter {
            opts: self.opts,
            start: Some(self.root.as_ref().to_path_buf()),
            stack: vec![],
            oldest_opened: 0,
            depth: 0,
        }
    }
}

pub struct WalkDirIter {
    opts: WalkDirOptions,
    start: Option<PathBuf>,
    stack: Vec<StackEntry>,
    oldest_opened: usize,
    depth: usize,
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

impl Iterator for WalkDirIter {
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
            ($walkdir:expr, $ret:expr) => {{
                let d = $walkdir.depth;
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
            self.depth = self.stack.len() - 1;
            let dent = match self.stack.last_mut().and_then(|v| v.next()) {
                None => {
                    if let Dir::Entry(dent) = self.pop().dir {
                        self.depth -= 1;
                        skip!(self, Some(Ok(dent)));
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
                    skip!(self, Some(Ok(dent)));
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
                if self.depth == self.opts.max_depth {
                    // Don't descend into this directory, just return it.
                    // Since min_depth <= max_depth, we don't need to check
                    // if we're skipping here.
                    //
                    // Note that this is a perf optimization and is not
                    // required for correctness.
                    return Some(Ok(dent));
                } else if let Some(dent) = self.push(dent) {
                    skip!(self, Some(Ok(dent)));
                }
            } else {
                skip!(self, Some(Ok(dent)));
            }
        }
        None
    }
}

impl WalkDirIter {
    pub fn skip_current_dir(&mut self) {
        if !self.stack.is_empty() {
            self.stack.pop();
        }
    }

    pub fn depth(&self) -> usize {
        self.depth
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

impl From<WalkDirError> for io::Error {
    fn from(err: WalkDirError) -> io::Error {
        match err {
            WalkDirError::Io { err, .. } => err,
            err @ WalkDirError::Loop { .. } => {
                io::Error::new(io::ErrorKind::Other, err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code, unused_imports)]

    extern crate rand;

    use std::env;
    use std::fs::{self, File};
    use std::io;
    use std::path::{Path, PathBuf};

    use self::rand::Rng;

    use super::{WalkDir, WalkDirError};

    struct TempDir(PathBuf);

    impl TempDir {
        fn join(&self, path: &str) -> PathBuf {
            (&*self.0).join(path)
        }

        fn path<'a>(&'a self) -> &'a Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn tmpdir() -> TempDir {
        let p = env::temp_dir();
        let mut r = rand::thread_rng();
        let ret = p.join(&format!("rust-{}", r.next_u32()));
        fs::create_dir(&ret).unwrap();
        TempDir(ret)
    }

    fn p<P: AsRef<Path>>(path: P) -> PathBuf { path.as_ref().to_path_buf() }

    #[derive(Debug, Eq, PartialEq)]
    enum Tree {
        Dir(PathBuf, Vec<Tree>),
        File(PathBuf),
    }

    impl Tree {
        fn from_walk<P: AsRef<Path>>(root: P) -> io::Result<Tree> {
            let mut tree = Tree::Dir(root.as_ref().to_path_buf(), vec![]);
            let mut it = WalkDir::new(root).into_iter();
            loop {
                let dent = match it.next() {
                    None => break,
                    Some(dent) => try!(dent),
                };
                let name =
                    AsRef::<Path>::as_ref(&dent.file_name()).to_path_buf();
                let child = if try!(dent.file_type()).is_dir() {
                    Tree::Dir(name, vec![])
                } else {
                    Tree::File(name)
                };
                tree.last_dir_at(it.depth()).add(child);
            }
            Ok(tree)
        }

        fn last_dir_at(&mut self, depth: usize) -> &mut Tree {
            if depth == 0 {
                self
            } else {
                self.last().last_dir_at(depth - 1)
            }
        }

        fn last(&mut self) -> &mut Tree {
            match *self {
                Tree::File(_) => panic!("cannot take last child of file"),
                Tree::Dir(_, ref mut childs) => childs.last_mut().unwrap(),
            }
        }

        fn unwrap_singleton(self) -> Tree {
            match self {
                Tree::File(_) => panic!("cannot unwrap file as dir"),
                Tree::Dir(_, mut childs) => {
                    assert_eq!(childs.len(), 1);
                    childs.pop().unwrap()
                }
            }
        }

        fn add(&mut self, tree: Tree) {
            match *self {
                Tree::File(_) => panic!("cannot add child to file"),
                Tree::Dir(_, ref mut childs) => childs.push(tree),
            }
        }

        fn create_in<P: AsRef<Path>>(&self, parent: P) -> io::Result<()> {
            let parent = parent.as_ref();
            match *self {
                Tree::File(ref p) => { try!(File::create(parent.join(p))); }
                Tree::Dir(ref dir, ref children) => {
                    try!(fs::create_dir(parent.join(dir)));
                    for child in children {
                        try!(child.create_in(parent.join(dir)));
                    }
                }
            }
            Ok(())
        }
    }

    #[test]
    fn walk_dir() {
        let tree = Tree::Dir(p("foo"), vec![Tree::File(p("bar"))]);
        let tmpdir = tmpdir();
        tree.create_in(tmpdir.path()).unwrap();
        let got = Tree::from_walk(tmpdir.path()).unwrap().unwrap_singleton();
        assert_eq!(tree, got);
    }

    #[test]
    fn file_test_walk_dir() {
        let tmpdir = tmpdir();
        let dir = &tmpdir.join("walk_dir");
        fs::create_dir(dir).unwrap();

        let dir1 = &dir.join("01/02/03");
        fs::create_dir_all(dir1).unwrap();
        File::create(&dir1.join("04")).unwrap();

        let dir2 = &dir.join("11/12/13");
        fs::create_dir_all(dir2).unwrap();
        File::create(&dir2.join("14")).unwrap();

        let mut cur = [0; 2];
        for f in WalkDir::new(dir) {
            let f = f.unwrap().path();
            let stem = f.file_stem().unwrap().to_str().unwrap();
            let root = stem.as_bytes()[0] - b'0';
            let name = stem.as_bytes()[1] - b'0';
            assert!(cur[root as usize] < name);
            cur[root as usize] = name;
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
