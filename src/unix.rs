
/// Unix specific extension methods.
pub trait DirEntryExt {

    /// Returns an inode.
    fn ino(&self) -> u64;
}

