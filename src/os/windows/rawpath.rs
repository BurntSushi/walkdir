use std::fmt;

#[derive(Clone)]
pub struct RawPathBuf {
    /// Buf always has length at least 1 and always ends with a zero u16.
    /// Buf only ever contains exactly 1 zero u16. (i.e., no interior NULs.)
    buf: Vec<u16>,
}

impl fmt::Debug for RawPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use crate::os::windows::escaped_u16s;

        f.debug_struct("RawPathBuf")
            .field("buf", &escaped_u16s(self.as_code_units()))
            .finish()
    }
}

impl RawPathBuf {
    /// Returns the code units (u16s) of this path without the NUL terminator.
    pub fn as_code_units(&self) -> &[u16] {
        &self.buf[..self.buf.len() - 1]
    }

    unsafe fn drop_nul(&mut self) {
        self.buf.set_len(self.buf.len() - 1);
    }
}
