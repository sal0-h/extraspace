use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

/// A snapshot contains desktop pixels. Keep it private and refuse to overwrite
/// an existing directory or file, including symlinks.
pub(crate) fn save(directory: &Path, frame: &[u8], graph: &str) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(directory)?;
    for (name, data) in [("frame.h264", frame), ("pipeline.dot", graph.as_bytes())] {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(name))?
            .write_all(data)?;
    }
    Ok(())
}
