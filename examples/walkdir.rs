extern crate docopt;
extern crate rustc_serialize;
extern crate walkdir;

use docopt::Docopt;
use walkdir::WalkDir;

const USAGE: &'static str = "
Usage:
    walkdir [options] [<dir>]

Options:
    -h, --help
    -L, --follow-links   Follow symlinks.
    -d, --depth          Traverse contents of directories first.
    --min-depth NUM      Minimum depth.
    --max-depth NUM      Maximum depth.
    -n, --fd-max NUM     Maximum open file descriptors. [default: 32]
    --tree               Show output as a tree.
";

#[derive(Debug, RustcDecodable)]
#[allow(dead_code)]
struct Args {
    arg_dir: Option<String>,
    flag_follow_links: bool,
    flag_min_depth: Option<usize>,
    flag_max_depth: Option<usize>,
    flag_fd_max: usize,
    flag_depth: bool,
    flag_tree: bool,
}

fn main() {
    let args: Args = Docopt::new(USAGE).and_then(|d| d.decode())
                                       .unwrap_or_else(|e| e.exit());
    let mind = args.flag_min_depth.unwrap_or(0);
    let maxd = args.flag_max_depth.unwrap_or(::std::usize::MAX);
    let mut it = WalkDir::new(args.arg_dir.unwrap_or(".".to_owned()))
                         .max_open(args.flag_fd_max)
                         .follow_links(args.flag_follow_links)
                         .contents_first(args.flag_depth)
                         .min_depth(mind)
                         .max_depth(maxd)
                         .into_iter();
    if args.flag_tree {
        loop {
            let dent = match it.next() {
                None => break,
                Some(Err(err)) => { println!("ERROR: {}", err); continue }
                Some(Ok(dent)) => dent,
            };
            let name = dent.file_name().into_string().unwrap();
            println!("{}{}", indent(it.depth()), name);
        }
    } else {
        for dent in it {
            match dent {
                Ok(dent) => println!("{}", dent.path().display()),
                Err(err) => println!("ERROR: {}", err),
            }
        }
    }
}

fn indent(depth: usize) -> String {
    ::std::iter::repeat(' ').take(2 * depth).collect()
}
