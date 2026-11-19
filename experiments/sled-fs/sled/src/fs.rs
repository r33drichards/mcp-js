//! Compile-time filesystem boundary. The default backend is the original std I/O.
//! The experimental backend must only run synchronously inside a Turmoil host.
#[cfg(not(feature = "turmoil-fs"))]
pub(crate) use std::fs::*;
#[cfg(feature = "turmoil-fs")]
pub(crate) use turmoil::fs::shim::std::fs::*;

#[cfg(not(feature = "turmoil-fs"))]
pub(crate) fn exists(path: &std::path::Path) -> bool {
    path.exists()
}

// Preserve sled's Linux writeback behavior exactly for the production backend.
#[cfg(all(target_os = "linux", not(feature = "turmoil-fs")))]
#[allow(unsafe_code)]
pub(crate) fn sync_range(
    file: &File,
    offset: u64,
    len: usize,
) -> std::io::Result<()> {
    use std::convert::TryFrom;
    use std::os::unix::io::AsRawFd;
    let ret = unsafe {
        libc::sync_file_range(
            file.as_raw_fd(),
            i64::try_from(offset).unwrap(),
            i64::try_from(len).unwrap(),
            libc::SYNC_FILE_RANGE_WAIT_BEFORE
                | libc::SYNC_FILE_RANGE_WRITE
                | libc::SYNC_FILE_RANGE_WAIT_AFTER,
        )
    };
    if ret < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOSYS) {
            file.sync_all()
        } else {
            Err(error)
        }
    } else {
        Ok(())
    }
}

// Turmoil has no raw fd / sync_file_range. Model a successful log write as
// a whole-file sync; this is intentionally stronger than Linux writeback.
#[cfg(any(not(target_os = "linux"), feature = "turmoil-fs"))]
pub(crate) fn sync_range(file: &File, _: u64, _: usize) -> std::io::Result<()> {
    file.sync_all()
}
