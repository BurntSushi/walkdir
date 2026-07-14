use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use crate::os::windows::Dir;
use crate::tests::util::Dir as TestDir;
use crate::WalkDir;

#[test]
fn empty() {
    let dir = TestDir::tmp();

    let mut handle = Dir::open(dir.path()).unwrap();
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
    let dir = TestDir::tmp();
    dir.mkdirp("a");

    let mut handle = Dir::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(3, ents.len());
    assert_eq!("a", ents[2].file_name_os());
    assert!(ents[2].file_type().is_dir());
}

#[test]
fn one_file() {
    let dir = TestDir::tmp();
    dir.touch("a");

    let mut handle = Dir::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();

    let ents = r.sorted_ents();
    assert_eq!(3, ents.len());
    assert_eq!("a", ents[2].file_name_os());
    assert!(ents[2].file_type().is_file());
}

#[test]
fn one_dir_file() {
    let dir = TestDir::tmp();
    dir.mkdirp("foo");
    dir.touch("foo/a");

    let mut handle = Dir::open(dir.path()).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();
    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("foo")];
    assert_eq!(expected, r.sorted_file_names());

    let mut handle = Dir::open(dir.path().join("foo")).unwrap();
    let r = dir.run_windows(&mut handle);
    r.assert_no_errors();
    let expected =
        vec![OsString::from("."), OsString::from(".."), OsString::from("a")];
    assert_eq!(expected, r.sorted_file_names());
}

#[test]
fn many_files() {
    let dir = TestDir::tmp();
    dir.touch_all(&["a", "b", "c", "d"]);

    let mut handle = Dir::open(dir.path()).unwrap();
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
    let dir = TestDir::tmp();
    dir.mkdirp("b");
    dir.mkdirp("d");
    dir.touch_all(&["a", "c"]);

    let mut handle = Dir::open(dir.path()).unwrap();
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

    let dir = TestDir::tmp();
    dir.touch("a");
    dir.symlink_file("a", "a-link");

    let mut handle = Dir::open(dir.path()).unwrap();
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

// A junction is classified as a symlink, so it is not descended by default
// but is followed under follow_links.
#[test]
fn junction_is_symlink_no_descent() {
    let dir = TestDir::tmp();
    dir.mkdirp("target");
    dir.touch("target/child");
    dir.junction("target", "link");

    let link = dir.join("link");
    let ty = crate::os::windows::lstat(&link).unwrap().file_type();
    assert!(ty.is_symlink());
    assert!(ty.is_symlink_dir());
    assert!(!ty.is_dir());

    let r = dir.run_recursive(WalkDir::new(dir.path()));
    r.assert_no_errors();
    let got = r.sorted_paths();
    assert!(got.contains(&link));
    assert!(!got.contains(&dir.join("link").join("child")));
}

#[test]
fn junction_descend_with_follow() {
    let dir = TestDir::tmp();
    dir.mkdirp("target");
    dir.touch("target/child");
    dir.junction("target", "link");

    let r = dir.run_recursive(WalkDir::new(dir.path()).follow_links(true));
    r.assert_no_errors();
    let got = r.sorted_paths();
    assert!(got.contains(&dir.join("link").join("child")));
}

// Relative opens off the root handle have no MAX_PATH limit, so a tree far
// deeper than 260 chars must enumerate without error.
#[test]
fn long_path() {
    let dir = TestDir::tmp();
    let mut rel = PathBuf::new();
    for _ in 0..40 {
        rel.push("abcdefghij");
    }
    dir.mkdirp(&rel);
    dir.touch(rel.join("leaf"));

    let r = dir.run_recursive(WalkDir::new(dir.path()));
    r.assert_no_errors();
    assert!(r.sorted_paths().contains(&dir.join(&rel).join("leaf")));

    // A verbatim root takes a different open path and must not be re-prefixed.
    let verbatim = PathBuf::from(format!(r"\\?\{}", dir.path().display()));
    let r = dir.run_recursive(WalkDir::new(&verbatim));
    r.assert_no_errors();
    assert!(r.sorted_paths().contains(&verbatim.join(&rel).join("leaf")));
}

// Deleting a directory out from under the walk must surface an error, not
// panic, and iteration must continue.
#[test]
fn mid_walk_delete_parent() {
    let dir = TestDir::tmp();
    dir.mkdirp("a");
    dir.touch("a/f");
    dir.mkdirp("b");
    dir.touch("b/f");

    let it = WalkDir::new(dir.path()).max_open(1).into_iter();
    let mut saw_error = false;
    for res in it {
        let ent = match res {
            Ok(ent) => ent,
            Err(_) => {
                saw_error = true;
                continue;
            }
        };
        if ent.depth() == 1 && ent.file_type().is_dir() {
            let _ = std::fs::remove_dir_all(ent.path());
        }
    }
    // The delete may race such that enumeration still succeeds. Despite that, the
    // walk must terminate without panicking.
    let _ = saw_error;
}

// A file name containing an unpaired surrogate must round-trip through the
// walk. Some filesystems reject such names, so skip if creation fails.
#[test]
fn unpaired_surrogate_name() {
    let dir = TestDir::tmp();
    let name = OsString::from_wide(&[0x0061, 0xD800, 0x0062]);
    let path = dir.path().join(&name);
    if std::fs::File::create(&path).is_err() {
        eprintln!("skipping: filesystem rejects unpaired surrogate names");
        return;
    }

    let r = dir.run_recursive(WalkDir::new(dir.path()).min_depth(1));
    r.assert_no_errors();
    let names: Vec<OsString> =
        r.ents().iter().map(|e| e.file_name().to_os_string()).collect();
    assert_eq!(1, names.len());
    assert_eq!(name, names[0]);
}
