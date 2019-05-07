use std::ffi::OsString;

use crate::os::windows::FindHandle;
use crate::tests::util::Dir;

#[test]
fn empty() {
    let dir = Dir::tmp();

    let mut handle = FindHandle::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(2, ents.len());
    assert_eq!(".", ents[0].file_name_os());
    assert_eq!("..", ents[1].file_name_os());
    assert!(ents[0].file_type().is_dir());
    assert!(ents[1].file_type().is_dir());
}

#[test]
fn one_dir() {
    let dir = Dir::tmp();
    dir.mkdirp("a");

    let mut handle = FindHandle::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(3, ents.len());
    assert_eq!("a", ents[2].file_name_os());
    assert!(ents[2].file_type().is_dir());
}

#[test]
fn one_file() {
    let dir = Dir::tmp();
    dir.touch("a");

    let mut handle = FindHandle::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(3, ents.len());
    assert_eq!("a", ents[2].file_name_os());
    assert!(ents[2].file_type().is_file());
}

#[test]
fn one_dir_file() {
    let dir = Dir::tmp();
    dir.mkdirp("foo");
    dir.touch("foo/a");

    let mut handle = FindHandle::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();
    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("foo")];
    assert_eq!(expected, r.sorted_file_names());

    let mut handle = FindHandle::open(dir.path().join("foo")).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();
    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("a")];
    assert_eq!(expected, r.sorted_file_names());
}

#[test]
fn many_files() {
    let dir = Dir::tmp();
    dir.touch_all(&["a", "b", "c", "d"]);

    let mut handle = FindHandle::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
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

    let mut handle = FindHandle::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
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
    assert!(ents[2].file_type().is_file());
    assert!(ents[3].file_type().is_dir());
    assert!(ents[4].file_type().is_file());
    assert!(ents[5].file_type().is_dir());
}

#[test]
fn symlink() {
    skip_if_no_symlinks!();

    let dir = Dir::tmp();
    dir.touch("a");
    dir.symlink_file("a", "a-link");

    let mut handle = FindHandle::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();

    let expected = vec![
        OsString::from("."),
        OsString::from(".."),
        OsString::from("a"),
        OsString::from("a-link"),
    ];
    assert_eq!(expected, r.sorted_file_names());

    let ents = r.sorted_ents();
    assert!(ents[2].file_type().is_file());
    assert!(ents[3].file_type().is_symlink());
    assert!(ents[3].file_type().is_symlink_file());
    assert!(!ents[3].file_type().is_symlink_dir());
    assert!(!ents[3].file_type().is_file());
}
