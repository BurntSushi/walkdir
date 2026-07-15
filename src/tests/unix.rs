use std::ffi::OsString;
use std::os::unix::io::AsRawFd;

use crate::os::unix;
use crate::tests::util::Dir;

/// Assert the type only when the platform reports `d_type`.
fn assert_file_type(ft: Option<unix::FileType>, kind: &str) {
    let ft = match ft {
        Some(ft) => ft,
        None => return,
    };
    match kind {
        "dir" => assert!(ft.is_dir(), "expected dir, got {:?}", ft),
        "file" => assert!(ft.is_file(), "expected file, got {:?}", ft),
        "symlink" => {
            assert!(ft.is_symlink(), "expected symlink, got {:?}", ft)
        }
        _ => unreachable!("unknown kind: {}", kind),
    }
}

#[test]
fn empty() {
    let dir = Dir::tmp();

    let mut udir = unix::Dir::open(dir.path()).unwrap();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(2, ents.len());
    assert_eq!(".", ents[0].file_name_os());
    assert_eq!("..", ents[1].file_name_os());
    assert_file_type(ents[0].file_type(), "dir");
    assert_file_type(ents[1].file_type(), "dir");
}

#[test]
fn one_dir() {
    let dir = Dir::tmp();
    dir.mkdirp("a");

    let mut udir = unix::Dir::open(dir.path()).unwrap();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(3, ents.len());
    assert_eq!("a", ents[2].file_name_os());
    assert_ne!(0, ents[2].ino());
    assert_file_type(ents[2].file_type(), "dir");
}

#[test]
fn one_file() {
    let dir = Dir::tmp();
    dir.touch("a");

    let mut udir = unix::Dir::open(dir.path()).unwrap();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(3, ents.len());
    assert_eq!("a", ents[2].file_name_os());
    assert_ne!(0, ents[2].ino());
    assert_file_type(ents[2].file_type(), "file");
}

#[test]
fn one_dir_file() {
    let dir = Dir::tmp();
    dir.mkdirp("foo");
    dir.touch("foo/a");

    let mut udir = unix::Dir::open(dir.path()).unwrap();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();
    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("foo")];
    assert_eq!(expected, r.sorted_file_names());

    let mut udir = unix::Dir::open(dir.path().join("foo")).unwrap();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();
    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("a")];
    assert_eq!(expected, r.sorted_file_names());
}

#[test]
fn many_files() {
    let dir = Dir::tmp();
    dir.touch_all(&["a", "b", "c", "d"]);

    let mut udir = unix::Dir::open(dir.path()).unwrap();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();

    let expected = vec![
        OsString::from("."),
        OsString::from(".."),
        OsString::from("a"),
        OsString::from("b"),
        OsString::from("c"),
        OsString::from("d"),
    ];
    assert_eq!(expected, r.sorted_file_names());
}

#[test]
fn many_mixed() {
    let dir = Dir::tmp();
    dir.mkdirp("b");
    dir.mkdirp("d");
    dir.touch_all(&["a", "c"]);

    let mut udir = unix::Dir::open(dir.path()).unwrap();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();

    let expected = vec![
        OsString::from("."),
        OsString::from(".."),
        OsString::from("a"),
        OsString::from("b"),
        OsString::from("c"),
        OsString::from("d"),
    ];
    assert_eq!(expected, r.sorted_file_names());

    let ents = r.sorted_ents();
    assert_file_type(ents[2].file_type(), "file");
    assert_file_type(ents[3].file_type(), "dir");
    assert_file_type(ents[4].file_type(), "file");
    assert_file_type(ents[5].file_type(), "dir");
}

#[test]
fn symlink() {
    let dir = Dir::tmp();
    dir.touch("a");
    dir.symlink_file("a", "a-link");

    let mut udir = unix::Dir::open(dir.path()).unwrap();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();

    let expected = vec![
        OsString::from("."),
        OsString::from(".."),
        OsString::from("a"),
        OsString::from("a-link"),
    ];
    assert_eq!(expected, r.sorted_file_names());

    let ents = r.sorted_ents();
    assert_file_type(ents[2].file_type(), "file");
    assert_file_type(ents[3].file_type(), "symlink");
}

#[test]
fn openat() {
    let dir = Dir::tmp();
    dir.mkdirp("foo");
    dir.touch("foo/a");

    let root = unix::Dir::open(dir.path()).unwrap();
    let mut foo = unix::Dir::openat(root.as_raw_fd(), "foo").unwrap();
    let r = dir.run_unix(&mut foo);
    r.assert_no_errors();

    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("a")];
    assert_eq!(expected, r.sorted_file_names());
}

#[test]
fn rewind() {
    let dir = Dir::tmp();
    dir.touch_all(&["a", "b", "c", "d"]);

    let mut udir = unix::Dir::open(dir.path()).unwrap();

    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();
    let expected = vec![
        OsString::from("."),
        OsString::from(".."),
        OsString::from("a"),
        OsString::from("b"),
        OsString::from("c"),
        OsString::from("d"),
    ];
    assert_eq!(expected, r.sorted_file_names());

    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();
    assert_eq!(0, r.ents().len());

    udir.rewind();
    let r = dir.run_unix(&mut udir);
    r.assert_no_errors();
    let expected = vec![
        OsString::from("."),
        OsString::from(".."),
        OsString::from("a"),
        OsString::from("b"),
        OsString::from("c"),
        OsString::from("d"),
    ];
    assert_eq!(expected, r.sorted_file_names());
}
