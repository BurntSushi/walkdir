#![cfg_attr(windows, allow(dead_code, unused_imports))]

use std::cmp;
use std::env;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::collections::HashMap;

use super::{DirEntry, WalkDir, IntoIter, Error, ErrorInner};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Tree {
    Dir(PathBuf, Vec<Tree>),
    File(PathBuf),
    Symlink {
        src: PathBuf,
        dst: PathBuf,
        dir: bool,
    }
}

impl Tree {
    fn from_walk_with<P, F>(
        p: P,
        f: F,
    ) -> io::Result<Tree>
    where P: AsRef<Path>, F: FnOnce(WalkDir) -> WalkDir {
        let mut stack = vec![Tree::Dir(p.as_ref().to_path_buf(), vec![])];
        let it: WalkEventIter = f(WalkDir::new(p)).into();
        for ev in it {
            match try!(ev) {
                WalkEvent::Exit => {
                    let tree = stack.pop().unwrap();
                    if stack.is_empty() {
                        return Ok(tree);
                    }
                    stack.last_mut().unwrap().children_mut().push(tree);
                }
                WalkEvent::Dir(dent) => {
                    stack.push(Tree::Dir(pb(dent.file_name()), vec![]));
                }
                WalkEvent::File(dent) => {
                    let node = if dent.file_type().is_symlink() {
                        let src = try!(dent.path().read_link());
                        let dst = pb(dent.file_name());
                        let dir = dent.path().is_dir();
                        Tree::Symlink { src: src, dst: dst, dir: dir }
                    } else {
                        Tree::File(pb(dent.file_name()))
                    };
                    stack.last_mut().unwrap().children_mut().push(node);
                }
            }
        }
        assert_eq!(stack.len(), 1);
        Ok(stack.pop().unwrap())
    }

    fn from_walk_with_contents_first<P, F>(
        p: P,
        f: F,
    ) -> io::Result<Tree>
    where P: AsRef<Path>, F: FnOnce(WalkDir) -> WalkDir {
        let mut contents_of_dir_at_depth = HashMap::new();
        let mut min_depth = ::std::usize::MAX;
        let top_level_path = p.as_ref().to_path_buf();
        for result in f(WalkDir::new(p).contents_first(true)) {
            let dentry = try!(result);

            let tree =
            if dentry.file_type().is_dir() {
                let any_contents = contents_of_dir_at_depth.remove(
                    &(dentry.depth+1));
            Tree::Dir(pb(dentry.file_name()), any_contents.unwrap_or_default())
            } else {
                if dentry.file_type().is_symlink() {
                    let src = try!(dentry.path().read_link());
                    let dst = pb(dentry.file_name());
                    let dir = dentry.path().is_dir();
                    Tree::Symlink { src: src, dst: dst, dir: dir }
                } else {
                    Tree::File(pb(dentry.file_name()))
                }
            };
            contents_of_dir_at_depth.entry(
                    dentry.depth).or_insert(vec!()).push(tree);
            min_depth = cmp::min(min_depth, dentry.depth);
        }
        Ok(Tree::Dir(top_level_path,
                contents_of_dir_at_depth.remove(&min_depth)
                .unwrap_or_default()))
    }

    fn unwrap_singleton(self) -> Tree {
        match self {
            Tree::File(_) | Tree::Symlink { .. } => {
                panic!("cannot unwrap file or link as dir");
            }
            Tree::Dir(_, mut childs) => {
                assert_eq!(childs.len(), 1);
                childs.pop().unwrap()
            }
        }
    }

    fn unwrap_dir(self) -> Vec<Tree> {
        match self {
            Tree::File(_) | Tree::Symlink { .. } => {
                panic!("cannot unwrap file as dir");
            }
            Tree::Dir(_, childs) => childs,
        }
    }

    fn children_mut(&mut self) -> &mut Vec<Tree> {
        match *self {
            Tree::File(_) | Tree::Symlink { .. } => {
                panic!("files do not have children");
            }
            Tree::Dir(_, ref mut children) => children,
        }
    }

    fn create_in<P: AsRef<Path>>(&self, parent: P) -> io::Result<()> {
        let parent = parent.as_ref();
        match *self {
            Tree::Symlink { ref src, ref dst, dir } => {
                if dir {
                    try!(soft_link_dir(src, parent.join(dst)));
                } else {
                    try!(soft_link_file(src, parent.join(dst)));
                }
            }
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

    fn canonical(&self) -> Tree {
        match *self {
            Tree::Symlink { ref src, ref dst, dir } => {
                Tree::Symlink { src: src.clone(), dst: dst.clone(), dir: dir }
            }
            Tree::File(ref p) => {
                Tree::File(p.clone())
            }
            Tree::Dir(ref p, ref cs) => {
                let mut cs: Vec<Tree> =
                    cs.iter().map(|c| c.canonical()).collect();
                cs.sort();
                Tree::Dir(p.clone(), cs)
            }
        }
    }
}

#[derive(Debug)]
enum WalkEvent {
    Dir(DirEntry),
    File(DirEntry),
    Exit,
}

struct WalkEventIter {
    depth: usize,
    it: IntoIter,
    next: Option<Result<DirEntry, Error>>,
}

impl From<WalkDir> for WalkEventIter {
    fn from(it: WalkDir) -> WalkEventIter {
        WalkEventIter { depth: 0, it: it.into_iter(), next: None }
    }
}

impl Iterator for WalkEventIter {
    type Item = io::Result<WalkEvent>;

    fn next(&mut self) -> Option<io::Result<WalkEvent>> {
        let dent = self.next.take().or_else(|| self.it.next());
        let depth = match dent {
            None => 0,
            Some(Ok(ref dent)) => dent.depth(),
            Some(Err(ref err)) => err.depth(),
        };
        if depth < self.depth {
            self.depth -= 1;
            self.next = dent;
            return Some(Ok(WalkEvent::Exit));
        }
        self.depth = depth;
        match dent {
            None => None,
            Some(Err(err)) => Some(Err(From::from(err))),
            Some(Ok(dent)) => {
                if dent.file_type().is_dir() {
                    self.depth += 1;
                    Some(Ok(WalkEvent::Dir(dent)))
                } else {
                    Some(Ok(WalkEvent::File(dent)))
                }
            }
        }
    }
}

struct TempDir(PathBuf);

impl TempDir {
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let p = env::temp_dir();
    let idx = COUNTER.fetch_add(1, Ordering::SeqCst);
    let ret = p.join(&format!("rust-{}", idx));
    fs::create_dir(&ret).unwrap();
    TempDir(ret)
}

fn dir_setup_with<F>(t: &Tree, f: F) -> (TempDir, Tree)
        where F: Fn(WalkDir) -> WalkDir {
    let tmp = tmpdir();
    t.create_in(tmp.path()).unwrap();
    let got = Tree::from_walk_with(tmp.path(), &f).unwrap();
    let got_cf = Tree::from_walk_with_contents_first(tmp.path(), &f).unwrap();
    assert_eq!(got, got_cf);

    (tmp, got.unwrap_singleton().unwrap_singleton())
}

fn dir_setup(t: &Tree) -> (TempDir, Tree) {
    dir_setup_with(t, |wd| wd)
}

fn canon(unix: &str) -> String {
    if cfg!(windows) {
        unix.replace("/", "\\")
    } else {
        unix.to_string()
    }
}

fn pb<P: AsRef<Path>>(p: P) -> PathBuf { p.as_ref().to_path_buf() }
fn td<P: AsRef<Path>>(p: P, cs: Vec<Tree>) -> Tree {
    Tree::Dir(pb(p), cs)
}
fn tf<P: AsRef<Path>>(p: P) -> Tree {
    Tree::File(pb(p))
}
fn tld<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> Tree {
    Tree::Symlink { src: pb(src), dst: pb(dst), dir: true }
}
fn tlf<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> Tree {
    Tree::Symlink { src: pb(src), dst: pb(dst), dir: false }
}

#[cfg(unix)]
fn soft_link_dir<P: AsRef<Path>, Q: AsRef<Path>>(
    src: P,
    dst: Q,
) -> io::Result<()> {
    use std::os::unix::fs::symlink;
    symlink(src, dst)
}

#[cfg(unix)]
fn soft_link_file<P: AsRef<Path>, Q: AsRef<Path>>(
    src: P,
    dst: Q,
) -> io::Result<()> {
    soft_link_dir(src, dst)
}

#[cfg(windows)]
fn soft_link_dir<P: AsRef<Path>, Q: AsRef<Path>>(
    src: P,
    dst: Q,
) -> io::Result<()> {
    use std::os::windows::fs::symlink_dir;
    symlink_dir(src, dst)
}

#[cfg(windows)]
fn soft_link_file<P: AsRef<Path>, Q: AsRef<Path>>(
    src: P,
    dst: Q,
) -> io::Result<()> {
    use std::os::windows::fs::symlink_file;
    symlink_file(src, dst)
}

macro_rules! assert_tree_eq {
    ($e1:expr, $e2:expr) => {
        assert_eq!($e1.canonical(), $e2.canonical());
    }
}

#[test]
fn walk_dir_1() {
    let exp = td("foo", vec![]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_2() {
    let exp = tf("foo");
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_3() {
    let exp = td("foo", vec![tf("bar")]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_4() {
    let exp = td("foo", vec![tf("foo"), tf("bar"), tf("baz")]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_5() {
    let exp = td("foo", vec![td("bar", vec![])]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_6() {
    let exp = td("foo", vec![
        td("bar", vec![
           tf("baz"), td("bat", vec![]),
        ]),
    ]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_7() {
    let exp = td("foo", vec![
        td("bar", vec![
           tf("baz"), td("bat", vec![]),
        ]),
        td("a", vec![tf("b"), tf("c"), tf("d")]),
    ]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_sym_1() {
    let exp = td("foo", vec![tf("bar"), tlf("bar", "baz")]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_sym_2() {
    let exp = td("foo", vec![
        td("a", vec![tf("a1"), tf("a2")]),
        tld("a", "alink"),
    ]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
fn walk_dir_sym_root() {
    let exp = td("foo", vec![
        td("bar", vec![tf("a"), tf("b")]),
        tld("bar", "alink"),
    ]);
    let tmp = tmpdir();
    let tmp_path = tmp.path();
    let tmp_len = tmp_path.to_str().unwrap().len();
    exp.create_in(tmp_path).unwrap();

    let it = WalkDir::new(tmp_path.join("foo").join("alink")).into_iter();
    let mut got = it
        .map(|d| d.unwrap().path().to_str().unwrap()[tmp_len+1..].into())
        .collect::<Vec<String>>();
    got.sort();
    assert_eq!(got, vec![
        canon("foo/alink"), canon("foo/alink/a"), canon("foo/alink/b"),
    ]);

    let it = WalkDir::new(tmp_path.join("foo/alink/")).into_iter();
    let mut got = it
        .map(|d| d.unwrap().path().to_str().unwrap()[tmp_len+1..].into())
        .collect::<Vec<String>>();
    got.sort();
    assert_eq!(got, vec!["foo/alink/", "foo/alink/a", "foo/alink/b"]);
}

// See: https://github.com/BurntSushi/ripgrep/issues/984
#[test]
#[cfg(unix)]
fn first_path_not_symlink() {
    let exp = td("foo", vec![]);
    let (tmp, _got) = dir_setup(&exp);

    let dents = WalkDir::new(tmp.path().join("foo"))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(1, dents.len());
    assert!(!dents[0].path_is_symlink());
}

// Like first_path_not_symlink, but checks that the first path is not reported
// as a symlink even when we are supposed to be following them.
#[test]
#[cfg(unix)]
fn first_path_not_symlink_follow() {
    let exp = td("foo", vec![]);
    let (tmp, _got) = dir_setup(&exp);

    let dents = WalkDir::new(tmp.path().join("foo"))
        .follow_links(true)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(1, dents.len());
    assert!(!dents[0].path_is_symlink());
}

// See: https://github.com/BurntSushi/walkdir/issues/115
#[test]
#[cfg(unix)]
fn first_path_is_followed() {
    let exp = td("foo", vec![
        td("a", vec![tf("a1"), tf("a2")]),
        td("b", vec![tlf("../a/a1", "alink")]),
    ]);
    let (tmp, _got) = dir_setup(&exp);

    let dents = WalkDir::new(tmp.path().join("foo/b/alink"))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(1, dents.len());
    assert!(dents[0].file_type().is_symlink());
    assert!(dents[0].metadata().unwrap().file_type().is_symlink());

    let dents = WalkDir::new(tmp.path().join("foo/b/alink"))
        .follow_links(true)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(1, dents.len());
    assert!(!dents[0].file_type().is_symlink());
    assert!(!dents[0].metadata().unwrap().file_type().is_symlink());
}

#[test]
#[cfg(unix)]
fn walk_dir_sym_detect_no_follow_no_loop() {
    let exp = td("foo", vec![
        td("a", vec![tf("a1"), tf("a2")]),
        td("b", vec![tld("../a", "alink")]),
    ]);
    let (_tmp, got) = dir_setup(&exp);
    assert_tree_eq!(exp, got);
}

#[test]
#[cfg(unix)]
fn walk_dir_sym_follow_dir() {
    let actual = td("foo", vec![
        td("a", vec![tf("a1"), tf("a2")]),
        td("b", vec![tld("../a", "alink")]),
    ]);
    let followed = td("foo", vec![
        td("a", vec![tf("a1"), tf("a2")]),
        td("b", vec![td("alink", vec![tf("a1"), tf("a2")])]),
    ]);
    let (_tmp, got) = dir_setup_with(&actual, |wd| wd.follow_links(true));
    assert_tree_eq!(followed, got);
}

#[test]
#[cfg(unix)]
fn walk_dir_sym_detect_loop() {
    let actual = td("foo", vec![
        td("a", vec![tlf("../b", "blink"), tf("a1"), tf("a2")]),
        td("b", vec![tlf("../a", "alink")]),
    ]);
    let tmp = tmpdir();
    actual.create_in(tmp.path()).unwrap();
    let got = WalkDir::new(tmp.path())
                      .follow_links(true)
                      .into_iter()
                      .collect::<Result<Vec<_>, _>>();
    match got {
        Ok(x) => panic!("expected loop error, got no error: {:?}", x),
        Err(err @ Error { inner: ErrorInner::Io { .. }, .. }) => {
            panic!("expected loop error, got generic IO error: {:?}", err);
        }
        Err(Error { inner: ErrorInner::Loop { .. }, .. }) => {}
    }
}

#[test]
fn walk_dir_sym_infinite() {
    let actual = tlf("a", "a");
    let tmp = tmpdir();
    actual.create_in(tmp.path()).unwrap();
    let got = WalkDir::new(tmp.path())
                      .follow_links(true)
                      .into_iter()
                      .collect::<Result<Vec<_>, _>>();
    match got {
        Ok(x) => panic!("expected IO error, got no error: {:?}", x),
        Err(Error { inner: ErrorInner::Loop { .. }, .. }) => {
            panic!("expected IO error, but got loop error");
        }
        Err(Error { inner: ErrorInner::Io { .. }, .. }) => {}
    }
}

#[test]
fn walk_dir_min_depth_1() {
    let exp = td("foo", vec![tf("bar")]);
    let (_tmp, got) = dir_setup_with(&exp, |wd| wd.min_depth(1));
    assert_tree_eq!(tf("bar"), got);
}

#[test]
fn walk_dir_min_depth_2() {
    let exp = td("foo", vec![tf("bar"), tf("baz")]);
    let tmp = tmpdir();
    exp.create_in(tmp.path()).unwrap();
    let got = Tree::from_walk_with(tmp.path(), |wd| wd.min_depth(2))
                   .unwrap().unwrap_dir();
    let got_cf = Tree::from_walk_with_contents_first(
                    tmp.path(), |wd| wd.min_depth(2))
                   .unwrap().unwrap_dir();
    assert_eq!(got, got_cf);
    assert_tree_eq!(exp, td("foo", got));
}

#[test]
fn walk_dir_min_depth_3() {
    let exp = td("foo", vec![
        tf("bar"),
        td("abc", vec![tf("xyz")]),
        tf("baz"),
    ]);
    let tmp = tmpdir();
    exp.create_in(tmp.path()).unwrap();
    let got = Tree::from_walk_with(tmp.path(), |wd| wd.min_depth(3))
                   .unwrap().unwrap_dir();
    assert_eq!(vec![tf("xyz")], got);
    let got_cf = Tree::from_walk_with_contents_first(
                    tmp.path(), |wd| wd.min_depth(3))
                   .unwrap().unwrap_dir();
    assert_eq!(got, got_cf);
}

#[test]
fn walk_dir_max_depth_1() {
    let exp = td("foo", vec![tf("bar")]);
    let (_tmp, got) = dir_setup_with(&exp, |wd| wd.max_depth(1));
    assert_tree_eq!(td("foo", vec![]), got);
}

#[test]
fn walk_dir_max_depth_2() {
    let exp = td("foo", vec![tf("bar"), tf("baz")]);
    let (_tmp, got) = dir_setup_with(&exp, |wd| wd.max_depth(1));
    assert_tree_eq!(td("foo", vec![]), got);
}

#[test]
fn walk_dir_max_depth_3() {
    let exp = td("foo", vec![
        tf("bar"),
        td("abc", vec![tf("xyz")]),
        tf("baz"),
    ]);
    let exp_trimmed = td("foo", vec![
        tf("bar"),
        td("abc", vec![]),
        tf("baz"),
    ]);
    let (_tmp, got) = dir_setup_with(&exp, |wd| wd.max_depth(2));
    assert_tree_eq!(exp_trimmed, got);
}

#[test]
fn walk_dir_min_max_depth() {
    let exp = td("foo", vec![
        tf("bar"),
        td("abc", vec![tf("xyz")]),
        tf("baz"),
    ]);
    let tmp = tmpdir();
    exp.create_in(tmp.path()).unwrap();
    let got = Tree::from_walk_with(tmp.path(),
                                   |wd| wd.min_depth(2).max_depth(2))
                   .unwrap().unwrap_dir();
    let got_cf = Tree::from_walk_with_contents_first(tmp.path(),
                                   |wd| wd.min_depth(2).max_depth(2))
                   .unwrap().unwrap_dir();
    assert_eq!(got, got_cf);
    assert_tree_eq!(
        td("foo", vec![tf("bar"), td("abc", vec![]), tf("baz")]),
        td("foo", got));
}

#[test]
fn walk_dir_skip() {
    let exp = td("foo", vec![
        tf("bar"),
        td("abc", vec![tf("xyz")]),
        tf("baz"),
    ]);
    let tmp = tmpdir();
    exp.create_in(tmp.path()).unwrap();
    let mut got = vec![];
    let mut it = WalkDir::new(tmp.path()).min_depth(1).into_iter();
    loop {
        let dent = match it.next().map(|x| x.unwrap()) {
            None => break,
            Some(dent) => dent,
        };
        let name = dent.file_name().to_str().unwrap().to_owned();
        if name == "abc" {
            it.skip_current_dir();
        }
        got.push(name);
    }
    got.sort();
    assert_eq!(got, vec!["abc", "bar", "baz", "foo"]); // missing xyz!
}

#[test]
fn walk_dir_filter() {
    let exp = td("foo", vec![
        tf("bar"),
        td("abc", vec![tf("fit")]),
        tf("faz"),
    ]);
    let tmp = tmpdir();
    let tmp_path = tmp.path().to_path_buf();
    exp.create_in(tmp.path()).unwrap();
    let it = WalkDir::new(tmp.path()).min_depth(1)
                     .into_iter()
                     .filter_entry(move |d| {
                         let n = d.file_name().to_string_lossy().into_owned();
                         !d.file_type().is_dir()
                         || n.starts_with("f")
                         || d.path() == &*tmp_path
                     });
    let mut got = it.map(|d| d.unwrap().file_name().to_str().unwrap().into())
                    .collect::<Vec<String>>();
    got.sort();
    assert_eq!(got, vec!["bar", "faz", "foo"]);
}

#[test]
fn walk_dir_sort() {
    let exp = td("foo", vec![
        tf("bar"),
        td("abc", vec![tf("fit")]),
        tf("faz"),
    ]);
    let tmp = tmpdir();
    let tmp_path = tmp.path();
    let tmp_len = tmp_path.to_str().unwrap().len();
    exp.create_in(tmp_path).unwrap();
    let it = WalkDir::new(tmp_path)
        .sort_by(|a,b| a.file_name().cmp(b.file_name()))
        .into_iter();
    let got = it.map(|d| {
        let path = d.unwrap();
        let path = &path.path().to_str().unwrap()[tmp_len..];
        path.replace("\\", "/")
    }).collect::<Vec<String>>();
    assert_eq!(
        got,
        ["", "/foo", "/foo/abc", "/foo/abc/fit", "/foo/bar", "/foo/faz"]);
}

#[test]
fn walk_dir_sort_small_fd_max() {
    let exp = td("foo", vec![
        tf("bar"),
        td("abc", vec![tf("fit")]),
        tf("faz"),
    ]);
    let tmp = tmpdir();
    let tmp_path = tmp.path();
    let tmp_len = tmp_path.to_str().unwrap().len();
    exp.create_in(tmp_path).unwrap();
    let it = WalkDir::new(tmp_path)
        .max_open(1)
        .sort_by(|a,b| a.file_name().cmp(b.file_name()))
        .into_iter();
    let got = it.map(|d| {
        let path = d.unwrap();
        let path = &path.path().to_str().unwrap()[tmp_len..];
        path.replace("\\", "/")
    }).collect::<Vec<String>>();
    assert_eq!(
        got,
        ["", "/foo", "/foo/abc", "/foo/abc/fit", "/foo/bar", "/foo/faz"]);
}

#[test]
fn walk_dir_send_sync_traits() {
    use FilterEntry;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    assert_send::<WalkDir>();
    assert_sync::<WalkDir>();
    assert_send::<IntoIter>();
    assert_sync::<IntoIter>();
    assert_send::<FilterEntry<IntoIter, u8>>();
    assert_sync::<FilterEntry<IntoIter, u8>>();
}

// We cannot mount different volumes for the sake of the test, but
// on Linux systems we can assume that /sys is a mounted volume.
#[test]
#[cfg(target_os = "linux")]
fn walk_dir_stay_on_file_system() {
    // If for some reason /sys doesn't exist or isn't a directory, just skip
    // this test.
    if !Path::new("/sys").is_dir() {
        return;
    }

    let actual = td("same_file", vec![
        td("a", vec![tld("/sys", "alink")]),
    ]);
    let unfollowed = td("same_file", vec![
        td("a", vec![tld("/sys", "alink")]),
    ]);
    let (_tmp, got) = dir_setup_with(&actual, |wd| wd);
    assert_tree_eq!(unfollowed, got);

    // Create a symlink to sys and enable following symlinks. If the
    // same_file_system option doesn't work, then this probably will hit a
    // permission error. Otherwise, it should just skip over the symlink
    // completely.
    let actual = td("same_file", vec![
        td("a", vec![tld("/sys", "alink")]),
    ]);
    let followed = td("same_file", vec![
        td("a", vec![td("alink", vec![])]),
    ]);
    let (_tmp, got) = dir_setup_with(&actual, |wd| {
        wd.follow_links(true).same_file_system(true)
    });
    assert_tree_eq!(followed, got);
}

