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
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::result;
use std::time::Instant;

use bstr::BString;
use walkdir::Cursor;
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
    fn count_walkdir<W: io::Write>(
        args: &Args,
        mut stderr: W,
        dir: &Path,
    ) -> Result<CountResult> {
        let mut res = args.empty_count_result();
        for result in args.walkdir(dir) {
            match result {
                Ok(dent) => {
                    res.count += 1;
                    if let Some(ref mut size) = res.size {
                        *size = dent.metadata()?.len();
                    }
                }
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(res)
    }

    fn count_std<W: io::Write>(
        args: &Args,
        mut stderr: W,
        dir: &Path,
    ) -> Result<CountResult> {
        let mut res = args.empty_count_result();
        for result in fs::read_dir(dir)? {
            match result {
                Ok(dent) => {
                    res.count += 1;
                    if let Some(ref mut size) = res.size {
                        *size = dent.metadata()?.len();
                    }
                }
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(res)
    }

    fn count_cursor<W: io::Write>(
        args: &Args,
        mut stderr: W,
        dir: &Path,
    ) -> Result<CountResult> {
        let mut res = args.empty_count_result();
        let mut cursor = args.cursor(dir);
        loop {
            match cursor.read() {
                Ok(None) => break,
                Ok(Some(entry)) => {
                    res.count += 1;
                    if let Some(ref mut size) = res.size {
                        let md = entry.path().metadata()?;
                        *size = md.len();
                    }
                }
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(res)
    }

    #[cfg(windows)]
    fn count_windows<W: io::Write>(
        args: &Args,
        mut stderr: W,
        dir: &Path,
    ) -> Result<CountResult> {
        use walkdir::os::windows;

        let mut res = args.empty_count_result();
        let mut handle = windows::FindHandle::open(dir)?;
        let mut dent = windows::DirEntry::empty();
        loop {
            match handle.read_into(&mut dent) {
                Ok(true) => {
                    res.count += 1;
                    if let Some(ref mut size) = res.size {
                        let md = dir.join(dent.file_name_os()).metadata()?;
                        *size = md.len();
                    }
                }
                Ok(false) => break,
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(res)
    }

    #[cfg(not(windows))]
    fn count_windows<W: io::Write>(
        _args: &Args,
        _stderr: W,
        _dir: &Path,
    ) -> Result<CountResult> {
        err!("cannot use --flat-windows on non-Windows platform")
    }

    #[cfg(unix)]
    fn count_unix<W: io::Write>(
        args: &Args,
        mut stderr: W,
        dir: &Path,
    ) -> Result<CountResult> {
        use walkdir::os::unix;

        let mut res = args.empty_count_result();
        let mut udir = unix::Dir::open(dir)?;
        let mut dent = unix::DirEntry::empty();
        loop {
            match udir.read_into(&mut dent) {
                Ok(true) => {
                    res.count += 1;
                    if let Some(ref mut size) = res.size {
                        let md = dir.join(dent.file_name_os()).metadata()?;
                        *size = md.len();
                    }
                }
                Ok(false) => break,
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(res)
    }

    #[cfg(not(unix))]
    fn count_unix<W: io::Write>(
        _args: &Args,
        _stderr: W,
        _dir: &Path,
    ) -> Result<CountResult> {
        err!("cannot use --flat-unix on non-Unix platform")
    }

    #[cfg(target_os = "linux")]
    fn count_linux<W: io::Write>(
        args: &Args,
        mut stderr: W,
        dir: &Path,
    ) -> Result<CountResult> {
        use std::os::unix::io::AsRawFd;
        use walkdir::os::{linux, unix};

        let mut res = args.empty_count_result();
        let udir = unix::Dir::open(dir)?;
        let mut cursor = linux::DirEntryCursor::new();
        loop {
            match linux::getdents(udir.as_raw_fd(), &mut cursor) {
                Ok(true) => {
                    while let Some(dent) = cursor.read() {
                        res.count += 1;
                        if let Some(ref mut size) = res.size {
                            let md =
                                dir.join(dent.file_name_os()).metadata()?;
                            *size = md.len();
                        }
                    }
                }
                Ok(false) => break,
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(res)
    }

    #[cfg(not(target_os = "linux"))]
    fn count_linux<W: io::Write>(
        _args: &Args,
        _stderr: W,
        _dir: &Path,
    ) -> Result<CountResult> {
        err!("cannot use --flat-linux on non-Linux platform")
    }

    let mut res = args.empty_count_result();
    for dir in &args.dirs {
        if args.flat_std {
            res = res.add(count_std(args, &mut stderr, &dir)?);
        } else if args.flat_windows {
            res = res.add(count_windows(args, &mut stderr, &dir)?);
        } else if args.flat_unix {
            res = res.add(count_unix(args, &mut stderr, &dir)?);
        } else if args.flat_linux {
            res = res.add(count_linux(args, &mut stderr, &dir)?);
        } else if args.flat_cursor {
            res = res.add(count_cursor(args, &mut stderr, &dir)?);
        } else {
            res = res.add(count_walkdir(args, &mut stderr, &dir)?);
        }
    }
    match res.size {
        Some(size) => {
            writeln!(stdout, "{} (file size: {})", res.count, size)?;
        }
        None => {
            writeln!(stdout, "{}", res.count)?;
        }
    }
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
    fn print_walkdir<W1, W2>(
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
            write_path(&mut stdout, dent.path())?;
            stdout.write_all(b"\n")?;
        }
        Ok(())
    }

    fn print_std<W1, W2>(
        args: &Args,
        mut stdout: W1,
        mut stderr: W2,
        dir: &Path,
    ) -> Result<()>
    where
        W1: io::Write,
        W2: io::Write,
    {
        for result in fs::read_dir(dir)? {
            let dent = match result {
                Ok(dent) => dent,
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                    continue;
                }
            };
            write_os_str(&mut stdout, &dent.file_name())?;
            stdout.write_all(b"\n")?;
        }
        Ok(())
    }

    fn print_cursor<W1: io::Write, W2: io::Write>(
        args: &Args,
        mut stdout: W1,
        mut stderr: W2,
        dir: &Path,
    ) -> Result<()> {
        let mut cursor = args.cursor(dir);
        loop {
            match cursor.read() {
                Ok(None) => break,
                Ok(Some(entry)) => {
                    write_os_str(&mut stdout, entry.path().as_os_str())?;
                    stdout.write_all(b"\n")?;
                }
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn print_windows<W1, W2>(
        args: &Args,
        mut stdout: W1,
        mut stderr: W2,
        dir: &Path,
    ) -> Result<()>
    where
        W1: io::Write,
        W2: io::Write,
    {
        use walkdir::os::windows;

        let mut handle = windows::FindHandle::open(dir)?;
        let mut dent = windows::DirEntry::empty();
        loop {
            match handle.read_into(&mut dent) {
                Ok(true) => {
                    write_os_str(&mut stdout, dent.file_name_os())?;
                    stdout.write_all(b"\n")?;
                }
                Ok(false) => break,
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn print_windows<W1, W2>(
        _args: &Args,
        _stdout: W1,
        _stderr: W2,
        _dir: &Path,
    ) -> Result<u64>
    where
        W1: io::Write,
        W2: io::Write,
    {
        err!("cannot use --flat-windows on non-Windows platform")
    }

    #[cfg(unix)]
    fn print_unix<W1, W2>(
        args: &Args,
        mut stdout: W1,
        mut stderr: W2,
        dir: &Path,
    ) -> Result<()>
    where
        W1: io::Write,
        W2: io::Write,
    {
        use walkdir::os::unix;

        let mut dir = unix::Dir::open(dir)?;
        let mut dent = unix::DirEntry::empty();
        loop {
            match dir.read_into(&mut dent) {
                Ok(true) => {
                    write_os_str(&mut stdout, dent.file_name_os())?;
                    stdout.write_all(b"\n")?;
                }
                Ok(false) => break,
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn print_unix<W1, W2>(
        _args: &Args,
        _stdout: W1,
        _stderr: W2,
        _dir: &Path,
    ) -> Result<()>
    where
        W1: io::Write,
        W2: io::Write,
    {
        err!("cannot use --flat-unix on non-Unix platform")
    }

    #[cfg(target_os = "linux")]
    fn print_linux<W1, W2>(
        args: &Args,
        mut stdout: W1,
        mut stderr: W2,
        dir: &Path,
    ) -> Result<()>
    where
        W1: io::Write,
        W2: io::Write,
    {
        use std::os::unix::io::AsRawFd;
        use walkdir::os::{linux, unix};

        let dir = unix::Dir::open(dir)?;
        let mut cursor = linux::DirEntryCursor::new();
        loop {
            match linux::getdents(dir.as_raw_fd(), &mut cursor) {
                Ok(true) => {
                    while let Some(dent) = cursor.read() {
                        write_os_str(&mut stdout, dent.file_name_os())?;
                        stdout.write_all(b"\n")?;
                    }
                }
                Ok(false) => break,
                Err(err) => {
                    if !args.ignore_errors {
                        writeln!(stderr, "ERROR: {}", err)?;
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn print_linux<W1, W2>(
        _args: &Args,
        _stdout: W1,
        _stderr: W2,
        _dir: &Path,
    ) -> Result<()>
    where
        W1: io::Write,
        W2: io::Write,
    {
        err!("cannot use --flat-linux on non-Linux platform")
    }

    for dir in &args.dirs {
        if args.flat_std {
            print_std(&args, &mut stdout, &mut stderr, dir)?;
        } else if args.flat_windows {
            print_windows(&args, &mut stdout, &mut stderr, dir)?;
        } else if args.flat_unix {
            print_unix(&args, &mut stdout, &mut stderr, dir)?;
        } else if args.flat_linux {
            print_linux(&args, &mut stdout, &mut stderr, dir)?;
        } else if args.flat_cursor {
            print_cursor(&args, &mut stdout, &mut stderr, dir)?;
        } else {
            print_walkdir(&args, &mut stdout, &mut stderr, dir)?;
        }
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
    ignore_errors: bool,
    sort: bool,
    depth_first: bool,
    same_file_system: bool,
    timeit: bool,
    count: bool,
    file_size: bool,
    flat_std: bool,
    flat_windows: bool,
    flat_unix: bool,
    flat_linux: bool,
    flat_cursor: bool,
}

impl Args {
    fn parse() -> Result<Args> {
        use clap::{crate_authors, crate_version, App, Arg};

        let mut app = App::new("List files using walkdir")
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
            .arg(Arg::with_name("file-size").long("file-size").help(
                "Print the total file size of all files. This \
                 implies --count.",
            ))
            .arg(
                Arg::with_name("flat-std")
                    .long("flat-std")
                    .conflicts_with("flat-unix")
                    .conflicts_with("flat-linux")
                    .conflicts_with("flat-windows")
                    .conflicts_with("flat-cursor")
                    .help(
                        "Use std::fs::read_dir to list contents of a single \
                         directory. This is NOT recursive.",
                    ),
            )
            .arg(
                Arg::with_name("flat-cursor")
                    .long("flat-cursor")
                    .conflicts_with("flat-std")
                    .conflicts_with("flat-unix")
                    .conflicts_with("flat-linux")
                    .conflicts_with("flat-windows")
                    .help(
                        "Use walkdir::Cursor to recursively list the contents \
                         of a single directory.",
                    ),
            );
        if cfg!(unix) {
            app = app.arg(
                Arg::with_name("flat-unix")
                    .long("flat-unix")
                    .conflicts_with("flat-std")
                    .conflicts_with("flat-linux")
                    .conflicts_with("flat-windows")
                    .conflicts_with("flat-cursor")
                    .help(
                        "Use Unix-specific APIs to list contents of a single \
                         directory. This is NOT recursive.",
                    ),
            );
        }
        if cfg!(target_os = "linux") {
            app = app.arg(
                Arg::with_name("flat-linux")
                    .long("flat-linux")
                    .conflicts_with("flat-std")
                    .conflicts_with("flat-unix")
                    .conflicts_with("flat-windows")
                    .conflicts_with("flat-cursor")
                    .help(
                        "Use Linux-specific syscalls (getdents64) to list \
                         contents of a single directory. This is NOT \
                         recursive.",
                    ),
            );
        }
        if cfg!(windows) {
            app = app
                .arg(Arg::with_name("flat-windows")
                .long("flat-windows")
                .conflicts_with("flat-std")
                .conflicts_with("flat-unix")
                .conflicts_with("flat-linux")
                .conflicts_with("flat-cursor")
                .help("Use Windows-specific APIs to list contents of a single \
                       directory. This is NOT recursive."));
        }
        let parsed = app.get_matches();

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
            ignore_errors: parsed.is_present("ignore-errors"),
            sort: parsed.is_present("sort"),
            depth_first: parsed.is_present("depth-first"),
            same_file_system: parsed.is_present("same-file-system"),
            timeit: parsed.is_present("timeit"),
            count: parsed.is_present("count"),
            file_size: parsed.is_present("file-size"),
            flat_std: parsed.is_present("flat-std"),
            flat_windows: parsed.is_present("flat-windows"),
            flat_unix: parsed.is_present("flat-unix"),
            flat_linux: parsed.is_present("flat-linux"),
            flat_cursor: parsed.is_present("flat-cursor"),
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

    fn cursor(&self, path: &Path) -> Cursor {
        Cursor::new(path)
    }

    fn empty_count_result(&self) -> CountResult {
        CountResult {
            count: 0,
            size: if self.file_size { Some(0) } else { None },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CountResult {
    count: u64,
    size: Option<u64>,
}

impl CountResult {
    fn add(self, other: CountResult) -> CountResult {
        CountResult {
            count: self.count + other.count,
            size: self.size.and_then(|s1| other.size.map(|s2| s1 + s2)),
        }
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
