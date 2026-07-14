use std::cmp::{self, min};
#[cfg(walkdir_unix)]
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::result;

use same_file::Handle;

use crate::dent::DirEntry;
use crate::error::{Error, Result};
use crate::util;

/// Like try, but for iterators that return [`Option<Result<_, _>>`].
///
/// [`Option<Result<_, _>>`]: https://doc.rust-lang.org/stable/std/option/enum.Option.html
macro_rules! itry {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(err) => return Some(Err(From::from(err))),
        }
    };
}

struct WalkDirOptions {
    follow_links: bool,
    follow_root_links: bool,
    max_open: usize,
    min_depth: usize,
    max_depth: usize,
    sorter: Option<
        Box<
            dyn FnMut(&DirEntry, &DirEntry) -> cmp::Ordering
                + Send
                + Sync
                + 'static,
        >,
    >,
    contents_first: bool,
    same_file_system: bool,
}

impl fmt::Debug for WalkDirOptions {
    fn fmt(&self, f: &mut fmt::Formatter) -> result::Result<(), fmt::Error> {
        let sorter_str = if self.sorter.is_some() {
            // FnMut isn't `Debug`
            "Some(...)"
        } else {
            "None"
        };
        f.debug_struct("WalkDirOptions")
            .field("follow_links", &self.follow_links)
            .field("follow_root_links", &self.follow_root_links)
            .field("max_open", &self.max_open)
            .field("min_depth", &self.min_depth)
            .field("max_depth", &self.max_depth)
            .field("sorter", &sorter_str)
            .field("contents_first", &self.contents_first)
            .field("same_file_system", &self.same_file_system)
            .finish()
    }
}

/// A builder to create an iterator for recursively walking a directory.
///
/// Results are returned in depth first fashion, with directories yielded
/// before their contents. If [`contents_first`] is true, contents are yielded
/// before their directories. The order is unspecified but if [`sort_by`] is
/// given, directory entries are sorted according to this function. Directory
/// entries `.` and `..` are always omitted.
///
/// If an error occurs at any point during iteration, then it is returned in
/// place of its corresponding directory entry and iteration continues as
/// normal. If an error occurs while opening a directory for reading, then it
/// is not descended into (but the error is still yielded by the iterator).
/// Iteration may be stopped at any time. When the iterator is destroyed, all
/// resources associated with it are freed.
///
/// [`contents_first`]: struct.WalkDir.html#method.contents_first
/// [`sort_by`]: struct.WalkDir.html#method.sort_by
///
/// # Usage
///
/// This type implements [`IntoIterator`] so that it may be used as the subject
/// of a `for` loop. You may need to call [`into_iter`] explicitly if you want
/// to use iterator adapters such as [`filter_entry`].
///
/// Idiomatic use of this type should use method chaining to set desired
/// options. For example, this only shows entries with a depth of `1`, `2` or
/// `3` (relative to `foo`):
///
/// ```no_run
/// use walkdir::WalkDir;
/// # use walkdir::Error;
///
/// # fn try_main() -> Result<(), Error> {
/// for entry in WalkDir::new("foo").min_depth(1).max_depth(3) {
///     println!("{}", entry?.path().display());
/// }
/// # Ok(())
/// # }
/// ```
///
/// [`IntoIterator`]: https://doc.rust-lang.org/stable/std/iter/trait.IntoIterator.html
/// [`into_iter`]: https://doc.rust-lang.org/nightly/core/iter/trait.IntoIterator.html#tymethod.into_iter
/// [`filter_entry`]: struct.IntoIter.html#method.filter_entry
///
/// Note that the iterator by default includes the top-most directory. Since
/// this is the only directory yielded with depth `0`, it is easy to ignore it
/// with the [`min_depth`] setting:
///
/// ```no_run
/// use walkdir::WalkDir;
/// # use walkdir::Error;
///
/// # fn try_main() -> Result<(), Error> {
/// for entry in WalkDir::new("foo").min_depth(1) {
///     println!("{}", entry?.path().display());
/// }
/// # Ok(())
/// # }
/// ```
///
/// [`min_depth`]: struct.WalkDir.html#method.min_depth
///
/// This will only return descendents of the `foo` directory and not `foo`
/// itself.
///
/// # Loops
///
/// This iterator (like most/all recursive directory iterators) assumes that
/// no loops can be made with *hard* links on your file system. In particular,
/// this would require creating a hard link to a directory such that it creates
/// a loop. On most platforms, this operation is illegal.
///
/// Note that when following symbolic/soft links, loops are detected and an
/// error is reported.
#[derive(Debug)]
pub struct WalkDir {
    root: PathBuf,
    opts: WalkDirOptions,
}

impl IntoIterator for WalkDir {
    type Item = Result<DirEntry>;
    type IntoIter = IntoIter;

    fn into_iter(self) -> IntoIter {
        IntoIter {
            opts: self.opts,
            start: Some(self.root),
            stack_list: vec![],
            stack_path: vec![],
            oldest_opened: 0,
            depth: 0,
            deferred_dirs: vec![],
            root_device: None,
        }
    }
}

impl WalkDir {
    /// Create a builder for a recursive directory iterator starting at the
    /// file path `root`. If `root` is a directory, then it is the first item
    /// yielded by the iterator. If `root` is a file, then it is the first
    /// and only item yielded by the iterator. If `root` is a symlink, then it
    /// is followed for the purposes of directory traversal unless
    /// [`follow_root_links`] is disabled. (A root `DirEntry` still obeys its
    /// documentation with respect to symlinks and the `follow_links` setting.)
    ///
    /// [`follow_root_links`]: struct.WalkDir.html#method.follow_root_links
    pub fn new<P: Into<PathBuf>>(root: P) -> WalkDir {
        WalkDir {
            root: root.into(),
            opts: WalkDirOptions {
                follow_links: false,
                follow_root_links: true,
                max_open: 10,
                min_depth: 0,
                max_depth: usize::MAX,
                sorter: None,
                contents_first: false,
                same_file_system: false,
            },
        }
    }

    /// Set the minimum depth of entries yielded by the iterator.
    ///
    /// The smallest depth is `0` and always corresponds to the path given
    /// to the `new` function on this type. Its direct descendents have depth
    /// `1`, and their descendents have depth `2`, and so on.
    pub fn min_depth(mut self, depth: usize) -> WalkDir {
        self.opts.min_depth = depth;
        if self.opts.min_depth > self.opts.max_depth {
            self.opts.min_depth = self.opts.max_depth;
        }
        self
    }

    /// Set the maximum depth of entries yield by the iterator.
    ///
    /// The smallest depth is `0` and always corresponds to the path given
    /// to the `new` function on this type. Its direct descendents have depth
    /// `1`, and their descendents have depth `2`, and so on.
    ///
    /// Note that this will not simply filter the entries of the iterator, but
    /// it will actually avoid descending into directories when the depth is
    /// exceeded.
    pub fn max_depth(mut self, depth: usize) -> WalkDir {
        self.opts.max_depth = depth;
        if self.opts.max_depth < self.opts.min_depth {
            self.opts.max_depth = self.opts.min_depth;
        }
        self
    }

    /// Follow symbolic links. By default, this is disabled.
    ///
    /// When `yes` is `true`, symbolic links are followed as if they were
    /// normal directories and files. If a symbolic link is broken or is
    /// involved in a loop, an error is yielded.
    ///
    /// When enabled, the yielded [`DirEntry`] values represent the target of
    /// the link while the path corresponds to the link. See the [`DirEntry`]
    /// type for more details.
    ///
    /// [`DirEntry`]: struct.DirEntry.html
    pub fn follow_links(mut self, yes: bool) -> WalkDir {
        self.opts.follow_links = yes;
        self
    }

    /// Follow symbolic links if these are the root of the traversal.
    /// By default, this is enabled.
    ///
    /// When `yes` is `true`, symbolic links on root paths are followed
    /// which is effective if the symbolic link points to a directory.
    /// If a symbolic link is broken or is involved in a loop, an error is
    /// yielded as the first entry of the traversal.
    ///
    /// When enabled, the yielded [`DirEntry`] values represent the target of
    /// the link while the path corresponds to the link. See the [`DirEntry`]
    /// type for more details, and all future entries will be contained within
    /// the resolved directory behind the symbolic link of the root path.
    ///
    /// [`DirEntry`]: struct.DirEntry.html
    pub fn follow_root_links(mut self, yes: bool) -> WalkDir {
        self.opts.follow_root_links = yes;
        self
    }

    /// Set the maximum number of simultaneously open file descriptors used
    /// by the iterator.
    ///
    /// `n` must be greater than or equal to `1`. If `n` is `0`, then it is set
    /// to `1` automatically. If this is not set, then it defaults to some
    /// reasonably low number.
    ///
    /// This setting has no impact on the results yielded by the iterator
    /// (even when `n` is `1`). Instead, this setting represents a trade off
    /// between scarce resources (file descriptors) and memory. Namely, when
    /// the maximum number of file descriptors is reached and a new directory
    /// needs to be opened to continue iteration, then a previous directory
    /// handle is closed and has its unyielded entries stored in memory. In
    /// practice, this is a satisfying trade off because it scales with respect
    /// to the *depth* of your file tree. Therefore, low values (even `1`) are
    /// acceptable.
    ///
    /// Note that this value does not impact the number of system calls made by
    /// an exhausted iterator.
    ///
    /// # Platform behavior
    ///
    /// On Windows, if `follow_links` is enabled, then this limit is not
    /// respected. In particular, the maximum number of file descriptors opened
    /// is proportional to the depth of the directory tree traversed.
    pub fn max_open(mut self, mut n: usize) -> WalkDir {
        if n == 0 {
            n = 1;
        }
        self.opts.max_open = n;
        self
    }

    /// Set a function for sorting directory entries.
    ///
    /// If a compare function is set, the resulting iterator will return all
    /// paths in sorted order. The compare function will be called to compare
    /// entries from the same directory.
    ///
    /// ```rust,no_run
    /// use std::cmp;
    /// use std::ffi::OsString;
    /// use walkdir::WalkDir;
    ///
    /// WalkDir::new("foo").sort_by(|a,b| a.file_name().cmp(b.file_name()));
    /// ```
    pub fn sort_by<F>(mut self, cmp: F) -> WalkDir
    where
        F: FnMut(&DirEntry, &DirEntry) -> cmp::Ordering
            + Send
            + Sync
            + 'static,
    {
        self.opts.sorter = Some(Box::new(cmp));
        self
    }

    /// Set a function for sorting directory entries with a key extraction
    /// function.
    ///
    /// If a compare function is set, the resulting iterator will return all
    /// paths in sorted order. The compare function will be called to compare
    /// entries from the same directory.
    ///
    /// ```rust,no_run
    /// use std::cmp;
    /// use std::ffi::OsString;
    /// use walkdir::WalkDir;
    ///
    /// WalkDir::new("foo").sort_by_key(|a| a.file_name().to_owned());
    /// ```
    pub fn sort_by_key<K, F>(self, mut cmp: F) -> WalkDir
    where
        F: FnMut(&DirEntry) -> K + Send + Sync + 'static,
        K: Ord,
    {
        self.sort_by(move |a, b| cmp(a).cmp(&cmp(b)))
    }

    /// Sort directory entries by file name, to ensure a deterministic order.
    ///
    /// This is a convenience function for calling [`Self::sort_by()`].
    ///
    /// ```rust,no_run
    /// use walkdir::WalkDir;
    ///
    /// WalkDir::new("foo").sort_by_file_name();
    /// ```
    pub fn sort_by_file_name(self) -> WalkDir {
        self.sort_by(|a, b| a.file_name().cmp(b.file_name()))
    }

    /// Yield a directory's contents before the directory itself. By default,
    /// this is disabled.
    ///
    /// When `yes` is `false` (as is the default), the directory is yielded
    /// before its contents are read. This is useful when, e.g. you want to
    /// skip processing of some directories.
    ///
    /// When `yes` is `true`, the iterator yields the contents of a directory
    /// before yielding the directory itself. This is useful when, e.g. you
    /// want to recursively delete a directory.
    ///
    /// # Example
    ///
    /// Assume the following directory tree:
    ///
    /// ```text
    /// foo/
    ///   abc/
    ///     qrs
    ///     tuv
    ///   def/
    /// ```
    ///
    /// With contents_first disabled (the default), the following code visits
    /// the directory tree in depth-first order:
    ///
    /// ```no_run
    /// use walkdir::WalkDir;
    ///
    /// for entry in WalkDir::new("foo") {
    ///     let entry = entry.unwrap();
    ///     println!("{}", entry.path().display());
    /// }
    ///
    /// // foo
    /// // foo/abc
    /// // foo/abc/qrs
    /// // foo/abc/tuv
    /// // foo/def
    /// ```
    ///
    /// With contents_first enabled:
    ///
    /// ```no_run
    /// use walkdir::WalkDir;
    ///
    /// for entry in WalkDir::new("foo").contents_first(true) {
    ///     let entry = entry.unwrap();
    ///     println!("{}", entry.path().display());
    /// }
    ///
    /// // foo/abc/qrs
    /// // foo/abc/tuv
    /// // foo/abc
    /// // foo/def
    /// // foo
    /// ```
    pub fn contents_first(mut self, yes: bool) -> WalkDir {
        self.opts.contents_first = yes;
        self
    }

    /// Do not cross file system boundaries.
    ///
    /// When this option is enabled, directory traversal will not descend into
    /// directories that are on a different file system from the root path.
    ///
    /// Currently, this option is only supported on Unix and Windows. If this
    /// option is used on an unsupported platform, then directory traversal
    /// will immediately return an error and will not yield any entries.
    pub fn same_file_system(mut self, yes: bool) -> WalkDir {
        self.opts.same_file_system = yes;
        self
    }
}

/// An iterator for recursively descending into a directory.
///
/// A value with this type must be constructed with the [`WalkDir`] type, which
/// uses a builder pattern to set options such as min/max depth, max open file
/// descriptors and whether the iterator should follow symbolic links. After
/// constructing a `WalkDir`, call [`.into_iter()`] at the end of the chain.
///
/// The order of elements yielded by this iterator is unspecified.
///
/// [`WalkDir`]: struct.WalkDir.html
/// [`.into_iter()`]: struct.WalkDir.html#into_iter.v
#[derive(Debug)]
pub struct IntoIter {
    opts: WalkDirOptions,
    /// The start path.
    ///
    /// This is only [`Some(...)`](`Some`) at the beginning. After the first iteration,
    /// this is always [`None`].
    start: Option<PathBuf>,
    /// A stack of open (up to max fd) or spilled handles to directories.
    stack_list: Vec<crate::dir::DirList>,
    /// A stack of file paths.
    ///
    /// This is *only* used when `follow_links` is enabled. In all other cases
    /// this stack is empty.
    stack_path: Vec<Ancestor>,
    /// An index into `stack_list` that points to the oldest open directory
    /// handle. If the maximum fd limit is reached and a new directory needs to
    /// be read, the handle at this index is spilled before the new directory is
    /// opened.
    oldest_opened: usize,
    /// The current depth of iteration.
    depth: usize,
    /// A list of [`DirEntry`]s corresponding to directories, that are yielded
    /// after their contents has been fully yielded. This is only used when
    /// `contents_first` is enabled.
    deferred_dirs: Vec<DirEntry>,
    /// The device of the root file path when the first call to `next` was made.
    ///
    /// If the `same_file_system` option isn't enabled, then this is always
    /// [`None`]. Conversely, if it is enabled, this is always [`Some(...)`](`Some`)
    /// after handling the root path.
    root_device: Option<u64>,
}

/// An ancestor is an item in the directory tree traversed by walkdir, and is
/// used to check for loops in the tree when traversing symlinks.
#[derive(Debug)]
struct Ancestor {
    /// The path of this ancestor.
    path: PathBuf,
    /// An open file to this ancesor. This is only used on Windows where
    /// opening a file handle appears to be quite expensive, so we choose to
    /// cache it. This comes at the cost of not respecting the file descriptor
    /// limit set by the user.
    #[cfg(windows)]
    handle: Handle,
}

impl Ancestor {
    /// Create a new ancestor from the given directory path.
    #[cfg(windows)]
    fn new(dent: &DirEntry) -> std::io::Result<Ancestor> {
        let handle = Handle::from_path(dent.path())?;
        Ok(Ancestor { path: dent.path().to_path_buf(), handle })
    }

    /// Create a new ancestor from the given directory path.
    #[cfg(not(windows))]
    fn new(dent: &DirEntry) -> std::io::Result<Ancestor> {
        Ok(Ancestor { path: dent.path().to_path_buf() })
    }

    /// Returns true if and only if the given open file handle corresponds to
    /// the same directory as this ancestor.
    #[cfg(windows)]
    fn is_same(&self, child: &Handle) -> std::io::Result<bool> {
        Ok(child == &self.handle)
    }

    /// Returns true if and only if the given open file handle corresponds to
    /// the same directory as this ancestor.
    #[cfg(not(windows))]
    fn is_same(&self, child: &Handle) -> std::io::Result<bool> {
        Ok(child == &Handle::from_path(&self.path)?)
    }
}

impl Iterator for IntoIter {
    type Item = Result<DirEntry>;
    /// Advances the iterator and returns the next value.
    ///
    /// # Errors
    ///
    /// If the iterator fails to retrieve the next value, this method returns
    /// an error value. The error will be wrapped in an [`Option::Some`].
    fn next(&mut self) -> Option<Result<DirEntry>> {
        if let Some(start) = self.start.take() {
            if self.opts.same_file_system {
                let result = util::device_num(&start)
                    .map_err(|e| Error::from_path(0, start.clone(), e));
                self.root_device = Some(itry!(result));
            }
            let dent = itry!(DirEntry::from_path(0, start, false));
            if let Some(result) = self.handle_entry(dent) {
                return Some(result);
            }
        }
        while !self.stack_list.is_empty() {
            self.depth = self.stack_list.len();
            if let Some(dentry) = self.get_deferred_dir() {
                return Some(Ok(dentry));
            }
            if self.depth > self.opts.max_depth {
                // If we've exceeded the max depth, pop the current dir
                // so that we don't descend.
                self.pop();
                continue;
            }
            match self.next_entry() {
                None => self.pop(),
                Some(Err(err)) => return Some(Err(err)),
                Some(Ok(dent)) => {
                    if let Some(result) = self.handle_entry(dent) {
                        return Some(result);
                    }
                }
            }
        }
        if self.opts.contents_first {
            self.depth = self.stack_list.len();
            if let Some(dentry) = self.get_deferred_dir() {
                return Some(Ok(dentry));
            }
        }
        None
    }
}

impl IntoIter {
    /// Skips the current directory.
    ///
    /// This causes the iterator to stop traversing the contents of the least
    /// recently yielded directory. This means any remaining entries in that
    /// directory will be skipped (including sub-directories).
    ///
    /// Note that the ergonomics of this method are questionable since it
    /// borrows the iterator mutably. Namely, you must write out the looping
    /// condition manually. For example, to skip hidden entries efficiently on
    /// unix systems:
    ///
    /// ```no_run
    /// use walkdir::{DirEntry, WalkDir};
    ///
    /// fn is_hidden(entry: &DirEntry) -> bool {
    ///     entry.file_name()
    ///          .to_str()
    ///          .map(|s| s.starts_with("."))
    ///          .unwrap_or(false)
    /// }
    ///
    /// let mut it = WalkDir::new("foo").into_iter();
    /// loop {
    ///     let entry = match it.next() {
    ///         None => break,
    ///         Some(Err(err)) => panic!("ERROR: {}", err),
    ///         Some(Ok(entry)) => entry,
    ///     };
    ///     if is_hidden(&entry) {
    ///         if entry.file_type().is_dir() {
    ///             it.skip_current_dir();
    ///         }
    ///         continue;
    ///     }
    ///     println!("{}", entry.path().display());
    /// }
    /// ```
    ///
    /// You may find it more convenient to use the [`filter_entry`] iterator
    /// adapter. (See its documentation for the same example functionality as
    /// above.)
    ///
    /// [`filter_entry`]: #method.filter_entry
    pub fn skip_current_dir(&mut self) {
        if !self.stack_list.is_empty() {
            self.pop();
        }
    }

    /// Yields only entries which satisfy the given predicate and skips
    /// descending into directories that do not satisfy the given predicate.
    ///
    /// The predicate is applied to all entries. If the predicate is
    /// true, iteration carries on as normal. If the predicate is false, the
    /// entry is ignored and if it is a directory, it is not descended into.
    ///
    /// This is often more convenient to use than [`skip_current_dir`]. For
    /// example, to skip hidden files and directories efficiently on unix
    /// systems:
    ///
    /// ```no_run
    /// use walkdir::{DirEntry, WalkDir};
    /// # use walkdir::Error;
    ///
    /// fn is_hidden(entry: &DirEntry) -> bool {
    ///     entry.file_name()
    ///          .to_str()
    ///          .map(|s| s.starts_with("."))
    ///          .unwrap_or(false)
    /// }
    ///
    /// # fn try_main() -> Result<(), Error> {
    /// for entry in WalkDir::new("foo")
    ///                      .into_iter()
    ///                      .filter_entry(|e| !is_hidden(e)) {
    ///     println!("{}", entry?.path().display());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Note that the iterator will still yield errors for reading entries that
    /// may not satisfy the predicate.
    ///
    /// Note that entries skipped with [`min_depth`] and [`max_depth`] are not
    /// passed to this predicate.
    ///
    /// Note that if the iterator has `contents_first` enabled, then this
    /// method is no different than calling the standard `Iterator::filter`
    /// method (because directory entries are yielded after they've been
    /// descended into).
    ///
    /// [`skip_current_dir`]: #method.skip_current_dir
    /// [`min_depth`]: struct.WalkDir.html#method.min_depth
    /// [`max_depth`]: struct.WalkDir.html#method.max_depth
    pub fn filter_entry<P>(self, predicate: P) -> FilterEntry<Self, P>
    where
        P: FnMut(&DirEntry) -> bool,
    {
        FilterEntry { it: self, predicate }
    }

    fn handle_entry(
        &mut self,
        mut dent: DirEntry,
    ) -> Option<Result<DirEntry>> {
        if self.opts.follow_links && dent.file_type().is_symlink() {
            dent = itry!(self.follow(dent));
        }
        let is_normal_dir = !dent.file_type().is_symlink() && dent.is_dir();
        if is_normal_dir {
            if self.opts.same_file_system && dent.depth() > 0 {
                if itry!(self.is_same_file_system(&dent)) {
                    itry!(self.push(&dent));
                }
            } else {
                itry!(self.push(&dent));
            }
        } else if dent.depth() == 0
            && dent.file_type().is_symlink()
            && self.opts.follow_root_links
        {
            // As a special case, if we are processing a root entry, then we
            // always follow it even if it's a symlink and follow_links is
            // false. We are careful to not let this change the semantics of
            // the DirEntry however. Namely, the DirEntry should still respect
            // the follow_links setting. When it's disabled, it should report
            // itself as a symlink. When it's enabled, it should always report
            // itself as the target.
            let md = itry!(fs::metadata(dent.path()).map_err(|err| {
                Error::from_path(dent.depth(), dent.path().to_path_buf(), err)
            }));
            if md.file_type().is_dir() {
                itry!(self.push(&dent));
            }
        }
        if is_normal_dir && self.opts.contents_first {
            self.deferred_dirs.push(dent);
            None
        } else if self.skippable() {
            None
        } else {
            Some(Ok(dent))
        }
    }

    fn get_deferred_dir(&mut self) -> Option<DirEntry> {
        if self.opts.contents_first && self.depth < self.deferred_dirs.len() {
            // Unwrap is safe here because we've guaranteed that
            // `self.deferred_dirs.len()` can never be less than 1
            let deferred: DirEntry = self
                .deferred_dirs
                .pop()
                .expect("BUG: deferred_dirs should be non-empty");
            if !self.skippable() {
                return Some(deferred);
            }
        }
        None
    }

    fn push(&mut self, dent: &DirEntry) -> Result<()> {
        // Spill the shallowest handle so the immediate parent stays open.
        let free =
            self.stack_list.len().checked_sub(self.oldest_opened).unwrap();
        if free == self.opts.max_open {
            self.stack_list[self.oldest_opened].spill();
            self.oldest_opened = self.oldest_opened.checked_add(1).unwrap();
        }

        #[cfg(walkdir_unix)]
        let mut list = {
            let follow = self.opts.follow_links
                || (dent.depth() == 0 && self.opts.follow_root_links);
            match self.stack_list.last().and_then(|list| list.parent_handle())
            {
                Some(parent) => crate::dir::DirList::openat(
                    self.depth,
                    parent,
                    &cstr_of_file_name(dent.path()),
                    follow,
                    dent.path().to_path_buf(),
                ),
                None => crate::dir::DirList::open_path(
                    self.depth,
                    dent.path().to_path_buf(),
                    &cstr_of_path(dent.path()),
                    follow,
                ),
            }
        };
        #[cfg(windows)]
        let mut list = {
            let follow = self.opts.follow_links
                || (dent.depth() == 0 && self.opts.follow_root_links);
            crate::dir::DirList::open_path(
                self.depth,
                dent.path().to_path_buf(),
                follow,
            )
        };
        #[cfg(not(any(walkdir_unix, windows)))]
        let mut list = crate::dir::DirList::open_path(
            self.depth,
            dent.path().to_path_buf(),
        );

        // Sorters need a materialized directory.
        if self.opts.sorter.is_some() {
            // Failed opens keep the directory path.
            let open_failure = list.is_failed();
            let mut ents: Vec<Result<DirEntry>> = Vec::new();
            while let Some(res) = list.next() {
                match res {
                    Ok(()) => {
                        if let Some(entry) =
                            entry_from_list(self.depth + 1, &list)
                        {
                            ents.push(entry);
                        }
                    }
                    Err(err) => ents.push(Err(dir_read_error(
                        open_failure,
                        self.depth,
                        list.path(),
                        self.depth + 1,
                        err,
                    ))),
                }
            }
            if let Some(ref mut cmp) = self.opts.sorter {
                ents.sort_by(|a, b| match (a, b) {
                    (Ok(a), Ok(b)) => cmp(a, b),
                    (&Err(_), &Err(_)) => cmp::Ordering::Equal,
                    (&Ok(_), &Err(_)) => cmp::Ordering::Greater,
                    (&Err(_), &Ok(_)) => cmp::Ordering::Less,
                });
            }
            list = list.into_built(ents);
        }

        // Push after stack_path so both stacks stay paired on failure.
        if self.opts.follow_links {
            let ancestor = Ancestor::new(dent)
                .map_err(|err| Error::from_io(self.depth, err))?;
            self.stack_path.push(ancestor);
        }
        self.stack_list.push(list);
        Ok(())
    }

    fn pop(&mut self) {
        self.stack_list.pop().expect("BUG: cannot pop from empty stack");
        if self.opts.follow_links {
            self.stack_path.pop().expect("BUG: list/path stacks out of sync");
        }
        // If everything in the stack is already closed, then there is
        // room for at least one more open descriptor and it will
        // always be at the top of the stack.
        self.oldest_opened = min(self.oldest_opened, self.stack_list.len());
    }

    /// Pull the next entry from the active directory.
    fn next_entry(&mut self) -> Option<Result<DirEntry>> {
        loop {
            let list = self.stack_list.last_mut()?;
            if list.is_built() {
                return list.next_built();
            }
            // Failed opens keep the directory path and depth.
            let open_failure = list.is_failed();
            let list_depth = list.depth();
            match list.next() {
                None => return None,
                Some(Err(err)) => {
                    return Some(Err(dir_read_error(
                        open_failure,
                        list_depth,
                        list.path(),
                        self.depth,
                        err,
                    )))
                }
                Some(Ok(())) => {
                    if let Some(entry) = entry_from_list(self.depth, list) {
                        return Some(entry);
                    }
                }
            }
        }
    }

    fn follow(&self, mut dent: DirEntry) -> Result<DirEntry> {
        dent =
            DirEntry::from_path(self.depth, dent.path().to_path_buf(), true)?;
        // The only way a symlink can cause a loop is if it points
        // to a directory. Otherwise, it always points to a leaf
        // and we can omit any loop checks.
        if dent.is_dir() {
            self.check_loop(dent.path())?;
        }
        Ok(dent)
    }

    fn check_loop<P: AsRef<Path>>(&self, child: P) -> Result<()> {
        let hchild = Handle::from_path(&child)
            .map_err(|err| Error::from_io(self.depth, err))?;
        for ancestor in self.stack_path.iter().rev() {
            let is_same = ancestor
                .is_same(&hchild)
                .map_err(|err| Error::from_io(self.depth, err))?;
            if is_same {
                return Err(Error::from_loop(
                    self.depth,
                    &ancestor.path,
                    child.as_ref(),
                ));
            }
        }
        Ok(())
    }

    #[cfg(walkdir_unix)]
    fn is_same_file_system(&mut self, dent: &DirEntry) -> Result<bool> {
        let parent =
            self.stack_list.last().and_then(|list| list.parent_handle());
        let dent_device = match parent {
            Some(fd) => {
                crate::os::unix::statat_c(fd, &cstr_of_file_name(dent.path()))
                    .map(|metadata| metadata.dev())
            }
            None => util::device_num(dent.path()),
        }
        .map_err(|err| Error::from_entry(dent, err))?;
        Ok(self
            .root_device
            .map(|device| device == dent_device)
            .expect("BUG: called is_same_file_system without root device"))
    }

    #[cfg(not(walkdir_unix))]
    fn is_same_file_system(&mut self, dent: &DirEntry) -> Result<bool> {
        let dent_device = util::device_num(dent.path())
            .map_err(|err| Error::from_entry(dent, err))?;
        Ok(self
            .root_device
            .map(|d| d == dent_device)
            .expect("BUG: called is_same_file_system without root device"))
    }

    fn skippable(&self) -> bool {
        self.depth < self.opts.min_depth || self.depth > self.opts.max_depth
    }
}

impl std::iter::FusedIterator for IntoIter {}

#[cfg(walkdir_unix)]
fn entry_from_list(
    depth: usize,
    list: &crate::dir::DirList,
) -> Option<Result<DirEntry>> {
    let entry = list.entry();
    if entry.file_name_bytes() == b"." || entry.file_name_bytes() == b".." {
        return None;
    }
    Some(DirEntry::from_os_entry(
        depth,
        list.path(),
        list.parent_handle(),
        entry,
    ))
}

#[cfg(windows)]
fn entry_from_list(
    depth: usize,
    list: &crate::dir::DirList,
) -> Option<Result<DirEntry>> {
    let entry = list.entry();
    if entry.is_dots() {
        return None;
    }
    Some(DirEntry::from_os_entry(depth, list.path(), entry))
}

#[cfg(not(any(walkdir_unix, windows)))]
fn entry_from_list(
    depth: usize,
    list: &crate::dir::DirList,
) -> Option<Result<DirEntry>> {
    Some(DirEntry::from_entry(depth, list.entry()))
}

/// Failed opens keep the directory's own path and depth, other errors use
/// the entry depth.
fn dir_read_error(
    open_failure: bool,
    dir_depth: usize,
    dir_path: &Path,
    entry_depth: usize,
    err: std::io::Error,
) -> Error {
    if open_failure {
        Error::from_path(dir_depth, dir_path.to_path_buf(), err)
    } else {
        Error::from_io(entry_depth, err)
    }
}

#[cfg(walkdir_unix)]
fn cstr_of_path(path: &Path) -> CString {
    use std::os::unix::ffi::OsStrExt;

    CString::new(path.as_os_str().as_bytes())
        .expect("path has no interior NUL")
}

#[cfg(walkdir_unix)]
fn cstr_of_file_name(path: &Path) -> CString {
    use std::os::unix::ffi::OsStrExt;

    let name = path.file_name().unwrap_or(path.as_os_str());
    CString::new(name.as_bytes()).expect("name has no interior NUL")
}

/// A recursive directory iterator that skips entries.
///
/// Values of this type are created by calling [`.filter_entry()`] on an
/// [`IntoIter`], which is formed by calling [`.into_iter()`] on a [`WalkDir`].
///
/// Directories that fail the predicate `P` are skipped. Namely, they are
/// never yielded and never descended into.
///
/// Entries that are skipped with the [`min_depth`] and [`max_depth`] options
/// are not passed through this filter.
///
/// If opening a handle to a directory resulted in an error, then it is yielded
/// and no corresponding call to the predicate is made.
///
/// Type parameter `I` refers to the underlying iterator and `P` refers to the
/// predicate, which is usually `FnMut(&DirEntry) -> bool`.
///
/// [`.filter_entry()`]: struct.IntoIter.html#method.filter_entry
/// [`.into_iter()`]: struct.WalkDir.html#into_iter.v
/// [`min_depth`]: struct.WalkDir.html#method.min_depth
/// [`max_depth`]: struct.WalkDir.html#method.max_depth
#[derive(Debug)]
pub struct FilterEntry<I, P> {
    it: I,
    predicate: P,
}

impl<P> std::iter::FusedIterator for FilterEntry<IntoIter, P> where
    P: FnMut(&DirEntry) -> bool
{
}

impl<P> Iterator for FilterEntry<IntoIter, P>
where
    P: FnMut(&DirEntry) -> bool,
{
    type Item = Result<DirEntry>;

    /// Advances the iterator and returns the next value.
    ///
    /// # Errors
    ///
    /// If the iterator fails to retrieve the next value, this method returns
    /// an error value. The error will be wrapped in an [`Option::Some`].
    fn next(&mut self) -> Option<Result<DirEntry>> {
        loop {
            let dent = itry!(self.it.next()?);

            if !(self.predicate)(&dent) {
                if dent.is_dir() {
                    self.it.skip_current_dir();
                }
                continue;
            }

            return Some(Ok(dent));
        }
    }
}

impl<P> FilterEntry<IntoIter, P>
where
    P: FnMut(&DirEntry) -> bool,
{
    /// Yields only entries which satisfy the given predicate and skips
    /// descending into directories that do not satisfy the given predicate.
    ///
    /// The predicate is applied to all entries. If the predicate is
    /// true, iteration carries on as normal. If the predicate is false, the
    /// entry is ignored and if it is a directory, it is not descended into.
    ///
    /// This is often more convenient to use than [`skip_current_dir`]. For
    /// example, to skip hidden files and directories efficiently on unix
    /// systems:
    ///
    /// ```no_run
    /// use walkdir::{DirEntry, WalkDir};
    /// # use walkdir::Error;
    ///
    /// fn is_hidden(entry: &DirEntry) -> bool {
    ///     entry.file_name()
    ///          .to_str()
    ///          .map(|s| s.starts_with("."))
    ///          .unwrap_or(false)
    /// }
    ///
    /// # fn try_main() -> Result<(), Error> {
    /// for entry in WalkDir::new("foo")
    ///                      .into_iter()
    ///                      .filter_entry(|e| !is_hidden(e)) {
    ///     println!("{}", entry?.path().display());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Note that the iterator will still yield errors for reading entries that
    /// may not satisfy the predicate.
    ///
    /// Note that entries skipped with [`min_depth`] and [`max_depth`] are not
    /// passed to this predicate.
    ///
    /// Note that if the iterator has `contents_first` enabled, then this
    /// method is no different than calling the standard [`Iterator::filter`]
    /// method (because directory entries are yielded after they've been
    /// descended into).
    ///
    /// [`skip_current_dir`]: #method.skip_current_dir
    /// [`min_depth`]: struct.WalkDir.html#method.min_depth
    /// [`max_depth`]: struct.WalkDir.html#method.max_depth
    pub fn filter_entry(self, predicate: P) -> FilterEntry<Self, P> {
        FilterEntry { it: self, predicate }
    }

    /// Skips the current directory.
    ///
    /// This causes the iterator to stop traversing the contents of the least
    /// recently yielded directory. This means any remaining entries in that
    /// directory will be skipped (including sub-directories).
    ///
    /// Note that the ergonomics of this method are questionable since it
    /// borrows the iterator mutably. Namely, you must write out the looping
    /// condition manually. For example, to skip hidden entries efficiently on
    /// unix systems:
    ///
    /// ```no_run
    /// use walkdir::{DirEntry, WalkDir};
    ///
    /// fn is_hidden(entry: &DirEntry) -> bool {
    ///     entry.file_name()
    ///          .to_str()
    ///          .map(|s| s.starts_with("."))
    ///          .unwrap_or(false)
    /// }
    ///
    /// let mut it = WalkDir::new("foo").into_iter();
    /// loop {
    ///     let entry = match it.next() {
    ///         None => break,
    ///         Some(Err(err)) => panic!("ERROR: {}", err),
    ///         Some(Ok(entry)) => entry,
    ///     };
    ///     if is_hidden(&entry) {
    ///         if entry.file_type().is_dir() {
    ///             it.skip_current_dir();
    ///         }
    ///         continue;
    ///     }
    ///     println!("{}", entry.path().display());
    /// }
    /// ```
    ///
    /// You may find it more convenient to use the [`filter_entry`] iterator
    /// adapter. (See its documentation for the same example functionality as
    /// above.)
    ///
    /// [`filter_entry`]: #method.filter_entry
    pub fn skip_current_dir(&mut self) {
        self.it.skip_current_dir();
    }
}
