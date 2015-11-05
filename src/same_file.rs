use std::io;
use std::path::Path;

// Below are platform specific functions for testing the equality of two files.
// Namely, we want to know whether two paths points to precisely the same
// underlying file object.
//
// In our particular use case, the paths should only be directories. If we're
// assuming that directories cannot be hard linked, then it seems like equality
// could be determined by canonicalizing both paths.
//
// I'd also note that other popular libraries (Java's NIO and Boost) expose
// a function like `is_same_file` whose implementation is similar. (i.e., check
// dev/inode on Unix and check `nFileIndex{High,Low}` on Windows.) So this may
// be a candidate for extracting into a separate crate.
//
// ---AG

#[cfg(unix)]
pub fn is_same_file<P, Q>(
    p1: P,
    p2: Q,
) -> io::Result<bool>
where P: AsRef<Path>, Q: AsRef<Path> {
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let md1 = try!(fs::metadata(p1));
    let md2 = try!(fs::metadata(p2));
    Ok((md1.dev(), md1.ino()) == (md2.dev(), md2.ino()))
}

#[cfg(windows)]
pub fn is_same_file<P, Q>(
    p1: P,
    p2: Q,
) -> io::Result<bool>
where P: AsRef<Path>, Q: AsRef<Path> {
    use std::ops::{Deref, Drop};
    use std::os::windows::prelude::*;
    use std::ptr;

    use kernel32;
    use winapi::{self, HANDLE};

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { let _ = kernel32::CloseHandle(self.0); }
        }
    }

    impl Deref for Handle {
        type Target = HANDLE;
        fn deref(&self) -> &HANDLE { &self.0 }
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct BY_HANDLE_FILE_INFORMATION {
        dwFileAttributes: winapi::DWORD,
        ftCreationTime: winapi::FILETIME,
        ftLastAccessTime: winapi::FILETIME,
        ftLastWriteTime: winapi::FILETIME,
        dwVolumeSerialNumber: winapi::DWORD,
        nFileSizeHigh: winapi::DWORD,
        nFileSizeLow: winapi::DWORD,
        nNumberOfLinks: winapi::DWORD,
        nFileIndexHigh: winapi::DWORD,
        nFileIndexLow: winapi::DWORD,
    }

    #[allow(non_camel_case_types)]
    type LPBY_HANDLE_FILE_INFORMATION = *mut BY_HANDLE_FILE_INFORMATION;

    fn file_info(h: &Handle) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
        #[link(name = "ws2_32")]
        #[link(name = "userenv")]
        extern "system" {
            fn GetFileInformationByHandle(
                hFile: HANDLE,
                lpFileInformation: LPBY_HANDLE_FILE_INFORMATION,
            ) -> winapi::BOOL;
        }

        unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = ::std::mem::zeroed();
            if GetFileInformationByHandle(**h, &mut info) == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(info)
            }
        }
    }

    fn open_read_attr<P: AsRef<Path>>(p: P) -> io::Result<Handle> {
        // According to MSDN, `FILE_FLAG_BACKUP_SEMANTICS`
        // must be set in order to get a handle to a directory:
        // https://msdn.microsoft.com/en-us/library/windows/desktop/aa363858(v=vs.85).aspx
        let h = unsafe {
            kernel32::CreateFileW(
                to_utf16(p.as_ref()).as_ptr(),
                0,
                winapi::FILE_SHARE_READ
                | winapi::FILE_SHARE_WRITE
                | winapi::FILE_SHARE_DELETE,
                ptr::null_mut(),
                winapi::OPEN_EXISTING,
                winapi::FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut())
        };
        if h == winapi::INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Handle(h))
        }
    }

    fn to_utf16(s: &Path) -> Vec<u16> {
        s.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    // For correctness, it is critical that both file handles remain open while
    // their attributes are checked for equality. In particular, the file index
    // numbers are not guaranteed to remain stable over time.
    //
    // See the docs and remarks on MSDN:
    // https://msdn.microsoft.com/en-us/library/windows/desktop/aa363788(v=vs.85).aspx
    //
    // It gets worse. It appears that the index numbers are not always
    // guaranteed to be unqiue. Namely, ReFS uses 128 bit numbers for unique
    // identifiers. This requires a distinct syscall to get `FILE_ID_INFO`
    // documented here:
    // https://msdn.microsoft.com/en-us/library/windows/desktop/hh802691(v=vs.85).aspx
    //
    // It seems straight-forward enough to modify this code to use
    // `FILE_ID_INFO` when available (minimum Windows Server 2012), but I don't
    // have access to such Windows machines.
    //
    // Two notes.
    //
    // 1. Java's NIO uses the approach implemented here and appears to ignore
    //    `FILE_ID_INFO` altogether. So Java's NIO and this code are
    //    susceptible to bugs when running on a file system where
    //    `nFileIndex{Low,High}` are not unique.
    //
    // 2. LLVM has a bug where they fetch the id of a file and continue to use
    //    it even after the handle has been closed, so that uniqueness is no
    //    longer guaranteed (when `nFileIndex{Low,High}` are unique).
    //    bug report: http://lists.llvm.org/pipermail/llvm-bugs/2014-December/037218.html
    //
    // All said and done, checking whether two files are the same on Windows
    // seems quite tricky. Moreover, even if the code is technically incorrect,
    // it seems like the chances of actually observing incorrect behavior are
    // extremely small. Nevertheless, we mitigate this by checking size too.
    //
    // In the case where this code is erroneous, two files will be reported
    // as equivalent when they are in fact distinct. This will cause the loop
    // detection code to report a false positive, which will prevent descending
    // into the offending directory. As far as failure modes goes, this isn't
    // that bad.
    let h1 = try!(open_read_attr(&p1));
    let h2 = try!(open_read_attr(&p2));
    let i1 = try!(file_info(&h1));
    let i2 = try!(file_info(&h2));

    let k1 = (
        i1.dwVolumeSerialNumber,
        i1.nFileIndexHigh, i1.nFileIndexLow,
        i1.nFileSizeHigh, i1.nFileSizeLow,
    );
    let k2 = (
        i2.dwVolumeSerialNumber,
        i2.nFileIndexHigh, i2.nFileIndexLow,
        i2.nFileSizeHigh, i2.nFileSizeLow,
    );
    Ok(k1 == k2)
}
