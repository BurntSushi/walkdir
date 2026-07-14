use std::ffi::CString;

use crate::dir::DirList;
use crate::tests::util::Dir;

fn cstr(p: &std::path::Path) -> CString {
    use std::os::unix::ffi::OsStrExt;
    CString::new(p.as_os_str().as_bytes()).unwrap()
}

#[test]
fn dirlist_reads_all_entries() {
    let dir = Dir::tmp();
    dir.touch_all(&["a", "b", "c"]);

    let mut list = DirList::open_path(
        0,
        dir.path().to_path_buf(),
        &cstr(dir.path()),
        true,
    );
    let mut names = vec![];
    while let Some(res) = list.next() {
        res.unwrap();
        let name = list.entry().file_name_bytes().to_vec();
        if name == b"." || name == b".." {
            continue;
        }
        names.push(String::from_utf8(name).unwrap());
    }
    names.sort();
    assert_eq!(names, vec!["a", "b", "c"]);
}

#[test]
fn openat_fifo_fails_fast() {
    use std::os::unix::io::AsRawFd;

    let dir = Dir::tmp();
    let fifo_path = cstr(&dir.join("fifo"));
    // SAFETY: fifo_path is NUL terminated.
    let rc = unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o644) };
    assert!(rc == 0, "mkfifo failed: {}", std::io::Error::last_os_error());

    // A dir swapped for a FIFO must fail to open, not block waiting for a writer.
    let dfd = crate::os::unix::DirFd::open_c(&cstr(dir.path())).unwrap();
    let name = CString::new("fifo").unwrap();
    let mut list =
        DirList::openat(1, dfd.as_raw_fd(), &name, false, dir.join("fifo"));
    assert!(list.is_failed());
    assert!(list.next().unwrap().is_err());
}

#[test]
fn file_type_eq_ignores_permissions() {
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = Dir::tmp();
    dir.mkdirp("d1");
    dir.mkdirp("d2");
    dir.touch_all(&["f"]);
    fs::set_permissions(dir.join("d2"), fs::Permissions::from_mode(0o700))
        .unwrap();

    let ft = |name: &str| -> crate::FileType {
        crate::os::unix::lstat(dir.join(name)).unwrap().file_type().into()
    };
    assert_eq!(ft("d1"), ft("d2"));
    assert_ne!(ft("d1"), ft("f"));
    let mut set = HashSet::new();
    set.insert(ft("d1"));
    assert!(set.contains(&ft("d2")));
}

#[test]
fn walk_yields_root_and_children() {
    let dir = Dir::tmp();
    dir.mkdirp("a");
    dir.touch_all(&["a/x", "b"]);

    let mut paths = crate::WalkDir::new(dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_path_buf())
        .collect::<Vec<_>>();
    paths.sort();

    let mut expected = vec![
        dir.path().to_path_buf(),
        dir.join("a"),
        dir.join("a").join("x"),
        dir.join("b"),
    ];
    expected.sort();
    assert_eq!(paths, expected);
}

#[test]
fn deep_tree_small_max_open() {
    let dir = Dir::tmp();
    // Build a chain deeper than max_open to force spill + fallback.
    let mut p = String::new();
    for i in 0..20 {
        p.push_str(&format!("d{i}/"));
    }
    dir.mkdirp(&p);

    let count = crate::WalkDir::new(dir.path())
        .max_open(2)
        .into_iter()
        .filter_map(|e| e.ok())
        .count();
    // root + 20 directories
    assert_eq!(count, 21);
}

#[test]
fn nofollow_symlink_dir_not_descended() {
    skip_if_no_symlinks!();
    let dir = Dir::tmp();
    dir.mkdirp("real");
    dir.touch_all(&["real/inside"]);
    dir.symlink_dir("real", "link");

    let mut names = crate::WalkDir::new(dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path() != dir.path())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect::<Vec<String>>();
    names.sort();
    // `link` is yielded (as a symlink) but not descended into.
    assert!(names.contains(&"link".to_string()));
    // `inside` appears once (under real), never under link.
    assert_eq!(names.iter().filter(|n| *n == "inside").count(), 1);
}

// Build a deep tree with openat so no syscall sees the full path.
fn mkdir_chain_deep(root: &std::path::Path, comp: &str, depth: usize) {
    use crate::os::unix::DirFd;
    use std::os::unix::io::AsRawFd;

    let comp_c = CString::new(comp).unwrap();
    let mut fd = DirFd::open_c(&cstr(root)).unwrap();
    for _ in 0..depth {
        // SAFETY: comp_c is NUL-terminated and fd is a live directory fd.
        let rc =
            unsafe { libc::mkdirat(fd.as_raw_fd(), comp_c.as_ptr(), 0o755) };
        assert!(
            rc == 0,
            "mkdirat failed: {}",
            std::io::Error::last_os_error()
        );
        fd = DirFd::openat_c(fd.as_raw_fd(), &comp_c).unwrap();
    }
}

#[test]
fn long_total_path_traversal() {
    let dir = Dir::tmp();
    // Many nested dirs whose joined path exceeds PATH_MAX, each component short.
    let comp = "abcdefghijklmnopqrstuvwxyz0123"; // 30 chars
    mkdir_chain_deep(dir.path(), comp, 200); // assembled path > 4096 bytes

    let count = crate::WalkDir::new(dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .count();
    assert_eq!(count, 201); // root + 200 dirs, no ENAMETOOLONG

    // max_open(2) keeps the immediate parent open during linear descent.
    let count_max_open_2 = crate::WalkDir::new(dir.path())
        .max_open(2)
        .into_iter()
        .filter_map(|e| e.ok())
        .count();
    assert_eq!(count_max_open_2, 201);
}

#[test]
fn iterators_are_fused() {
    fn assert_fused<I: std::iter::FusedIterator>() {}
    assert_fused::<crate::IntoIter>();
    assert_fused::<
        crate::FilterEntry<crate::IntoIter, fn(&crate::DirEntry) -> bool>,
    >();
}

#[test]
fn open_failure_preserves_path() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = Dir::tmp();
    dir.mkdirp("sub");
    dir.touch_all(&["sub/inside"]);
    let sub = dir.join("sub");
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();

    // Check streaming and sort_by because sort_by builds eagerly.
    let streamed = crate::WalkDir::new(dir.path())
        .into_iter()
        .filter_map(|e| e.err())
        .collect::<Vec<_>>();
    let sorted = crate::WalkDir::new(dir.path())
        .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        .into_iter()
        .filter_map(|e| e.err())
        .collect::<Vec<_>>();

    let _ = fs::set_permissions(&sub, fs::Permissions::from_mode(0o755));

    // Root may bypass permissions, but any open error must keep the path.
    for errs in [&streamed, &sorted] {
        let had_error = !errs.is_empty();
        let has_path = errs.iter().any(|e| e.path() == Some(sub.as_path()));
        assert!(
            !had_error || has_path,
            "dir-open error lost its path: {:?}",
            errs
        );
    }
}
