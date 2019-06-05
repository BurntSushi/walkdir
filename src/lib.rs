/*!
TODO
*/

// #![deny(missing_docs)]
#![allow(unknown_lints)]
#![allow(warnings)]

#[cfg(test)]
doc_comment::doctest!("../README.md");

pub use crate::dent::DirEntry;
#[cfg(unix)]
pub use crate::dent::DirEntryExt;
pub use crate::error::{Error, Result};
pub use crate::walk::{FilterEntry, IntoIter, WalkDir};

#[cfg(not(windows))]
pub use cursor::*;

#[cfg(not(windows))]
mod cursor;
mod dent;
mod dir;
mod error;
pub mod os;
#[cfg(test)]
mod tests;
mod util;
mod walk;
