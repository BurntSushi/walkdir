#[macro_use]
mod util;

mod recursive;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
