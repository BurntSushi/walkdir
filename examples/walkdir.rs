#![feature(fs_walk)]

extern crate docopt;
extern crate rustc_serialize;
extern crate walkdir;

use std::fs;

use docopt::Docopt;
use walkdir::WalkDirBuilder;

const USAGE: &'static str = "
Usage:
    walkdir [options] [<dir>]
    walkdir [options] old <dir>

Options:
    -L, --follow-links   Follow symlinks.
    -d, --depth          Traverse contents of directories first.
    --min-depth NUM      Minimum depth.
    --max-depth NUM      Maximum depth.
    -n, --fd-max NUM     Maximum open file descriptors. [default: 32]
";

#[derive(Debug, RustcDecodable)]
#[allow(dead_code)]
struct Args {
    cmd_old: bool,
    arg_dir: Option<String>,
    flag_follow_links: bool,
    flag_min_depth: usize,
    flag_max_depth: usize,
    flag_fd_max: usize,
    flag_depth: bool,
}

fn main() {
    let args: Args = Docopt::new(USAGE).and_then(|d| d.decode())
                                       .unwrap_or_else(|e| e.exit());
    if args.cmd_old {
        let it = fs::walk_dir(args.arg_dir.unwrap_or(".".to_owned())).unwrap();
        for dent in it {
            match dent {
                Err(err) => println!("ERROR: {}", err),
                Ok(dent) => println!("{}", dent.path().display()),
            }
        }
        return;
    }

    let it = WalkDirBuilder::new(args.arg_dir.unwrap_or(".".to_owned()))
                            .max_open(args.flag_fd_max)
                            .follow_links(args.flag_follow_links)
                            .contents_first(args.flag_depth);
    for dent in it {
        match dent {
            Err(err) => println!("ERROR: {}", err),
            Ok(dent) => println!("{}", dent.path().display()),
        }
    }
}
