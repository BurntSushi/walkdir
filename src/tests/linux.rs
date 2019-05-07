use std::ffi::OsString;
use std::fs;
use std::io::{self, Seek};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use crate::os::unix;
use crate::tests::util::Dir;

#[test]
fn empty() {
    let dir = Dir::tmp();

    let mut dirfd = unix::DirFd::open(dir.path()).unwrap();
    let r = dir.run_linux(&mut dirfd);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(2, ents.len());
    assert_eq!(".", ents[0].file_name_os());
    assert_eq!("..", ents[1].file_name_os());
    assert!(ents[0].file_type().unwrap().is_dir());
    assert!(ents[1].file_type().unwrap().is_dir());
}

#[test]
fn one_dir() {
    let dir = Dir::tmp();
    dir.mkdirp("a");

    let mut dirfd = unix::DirFd::open(dir.path()).unwrap();
    let r = dir.run_linux(&mut dirfd);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(3, ents.len());
    assert_eq!("a", ents[2].file_name_os());
    assert_ne!(0, ents[2].ino());
    assert!(ents[2].file_type().unwrap().is_dir());
}

#[test]
fn one_file() {
    let dir = Dir::tmp();
    dir.touch("a");

    let mut dirfd = unix::DirFd::open(dir.path()).unwrap();
    let r = dir.run_linux(&mut dirfd);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(3, ents.len());
    assert_eq!("a", ents[2].file_name_os());
    assert_ne!(0, ents[2].ino());
    assert!(ents[2].file_type().unwrap().is_file());
}

#[test]
fn one_dir_file() {
    let dir = Dir::tmp();
    dir.mkdirp("foo");
    dir.touch("foo/a");

    let mut dirfd = unix::DirFd::open(dir.path()).unwrap();
    let r = dir.run_linux(&mut dirfd);
    r.assert_no_errors();
    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("foo")];
    assert_eq!(expected, r.sorted_file_names());

    let mut dirfd = unix::DirFd::open(dir.path().join("foo")).unwrap();
    let r = dir.run_linux(&mut dirfd);
    r.assert_no_errors();
    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("a")];
    assert_eq!(expected, r.sorted_file_names());
}

#[test]
fn many_files() {
    let dir = Dir::tmp();
    dir.touch_all(&["a", "b", "c", "d"]);

    let mut dirfd = unix::DirFd::open(dir.path()).unwrap();
    let r = dir.run_linux(&mut dirfd);
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

    let mut dirfd = unix::DirFd::open(dir.path()).unwrap();
    let r = dir.run_linux(&mut dirfd);
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
    assert!(ents[2].file_type().unwrap().is_file());
    assert!(ents[3].file_type().unwrap().is_dir());
    assert!(ents[4].file_type().unwrap().is_file());
    assert!(ents[5].file_type().unwrap().is_dir());
}

#[test]
fn symlink() {
    let dir = Dir::tmp();
    dir.touch("a");
    dir.symlink_file("a", "a-link");

    let mut dirfd = unix::DirFd::open(dir.path()).unwrap();
    let r = dir.run_linux(&mut dirfd);
    r.assert_no_errors();

    let expected = vec![
        OsString::from("."),
        OsString::from(".."),
        OsString::from("a"),
        OsString::from("a-link"),
    ];
    assert_eq!(expected, r.sorted_file_names());

    let ents = r.sorted_ents();
    assert!(ents[2].file_type().unwrap().is_file());
    assert!(ents[3].file_type().unwrap().is_symlink());
}

#[test]
fn openat() {
    let dir = Dir::tmp();
    dir.mkdirp("foo");
    dir.touch("foo/a");

    let mut root = unix::DirFd::open(dir.path()).unwrap();
    let mut foo = unix::DirFd::openat(root.as_raw_fd(), "foo").unwrap();
    let r = dir.run_linux(&mut foo);
    r.assert_no_errors();

    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("a")];
    assert_eq!(expected, r.sorted_file_names());
}

#[test]
fn rewind() {
    let dir = Dir::tmp();
    dir.touch_all(&["a", "b", "c", "d"]);

    let mut dirfd = unix::DirFd::open(dir.path()).unwrap();

    let r = dir.run_linux(&mut dirfd);
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

    let r = dir.run_linux(&mut dirfd);
    r.assert_no_errors();
    assert_eq!(0, r.ents().len());

    dirfd.seek(io::SeekFrom::Start(0)).unwrap();
    let r = dir.run_linux(&mut dirfd);
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
