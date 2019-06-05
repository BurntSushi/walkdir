use std::fs;
use std::path::PathBuf;

use crate::tests::util::{self, Dir};
use crate::Cursor;

#[test]
fn many_mixed() {
    let dir = Dir::tmp();
    dir.mkdirp("foo/a");
    dir.mkdirp("foo/c");
    dir.mkdirp("foo/e");
    dir.touch_all(&["foo/b", "foo/d", "foo/f"]);
    dir.touch_all(&["foo/c/bar", "foo/c/baz"]);
    dir.touch_all(&["foo/a/quux"]);

    let mut cur = Cursor::new(dir.path());
    loop {
        match cur.read() {
            Ok(None) => break,
            Ok(Some(entry)) => {
                println!("{:?}", entry.path());
            }
            Err(err) => {
                println!("ERROR: {}", err);
                break;
            }
        }
    }

    // let r = dir.run_recursive(wd);
    // r.assert_no_errors();
    //
    // let expected = vec![
    // dir.path().to_path_buf(),
    // dir.join("foo"),
    // dir.join("foo").join("a"),
    // dir.join("foo").join("b"),
    // dir.join("foo").join("c"),
    // dir.join("foo").join("d"),
    // dir.join("foo").join("e"),
    // dir.join("foo").join("f"),
    // ];
    // assert_eq!(expected, r.sorted_paths());
}
