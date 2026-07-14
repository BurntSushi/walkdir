#[macro_use]
mod util;

#[cfg(target_os = "linux")]
mod linux;
mod recursive;
#[cfg(walkdir_unix)]
mod unix;
