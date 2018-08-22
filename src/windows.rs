use std::fs::OpenOptions;
use std::io::Error;
use std::mem;
use std::os::windows::prelude::*;
use std::path::Path;

use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;

/// uses winapi to get Windows file metadata
pub fn windows_file_handle_info<P: AsRef<Path>>(
    pbuf: P,
) -> Result<BY_HANDLE_FILE_INFORMATION, Error> {
    // The FILE_FLAG_BACKUP_SEMANTICS flag is needed to open directories
    // https://msdn.microsoft.com/en-us/library/windows/desktop/aa365258(v=vs.85).aspx
    let opened_file = OpenOptions::new()
        .create(false)
        .write(false)
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(pbuf.as_ref())?;

    let (ainfo, code) = unsafe {
        let mut ainfo = mem::zeroed();
        (ainfo, GetFileInformationByHandle(opened_file.as_raw_handle(), &mut ainfo))
    };
    if code == 0 {
        Err(Error::last_os_error())
    } else {
        Ok(ainfo)
    }
}
