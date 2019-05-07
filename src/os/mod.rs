/*!
Low level platform specific APIs for reading directory entries.
*/

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;
