use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// Directories the daemon writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    pub logs: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("no home directory found, set NULLRADIX_HOME")]
pub struct PathsError;

impl Paths {
    /// Platform default directories for `app_name`, or everything below `home` when given.
    pub fn resolve(app_name: &str, home: Option<&Path>) -> Result<Self, PathsError> {
        if let Some(home) = home {
            return Ok(Self::below(home));
        }
        let dirs = ProjectDirs::from("", "", app_name).ok_or(PathsError)?;
        let logs = dirs
            .state_dir()
            .unwrap_or_else(|| dirs.data_local_dir())
            .join("logs");
        Ok(Self {
            data: dirs.data_local_dir().to_path_buf(),
            config: dirs.config_dir().to_path_buf(),
            cache: dirs.cache_dir().to_path_buf(),
            logs,
        })
    }

    #[must_use]
    pub fn below(home: &Path) -> Self {
        Self {
            data: home.join("data"),
            config: home.join("config"),
            cache: home.join("cache"),
            logs: home.join("logs"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_override_puts_everything_below_it() {
        let home = Path::new("/srv/nullradix");
        let paths = Paths::resolve("nullradix", Some(home)).unwrap();
        assert_eq!(paths, Paths::below(home));
        assert_eq!(paths.data, home.join("data"));
        assert_eq!(paths.logs, home.join("logs"));
    }

    #[test]
    fn platform_defaults_are_separated_by_app_name() {
        let release = Paths::resolve("nullradix", None).unwrap();
        let dev = Paths::resolve("nullradix-dev", None).unwrap();
        assert_ne!(release.data, dev.data);
        assert!(release.data.to_string_lossy().contains("nullradix"));
    }
}
