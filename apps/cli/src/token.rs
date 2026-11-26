use std::fs;
use std::io;
use std::path::Path;

use nullradix_app::auth::Secret;
pub(crate) use nullradix_client::token::{path, read};

/// Writes the token so only the current user can read it: mode 0600, or an owner-only ACL.
pub(crate) fn write(path: &Path, secret: &Secret) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        private_dir(dir)?;
    }
    nullradix_os::write_private(path, secret.expose().as_bytes())
}

fn private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_ignores_damaged_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("cli-token");
        assert_eq!(read(&path).unwrap(), None);

        let secret = Secret::parse(&"d".repeat(64)).unwrap();
        write(&path, &secret).unwrap();
        assert_eq!(read(&path).unwrap(), Some(secret));

        fs::write(&path, "garbage").unwrap();
        assert_eq!(read(&path).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_can_read_the_token() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli-token");
        write(&path, &Secret::parse(&"e".repeat(64)).unwrap()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
