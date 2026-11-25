use std::io;
use std::path::{Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};

/// Directories the daemon writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    pub logs: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    #[error("no home directory found, set NULLRADIX_HOME")]
    NoHome,
    #[error("cannot resolve {}", path.display())]
    Unresolvable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl Paths {
    /// Absolute platform default directories for `app_name`, or all below `home` when given.
    pub fn resolve(app_name: &str, home: Option<&Path>) -> Result<Self, PathsError> {
        if let Some(home) = home {
            let absolute =
                std::path::absolute(home).map_err(|source| PathsError::Unresolvable {
                    path: home.to_path_buf(),
                    source,
                })?;
            return Ok(Self::below(&absolute));
        }
        let dirs = ProjectDirs::from("", "", app_name).ok_or(PathsError::NoHome)?;
        let home = BaseDirs::new().map(|base| base.home_dir().to_path_buf());
        let logs = logs_dir(
            cfg!(target_os = "macos"),
            home.as_deref(),
            dirs.state_dir(),
            dirs.data_local_dir(),
            app_name,
        );
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

/// macOS keeps logs in `~/Library/Logs`; elsewhere they sit in the state or data directory.
fn logs_dir(
    macos: bool,
    home: Option<&Path>,
    state: Option<&Path>,
    data: &Path,
    app_name: &str,
) -> PathBuf {
    match home {
        Some(home) if macos => home.join("Library").join("Logs").join(app_name),
        _ => state.unwrap_or(data).join("logs"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_override_puts_everything_below_it() {
        let home = std::env::temp_dir().join("nullradix");
        let paths = Paths::resolve("nullradix", Some(&home)).unwrap();
        assert_eq!(paths, Paths::below(&home));
        assert_eq!(paths.data, home.join("data"));
        assert_eq!(paths.logs, home.join("logs"));
    }

    #[test]
    fn a_relative_home_becomes_absolute() {
        let paths = Paths::resolve("nullradix", Some(Path::new("relative home"))).unwrap();
        assert!(paths.data.is_absolute());
        assert!(
            paths
                .data
                .ends_with(Path::new("relative home").join("data"))
        );
    }

    #[test]
    fn macos_logs_go_to_the_library_and_other_systems_keep_them_beside_the_data() {
        let home = Path::new("/Users/ada");
        let data = Path::new("/Users/ada/Library/Application Support/nullradix");
        assert_eq!(
            logs_dir(true, Some(home), None, data, "nullradix"),
            home.join("Library/Logs/nullradix")
        );
        assert_eq!(
            logs_dir(true, None, None, data, "nullradix"),
            data.join("logs")
        );
        let state = Path::new("/home/ada/.local/state/nullradix");
        assert_eq!(
            logs_dir(false, Some(home), Some(state), data, "nullradix"),
            state.join("logs")
        );
        assert_eq!(
            logs_dir(false, Some(home), None, data, "nullradix"),
            data.join("logs")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_defaults_use_application_support_caches_and_logs() {
        let paths = Paths::resolve("nullradix", None).unwrap();
        let home = BaseDirs::new().unwrap().home_dir().to_path_buf();
        assert_eq!(
            paths.data,
            home.join("Library/Application Support/nullradix")
        );
        assert_eq!(paths.cache, home.join("Library/Caches/nullradix"));
        assert_eq!(paths.logs, home.join("Library/Logs/nullradix"));
    }

    #[test]
    fn platform_defaults_are_separated_by_app_name() {
        let release = Paths::resolve("nullradix", None).unwrap();
        let dev = Paths::resolve("nullradix-dev", None).unwrap();
        assert_ne!(release.data, dev.data);
        assert!(release.data.to_string_lossy().contains("nullradix"));
    }
}
