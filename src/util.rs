use std::fs;
use std::io;
use std::path::Path;

#[cfg(unix)]
pub fn device_num<P: AsRef<Path>>(path: P) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;

    path.as_ref().metadata().map(|md| md.dev())
}

#[cfg(unix)]
pub fn device_num_from_metadata(md: &fs::Metadata) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;

    Ok(md.dev())
}

#[cfg(windows)]
pub fn device_num<P: AsRef<Path>>(path: P) -> io::Result<u64> {
    use winapi_util::{file, Handle};

    let h = Handle::from_path_any(path)?;
    file::information(h).map(|info| info.volume_serial_number())
}

#[cfg(windows)]
pub fn device_num_from_metadata(md: &fs::Metadata) -> io::Result<u64> {
    use std::os::windows::fs::MetadataExt;

    Ok(md.volume_serial_number().unwrap_or(0))
}

#[cfg(not(any(unix, windows)))]
pub fn device_num<P: AsRef<Path>>(_: P) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Other,
        "walkdir: same_file_system option not supported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn device_num_from_metadata(_: &fs::Metadata) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Other,
        "walkdir: same_file_system option not supported on this platform",
    ))
}
