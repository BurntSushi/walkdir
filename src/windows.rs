extern crate winapi;
use std::io::Error;
use std::path::PathBuf;
use std::mem;
use self::winapi::um::winnt::HANDLE;
use self::winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;
pub use self::winapi::um::fileapi::BY_HANDLE_FILE_INFORMATION;

/// uses winapi to get Windows file metadata
pub fn windows_file_handle_info(pbuf: &PathBuf) -> Result<BY_HANDLE_FILE_INFORMATION, Error> {

    extern "system" {
        fn GetFileInformationByHandle(a: HANDLE, b: *mut BY_HANDLE_FILE_INFORMATION) -> i32;
    }

    use std::fs::OpenOptions;
    use std::os::windows::prelude::*;

    // The FILE_FLAG_BACKUP_SEMANTICS flag is needed to open directories
    // https://msdn.microsoft.com/en-us/library/windows/desktop/aa365258(v=vs.85).aspx
    let opened_file = OpenOptions::new()
        .create(false)
        .write(false)
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(pbuf.as_path())?;

    unsafe {
        let mut ainfo = mem::zeroed();

        let return_code = GetFileInformationByHandle(opened_file.as_raw_handle(), &mut ainfo);
        // 0 is an error
        match return_code {
            0_i32 => Err(Error::last_os_error()),
            _ => Ok(ainfo),
        }
    }
}
