// This program isn't necessarily meant to serve as an example of how to use
// walkdir, but rather, is a good example of how a basic `find` utility can be
// written using walkdir in a way that is both correct and as fast as possible.
// This includes doing things like block buffering when not printing to a tty,
// and correctly writing file paths to stdout without allocating on Unix.
//
// Additionally, this program is useful for demonstrating all of walkdir's
// features. That is, when new functionality is added, some demonstration of
// it should be added to this program.
//
// Finally, this can be useful for ad hoc benchmarking. e.g., See the --timeit
// and --count flags.

use std::error::Error;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::result;
use std::time::Instant;

use bstr::BString;
use walkdir::WalkDir;

type Result<T> = result::Result<T, Box<dyn Error>>;

macro_rules! err {
    ($($tt:tt)*) => { Err(From::from(format!($($tt)*))) }
}

fn main() {
    if let Err(err) = try_main() {
        eprintln!("{}", err);
        process::exit(1);
    }
}

fn try_main() -> Result<()> {
    let args = Args::parse()?;
    let mut stderr = io::stderr();

    let start = Instant::now();
    if args.count {
        print_count(&args, io::stdout(), &mut stderr)?;
    } else if atty::is(atty::Stream::Stdout) {
        print_paths(&args, io::stdout(), &mut stderr)?;
    } else {
        print_paths(&args, io::BufWriter::new(io::stdout()), &mut stderr)?;
    }
    if args.timeit {
        let since = Instant::now().duration_since(start);
        writeln!(stderr, "duration: {:?}", since)?;
    }
    Ok(())
}

fn print_count<W1, W2>(
    args: &Args,
    mut stdout: W1,
    mut stderr: W2,
) -> Result<()>
where
    W1: io::Write,
    W2: io::Write,
{
    let mut count: u64 = 0;
    for dir in &args.dirs {
        for result in args.walkdir(dir) {
            match result {
                Ok(_) => count += 1,
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
    }
    writeln!(stdout, "{}", count)?;
    Ok(())
}

fn print_paths<W1, W2>(
    args: &Args,
    mut stdout: W1,
    mut stderr: W2,
) -> Result<()>
where
    W1: io::Write,
    W2: io::Write,
{
    for dir in &args.dirs {
        if args.tree {
            print_paths_tree(&args, &mut stdout, &mut stderr, dir)?;
        } else {
            print_paths_flat(&args, &mut stdout, &mut stderr, dir)?;
        }
    }
    Ok(())
}

fn print_paths_flat<W1, W2>(
    args: &Args,
    mut stdout: W1,
    mut stderr: W2,
    dir: &Path,
) -> Result<()>
where
    W1: io::Write,
    W2: io::Write,
{
    for result in args.walkdir(dir) {
        let dent = match result {
            Ok(dent) => dent,
            Err(err) => {
                if !args.ignore_errors {
                    writeln!(stderr, "ERROR: {}", err)?;
                }
                continue;
            }
        };

        if dent.is_dir() {
            if !args.justfiles {
                write_path(&mut stdout, dent.path())?;
                stdout.write_all(b"\n")?;
            }
        } else {
            write_path(&mut stdout, dent.path())?;
            stdout.write_all(b"\n")?;
        }
    }
    Ok(())
}

fn print_paths_tree<W1, W2>(
    args: &Args,
    mut stdout: W1,
    mut stderr: W2,
    dir: &Path,
) -> Result<()>
where
    W1: io::Write,
    W2: io::Write,
{
    for result in args.walkdir(dir) {
        let dent = match result {
            Ok(dent) => dent,
            Err(err) => {
                if !args.ignore_errors {
                    writeln!(stderr, "ERROR: {}", err)?;
                }
                continue;
            }
        };

        stdout.write_all("  ".repeat(dent.depth()).as_bytes())?;
        write_os_str(&mut stdout, dent.file_name())?;
        stdout.write_all(b"\n")?;
    }
    Ok(())
}

#[derive(Debug)]
struct Args {
    dirs: Vec<PathBuf>,
    follow_links: bool,
    min_depth: Option<usize>,
    max_depth: Option<usize>,
    max_open: Option<usize>,
    tree: bool,
    ignore_errors: bool,
    sort: bool,
    depth_first: bool,
    same_file_system: bool,
    timeit: bool,
    count: bool,
    justfiles: bool,
}

impl Args {
    fn parse() -> Result<Args> {
        use clap::{crate_authors, crate_version, App, Arg};

        let parsed = App::new("List files using walkdir")
            .author(crate_authors!())
            .version(crate_version!())
            .max_term_width(100)
            .arg(Arg::with_name("dirs").multiple(true))
            .arg(
                Arg::with_name("follow-links")
                    .long("follow-links")
                    .short("L")
                    .help("Follow symbolic links."),
            )
            .arg(
                Arg::with_name("min-depth")
                    .long("min-depth")
                    .takes_value(true)
                    .help("Only show entries at or above this depth."),
            )
            .arg(
                Arg::with_name("max-depth")
                    .long("max-depth")
                    .takes_value(true)
                    .help("Only show entries at or below this depth."),
            )
            .arg(
                Arg::with_name("max-open")
                    .long("max-open")
                    .takes_value(true)
                    .default_value("10")
                    .help("Use at most this many open file descriptors."),
            )
            .arg(
                Arg::with_name("tree")
                    .long("tree")
                    .help("Show file paths in a tree."),
            )
            .arg(
                Arg::with_name("ignore-errors")
                    .long("ignore-errors")
                    .short("q")
                    .help("Don't print error messages."),
            )
            .arg(
                Arg::with_name("sort")
                    .long("sort")
                    .help("Sort file paths lexicographically."),
            )
            .arg(
                Arg::with_name("depth-first").long("depth-first").help(
                    "Show directory contents before the directory path.",
                ),
            )
            .arg(
                Arg::with_name("same-file-system")
                    .long("same-file-system")
                    .short("x")
                    .help(
                        "Only show paths on the same file system as the root.",
                    ),
            )
            .arg(
                Arg::with_name("timeit")
                    .long("timeit")
                    .short("t")
                    .help("Print timing info."),
            )
            .arg(
                Arg::with_name("count")
                    .long("count")
                    .short("c")
                    .help("Print only a total count of all file paths."),
            )
            .arg(
                Arg::with_name("justfiles")
                    .long("justfiles")
                    .short("j")
                    .help("Print only the file paths like find type f"),
            )
            .get_matches();

        let dirs = match parsed.values_of_os("dirs") {
            None => vec![PathBuf::from("./")],
            Some(dirs) => dirs.map(PathBuf::from).collect(),
        };
        Ok(Args {
            dirs: dirs,
            follow_links: parsed.is_present("follow-links"),
            min_depth: parse_usize(&parsed, "min-depth")?,
            max_depth: parse_usize(&parsed, "max-depth")?,
            max_open: parse_usize(&parsed, "max-open")?,
            tree: parsed.is_present("tree"),
            ignore_errors: parsed.is_present("ignore-errors"),
            sort: parsed.is_present("sort"),
            depth_first: parsed.is_present("depth-first"),
            same_file_system: parsed.is_present("same-file-system"),
            timeit: parsed.is_present("timeit"),
            count: parsed.is_present("count"),
            justfiles: parsed.is_present("justfiles"),
        })
    }

    fn walkdir(&self, path: &Path) -> WalkDir {
        let mut walkdir = WalkDir::new(path)
            .follow_links(self.follow_links)
            .contents_first(self.depth_first)
            .same_file_system(self.same_file_system);
        if let Some(x) = self.min_depth {
            walkdir = walkdir.min_depth(x);
        }
        if let Some(x) = self.max_depth {
            walkdir = walkdir.max_depth(x);
        }
        if let Some(x) = self.max_open {
            walkdir = walkdir.max_open(x);
        }
        if self.sort {
            walkdir = walkdir.sort_by(|a, b| a.file_name().cmp(b.file_name()));
        }
        walkdir
    }
}

fn parse_usize(
    parsed: &clap::ArgMatches,
    flag: &str,
) -> Result<Option<usize>> {
    match parsed.value_of_lossy(flag) {
        None => Ok(None),
        Some(x) => x.parse().map(Some).or_else(|e| {
            err!("failed to parse --{} as a number: {}", flag, e)
        }),
    }
}

fn write_path<W: io::Write>(wtr: W, path: &Path) -> io::Result<()> {
    write_os_str(wtr, path.as_os_str())
}

fn write_os_str<W: io::Write>(mut wtr: W, os: &OsStr) -> io::Result<()> {
    // On Unix, this is a no-op, and correctly prints raw paths. On Windows,
    // this lossily converts paths that originally contained invalid UTF-16
    // to paths that will ultimately contain only valid UTF-16. This doesn't
    // correctly print all possible paths, but on Windows, one can't print
    // invalid UTF-16 to a console anyway.
    wtr.write_all(BString::from_os_str_lossy(os).as_bytes())
}
