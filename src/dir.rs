#[cfg(unix)]
use std::ffi::CStr;
use std::io;
#[cfg(unix)]
use std::os::unix::io::RawFd;

#[cfg(target_os = "linux")]
use crate::os::linux;
#[cfg(unix)]
use crate::os::unix;
#[cfg(unix)]
use crate::os::unix::RawPathBuf;

#[derive(Debug)]
pub struct Cursor {
    #[cfg(unix)]
    dir: unix::Dir,
    #[cfg(unix)]
    dent: unix::DirEntry,
    #[cfg(target_os = "linux")]
    linux_cursor: linux::DirEntryCursor,
}

impl Cursor {
    #[cfg(unix)]
    pub fn new(parent: RawFd, dir_name: &CStr) -> io::Result<Cursor> {
        let dir = unix::Dir::openat_c(parent, dir_name)?;
        Ok(Cursor {
            dir,
            #[cfg(unix)]
            dent: unix::DirEntry::empty(),
            #[cfg(target_os = "linux")]
            linux_cursor: linux::DirEntryCursor::new(),
        })
    }

    /// Reset this cursor to the beginning of the given directory.
    ///
    /// An error is returned if the given directory could not be opened for
    /// reading. If an error is returned, the behavior of this cursor is
    /// unspecified until a subsequent and successful `reset` call is made.
    #[cfg(unix)]
    pub fn reset(&mut self, parent: RawFd, dir_name: &CStr) -> io::Result<()> {
        self.dir = unix::Dir::openat_c(parent, dir_name)?;
        Ok(())
    }

    #[cfg(all(unix, walkdir_getdents))]
    pub fn read(&mut self) -> io::Result<Option<CursorEntry>> {
        use std::os::unix::io::AsRawFd;

        let c = &mut self.linux_cursor;
        loop {
            if c.advance() {
                if is_dots(c.current().file_name_bytes()) {
                    continue;
                }
                return Ok(Some(CursorEntry { linux_dent: c.current() }));
            }
            if !linux::getdents(self.dir.as_raw_fd(), c)? {
                return Ok(None);
            }
            // This is guaranteed since getdents returning true means
            // that the buffer has at least one item in it.
            assert!(c.advance());
            if is_dots(c.current().file_name_bytes()) {
                continue;
            }
            return Ok(Some(CursorEntry { linux_dent: c.current() }));
        }
    }

    #[cfg(all(unix, not(walkdir_getdents)))]
    pub fn read(&mut self) -> io::Result<Option<CursorEntry>> {
        loop {
            return if self.dir.read_into(&mut self.dent)? {
                if is_dots(dent.file_name_bytes()) {
                    continue;
                }
                Ok(Some(CursorEntry { cursor: self }))
            } else {
                Ok(None)
            };
        }
    }
}

#[derive(Debug)]
pub struct CursorEntry<'a> {
    #[cfg(not(all(unix, walkdir_getdents)))]
    cursor: &'a Cursor,
    #[cfg(all(unix, walkdir_getdents))]
    linux_dent: linux::DirEntry<'a>,
}

impl<'a> CursorEntry<'a> {}

fn is_dots(file_name: &[u8]) -> bool {
    file_name == b"." || file_name == b".."
}
