//! The local token the daemon writes for clients on the same machine.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nullradix_app::auth::Secret;
use nullradix_os::Paths;

#[must_use]
pub fn path(paths: &Paths) -> PathBuf {
    paths.config.join("cli-token")
}

/// The local token, or `None` when the file is missing or damaged.
pub fn read(path: &Path) -> io::Result<Option<Secret>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Secret::parse(text.trim())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_valid_tokens_and_ignores_damaged_ones() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cli-token");
        assert_eq!(read(&file).unwrap(), None);
        fs::write(&file, format!("{}\n", "d".repeat(64))).unwrap();
        assert_eq!(read(&file).unwrap(), Secret::parse(&"d".repeat(64)));
        fs::write(&file, "garbage").unwrap();
        assert_eq!(read(&file).unwrap(), None);
    }
}
