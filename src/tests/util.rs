use std::env;
use std::error;
#[cfg(any(unix, windows))]
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::result;

#[cfg(unix)]
use crate::os::unix;
#[cfg(windows)]
use crate::os::windows;
use crate::{DirEntry, Error};

/// Skip the current test if the current environment doesn't support symlinks.
#[macro_export]
macro_rules! skip_if_no_symlinks {
    () => {
        if !$crate::tests::util::symlink_file_works() {
            eprintln!("skipping test because symlinks don't work");
            return;
        }
    };
}

/// Create an error from a format!-like syntax.
#[macro_export]
macro_rules! err {
    ($($tt:tt)*) => {
        Box::<dyn error::Error + Send + Sync>::from(format!($($tt)*))
    }
}

/// A convenient result type alias.
pub type Result<T> = result::Result<T, Box<dyn error::Error + Send + Sync>>;

/// The result of running a recursive directory iterator on a single directory.
#[derive(Debug)]
pub struct RecursiveResults {
    ents: Vec<DirEntry>,
    errs: Vec<Error>,
}

impl RecursiveResults {
    /// Return all of the errors encountered during traversal.
    pub fn errs(&self) -> &[Error] {
        &self.errs
    }

    /// Assert that no errors have occurred.
    pub fn assert_no_errors(&self) {
        assert!(
            self.errs.is_empty(),
            "expected to find no errors, but found: {:?}",
            self.errs
        );
    }

    /// Return all the successfully retrieved directory entries in the order
    /// in which they were retrieved.
    pub fn ents(&self) -> &[DirEntry] {
        &self.ents
    }

    /// Return all paths from all successfully retrieved directory entries.
    ///
    /// This does not include paths that correspond to an error.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.ents.iter().map(|d| d.path().to_path_buf()).collect()
    }

    /// Return all the successfully retrieved directory entries, sorted
    /// lexicographically by their full file path.
    pub fn sorted_ents(&self) -> Vec<DirEntry> {
        let mut ents = self.ents.clone();
        ents.sort_by(|e1, e2| e1.path().cmp(e2.path()));
        ents
    }

    /// Return all paths from all successfully retrieved directory entries,
    /// sorted lexicographically.
    ///
    /// This does not include paths that correspond to an error.
    pub fn sorted_paths(&self) -> Vec<PathBuf> {
        self.sorted_ents().into_iter().map(|d| d.into_path()).collect()
    }
}

/// The result of running a Unix directory iterator on a single directory.
#[cfg(unix)]
#[derive(Debug)]
pub struct UnixResults {
    ents: Vec<unix::DirEntry>,
    errs: Vec<io::Error>,
}

#[cfg(unix)]
impl UnixResults {
    /// Return all of the errors encountered during traversal.
    pub fn errs(&self) -> &[io::Error] {
        &self.errs
    }

    /// Assert that no errors have occurred.
    pub fn assert_no_errors(&self) {
        assert!(
            self.errs.is_empty(),
            "expected to find no errors, but found: {:?}",
            self.errs
        );
    }

    /// Return all the successfully retrieved directory entries in the order
    /// in which they were retrieved.
    pub fn ents(&self) -> &[unix::DirEntry] {
        &self.ents
    }

    /// Return all file names from all successfully retrieved directory
    /// entries.
    ///
    /// This does not include file names that correspond to an error.
    pub fn file_names(&self) -> Vec<OsString> {
        self.ents.iter().map(|d| d.file_name_os().to_os_string()).collect()
    }

    /// Return all the successfully retrieved directory entries, sorted
    /// lexicographically by their file name.
    pub fn sorted_ents(&self) -> Vec<unix::DirEntry> {
        let mut ents = self.ents.clone();
        ents.sort_by(|e1, e2| e1.file_name_bytes().cmp(e2.file_name_bytes()));
        ents
    }

    /// Return all file names from all successfully retrieved directory
    /// entries, sorted lexicographically.
    ///
    /// This does not include file names that correspond to an error.
    pub fn sorted_file_names(&self) -> Vec<OsString> {
        self.sorted_ents().into_iter().map(|d| d.into_file_name_os()).collect()
    }
}

/// The result of running a Windows directory iterator on a single directory.
#[cfg(windows)]
#[derive(Debug)]
pub struct WindowsResults {
    ents: Vec<windows::DirEntry>,
    errs: Vec<io::Error>,
}

#[cfg(windows)]
impl WindowsResults {
    /// Return all of the errors encountered during traversal.
    pub fn errs(&self) -> &[io::Error] {
        &self.errs
    }

    /// Assert that no errors have occurred.
    pub fn assert_no_errors(&self) {
        assert!(
            self.errs.is_empty(),
            "expected to find no errors, but found: {:?}",
            self.errs
        );
    }

    /// Return all the successfully retrieved directory entries in the order
    /// in which they were retrieved.
    pub fn ents(&self) -> &[windows::DirEntry] {
        &self.ents
    }

    /// Return all file names from all successfully retrieved directory
    /// entries.
    ///
    /// This does not include file names that correspond to an error.
    pub fn file_names(&self) -> Vec<OsString> {
        self.ents.iter().map(|d| d.file_name_os().to_os_string()).collect()
    }

    /// Return all the successfully retrieved directory entries, sorted
    /// lexicographically by their file name.
    pub fn sorted_ents(&self) -> Vec<windows::DirEntry> {
        let mut ents = self.ents.clone();
        ents.sort_by(|e1, e2| e1.file_name_u16().cmp(e2.file_name_u16()));
        ents
    }

    /// Return all file names from all successfully retrieved directory
    /// entries, sorted lexicographically.
    ///
    /// This does not include file names that correspond to an error.
    pub fn sorted_file_names(&self) -> Vec<OsString> {
        self.sorted_ents().into_iter().map(|d| d.into_file_name_os()).collect()
    }
}

/// A helper for managing a directory in which to run tests.
///
/// When manipulating paths within this directory, paths are interpreted
/// relative to this directory.
#[derive(Debug)]
pub struct Dir {
    dir: TempDir,
}

impl Dir {
    /// Create a new empty temporary directory.
    pub fn tmp() -> Dir {
        let dir = TempDir::new().unwrap();
        Dir { dir }
    }

    /// Return the path to this directory.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Return a path joined to the path to this directory.
    pub fn join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.path().join(path)
    }

    /// Run the given iterator and return the result as a distinct collection
    /// of directory entries and errors.
    pub fn run_recursive<I>(&self, it: I) -> RecursiveResults
    where
        I: IntoIterator<Item = result::Result<DirEntry, Error>>,
    {
        let mut results = RecursiveResults { ents: vec![], errs: vec![] };
        for result in it {
            match result {
                Ok(ent) => results.ents.push(ent),
                Err(err) => results.errs.push(err),
            }
        }
        results
    }

    #[cfg(unix)]
    pub fn run_unix(&self, udir: &mut unix::Dir) -> UnixResults {
        let mut results = UnixResults { ents: vec![], errs: vec![] };
        while let Some(result) = udir.read() {
            match result {
                Ok(ent) => results.ents.push(ent),
                Err(err) => results.errs.push(err),
            }
        }
        results
    }

    #[cfg(target_os = "linux")]
    pub fn run_linux(&self, dirfd: &mut unix::DirFd) -> UnixResults {
        use crate::os::linux::{getdents, DirEntryCursor};
        use std::os::unix::io::AsRawFd;

        let mut results = UnixResults { ents: vec![], errs: vec![] };
        let mut cursor = DirEntryCursor::new();
        loop {
            match getdents(dirfd.as_raw_fd(), &mut cursor) {
                Err(err) => {
                    results.errs.push(err);
                    break;
                }
                Ok(false) => {
                    break;
                }
                Ok(true) => {
                    while let Some(ent) = cursor.read_unix() {
                        results.ents.push(ent);
                    }
                }
            }
        }
        results
    }

    #[cfg(windows)]
    pub fn run_windows(&self, h: &mut windows::FindHandle) -> WindowsResults {
        let mut results = WindowsResults { ents: vec![], errs: vec![] };
        while let Some(result) = h.read() {
            match result {
                Ok(ent) => results.ents.push(ent),
                Err(err) => results.errs.push(err),
            }
        }
        results
    }

    /// Create a directory at the given path, while creating all intermediate
    /// directories as needed.
    pub fn mkdirp<P: AsRef<Path>>(&self, path: P) {
        let full = self.join(path);
        fs::create_dir_all(&full)
            .map_err(|e| {
                err!("failed to create directory {}: {}", full.display(), e)
            })
            .unwrap();
    }

    /// Create an empty file at the given path. All ancestor directories must
    /// already exists.
    pub fn touch<P: AsRef<Path>>(&self, path: P) {
        let full = self.join(path);
        File::create(&full)
            .map_err(|e| {
                err!("failed to create file {}: {}", full.display(), e)
            })
            .unwrap();
    }

    /// Create empty files at the given paths. All ancestor directories must
    /// already exists.
    pub fn touch_all<P: AsRef<Path>>(&self, paths: &[P]) {
        for p in paths {
            self.touch(p);
        }
    }

    /// Create a file symlink to the given src with the given link name.
    pub fn symlink_file<P1: AsRef<Path>, P2: AsRef<Path>>(
        &self,
        src: P1,
        link_name: P2,
    ) {
        symlink_file(self.join(src), self.join(link_name)).unwrap()
    }

    /// Create a directory symlink to the given src with the given link name.
    pub fn symlink_dir<P1: AsRef<Path>, P2: AsRef<Path>>(
        &self,
        src: P1,
        link_name: P2,
    ) {
        symlink_dir(self.join(src), self.join(link_name)).unwrap()
    }
}

/// A simple wrapper for creating a temporary directory that is automatically
/// deleted when it's dropped.
///
/// We use this in lieu of tempfile because tempfile brings in too many
/// dependencies.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

impl TempDir {
    /// Create a new empty temporary directory under the system's configured
    /// temporary directory.
    pub fn new() -> Result<TempDir> {
        #[allow(deprecated)]
        use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};

        static TRIES: usize = 100;
        #[allow(deprecated)]
        static COUNTER: AtomicUsize = ATOMIC_USIZE_INIT;

        let tmpdir = env::temp_dir();
        for _ in 0..TRIES {
            let count = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = tmpdir.join("rust-walkdir").join(count.to_string());
            if path.is_dir() {
                continue;
            }
            fs::create_dir_all(&path).map_err(|e| {
                err!("failed to create {}: {}", path.display(), e)
            })?;
            return Ok(TempDir(path));
        }
        Err(err!("failed to create temp dir after {} tries", TRIES))
    }

    /// Return the underlying path to this temporary directory.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Test whether file symlinks are believed to work on in this environment.
///
/// If they work, then return true, otherwise return false.
pub fn symlink_file_works() -> bool {
    use std::sync::atomic::{AtomicUsize, Ordering};

    // 0 = untried
    // 1 = works
    // 2 = does not work
    static WORKS: AtomicUsize = AtomicUsize::new(0);

    let status = WORKS.load(Ordering::SeqCst);
    if status != 0 {
        return status == 1;
    }

    let tmp = TempDir::new().unwrap();
    let foo = tmp.path().join("foo");
    let foolink = tmp.path().join("foo-link");
    File::create(&foo)
        .map_err(|e| {
            err!("error creating file {} for link test: {}", foo.display(), e)
        })
        .unwrap();
    if let Err(_) = symlink_file(&foo, &foolink) {
        WORKS.store(2, Ordering::SeqCst);
        return false;
    }
    if let Err(_) = fs::read(&foolink) {
        WORKS.store(2, Ordering::SeqCst);
        return false;
    }
    WORKS.store(1, Ordering::SeqCst);
    true
}

/// Create a file symlink to the given src with the given link name.
fn symlink_file<P1: AsRef<Path>, P2: AsRef<Path>>(
    src: P1,
    link_name: P2,
) -> Result<()> {
    #[cfg(windows)]
    fn imp(src: &Path, link_name: &Path) -> io::Result<()> {
        use std::os::windows::fs::symlink_file;
        symlink_file(src, link_name)
    }

    #[cfg(unix)]
    fn imp(src: &Path, link_name: &Path) -> io::Result<()> {
        use std::os::unix::fs::symlink;
        symlink(src, link_name)
    }

    imp(src.as_ref(), link_name.as_ref()).map_err(|e| {
        err!(
            "failed to symlink file {} with target {}: {}",
            src.as_ref().display(),
            link_name.as_ref().display(),
            e
        )
    })
}

/// Create a directory symlink to the given src with the given link name.
fn symlink_dir<P1: AsRef<Path>, P2: AsRef<Path>>(
    src: P1,
    link_name: P2,
) -> Result<()> {
    #[cfg(windows)]
    fn imp(src: &Path, link_name: &Path) -> io::Result<()> {
        use std::os::windows::fs::symlink_dir;
        symlink_dir(src, link_name)
    }

    #[cfg(unix)]
    fn imp(src: &Path, link_name: &Path) -> io::Result<()> {
        use std::os::unix::fs::symlink;
        symlink(src, link_name)
    }

    imp(src.as_ref(), link_name.as_ref()).map_err(|e| {
        err!(
            "failed to symlink directory {} with target {}: {}",
            src.as_ref().display(),
            link_name.as_ref().display(),
            e
        )
    })
}
