use mcp_js_scheduler_core::{Core, Limits};
use std::{
    path::Path,
    time::{Duration, Instant},
};

// Sled can retain its OS file lock briefly in background I/O after the last
// public handle drops. Wait only for that release, never for scheduler time.
// Call only after dropping the previous owner; other errors fail immediately.
pub fn reopen(path: &Path) -> Core {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match Core::open(path, Limits::default()) {
            Ok(core) => return core,
            Err(error) if error.contains("could not acquire lock") && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("reopen failed: {error}"),
        }
    }
}
