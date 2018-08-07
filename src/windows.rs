use std::io::Error;
use std::mem;
use std::path::Path;

use winapi::um::fileapi::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;

/// uses winapi to get Windows file metadata
pub fn windows_file_handle_info<P: AsRef<Path>>(
    pbuf: P,
) -> Result<BY_HANDLE_FILE_INFORMATION, Error> {
    use std::fs::OpenOptions;
    use std::os::windows::prelude::*;

    // The FILE_FLAG_BACKUP_SEMANTICS flag is needed to open directories
    // https://msdn.microsoft.com/en-us/library/windows/desktop/aa365258(v=vs.85).aspx
    let opened_file = OpenOptions::new()
        .create(false)
        .write(false)
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(pbuf.as_ref())?;

    unsafe {
        let mut ainfo = mem::zeroed();
        let code = GetFileInformationByHandle(opened_file.as_raw_handle(), &mut ainfo);
        // 0 is an error
        if code == 0 {
            Err(Error::last_os_error())
        } else {
            Ok(ainfo)
        }
    }
}
