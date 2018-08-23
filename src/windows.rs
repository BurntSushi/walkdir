use std::fs::OpenOptions;
use std::io::Error;
use std::mem;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use winapi::um::fileapi::{
    BY_HANDLE_FILE_INFORMATION,
    GetFileInformationByHandle,
};
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;

/// Return metadata for the file at the given path.
pub fn windows_file_handle_info<P: AsRef<Path>>(
    path: P,
) -> Result<BY_HANDLE_FILE_INFORMATION, Error> {
    // The FILE_FLAG_BACKUP_SEMANTICS flag is needed to open directories
    // https://msdn.microsoft.com/en-us/library/windows/desktop/aa365258(v=vs.85).aspx
    let file = OpenOptions::new()
        .create(false)
        .write(false)
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;

    unsafe {
        let mut info = mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle(), &mut info) == 0 {
            return Err(Error::last_os_error());
        }
        Ok(info)
    }
}
