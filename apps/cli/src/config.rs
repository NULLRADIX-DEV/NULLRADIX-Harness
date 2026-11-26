use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use nullradix_os::{Paths, PathsError};
use serde::Deserialize;

/// Separates development data, port and plugin id from an installed release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Profile {
    Release,
    Dev,
}

impl Profile {
    pub(crate) const fn of_build() -> Self {
        if cfg!(debug_assertions) {
            Self::Dev
        } else {
            Self::Release
        }
    }

    pub(crate) const fn app_name(self) -> &'static str {
        match self {
            Self::Release => "nullradix",
            Self::Dev => "nullradix-dev",
        }
    }

    pub(crate) const fn default_port(self) -> u16 {
        match self {
            Self::Release => 4710,
            Self::Dev => 4711,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    server: ServerSection,
    log: LogSection,
    web: WebSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WebSection {
    dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServerSection {
    bind: Option<IpAddr>,
    port: Option<u16>,
    allowed_hosts: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LogSection {
    filter: Option<String>,
}

/// Values from flags or environment variables; they win over the config file.
#[derive(Debug, Default)]
pub(crate) struct Overrides {
    pub(crate) bind: Option<IpAddr>,
    pub(crate) port: Option<u16>,
    pub(crate) log: Option<String>,
    pub(crate) web_dir: Option<PathBuf>,
    pub(crate) theme_catalog_local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) profile: Profile,
    pub(crate) paths: Paths,
    pub(crate) address: SocketAddr,
    pub(crate) allowed_hosts: Vec<String>,
    pub(crate) log_filter: String,
    pub(crate) web_dir: Option<PathBuf>,
    /// Theme catalogs may also be http, local or private; for self-hosted catalogs.
    pub(crate) theme_catalog_local: bool,
    /// A home override keeps secrets and Claude logins away from the real ones of the user.
    pub(crate) isolated: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error(transparent)]
    Paths(#[from] PathsError),
    #[error("cannot read {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid config file {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

impl Config {
    pub(crate) fn load(
        profile: Profile,
        home: Option<&Path>,
        overrides: Overrides,
    ) -> Result<Self, ConfigError> {
        let paths = Paths::resolve(profile.app_name(), home)?;
        let file = read_file(&paths.config.join("config.toml"))?;
        Ok(Self {
            isolated: home.is_some(),
            ..Self::merge(profile, paths, file, overrides, bundled_web_dir())
        })
    }

    fn merge(
        profile: Profile,
        paths: Paths,
        file: FileConfig,
        overrides: Overrides,
        bundled_web: Option<PathBuf>,
    ) -> Self {
        let bind = overrides
            .bind
            .or(file.server.bind)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let port = overrides
            .port
            .or(file.server.port)
            .unwrap_or(profile.default_port());
        let log_filter = overrides
            .log
            .or(file.log.filter)
            .unwrap_or_else(|| String::from("info"));
        let mut allowed_hosts = file.server.allowed_hosts;
        if !bind.is_unspecified() && !bind.is_loopback() {
            allowed_hosts.push(match bind {
                IpAddr::V4(ip) => ip.to_string(),
                IpAddr::V6(ip) => format!("[{ip}]"),
            });
        }
        let web_dir = overrides.web_dir.or(file.web.dir).or(bundled_web);
        Self {
            profile,
            paths,
            address: SocketAddr::new(bind, port),
            allowed_hosts,
            log_filter,
            web_dir,
            theme_catalog_local: overrides.theme_catalog_local,
            isolated: false,
        }
    }
}

/// Web UI shipped next to the executable, as installers lay it out.
fn bundled_web_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("web");
    dir.join("index.html").is_file().then_some(dir)
}

fn read_file(path: &Path) -> Result<FileConfig, ConfigError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(FileConfig::default()),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    toml::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(home: &Path, text: &str) {
        let dir = home.join("config");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    #[test]
    fn uses_profile_defaults_without_a_config_file() {
        let home = tempfile::tempdir().unwrap();
        let release =
            Config::load(Profile::Release, Some(home.path()), Overrides::default()).unwrap();
        let dev = Config::load(Profile::Dev, Some(home.path()), Overrides::default()).unwrap();
        assert_eq!(release.address, "127.0.0.1:4710".parse().unwrap());
        assert_eq!(dev.address, "127.0.0.1:4711".parse().unwrap());
        assert_eq!(release.log_filter, "info");
        assert_eq!(release.paths, Paths::below(home.path()));
    }

    #[test]
    fn config_file_overrides_defaults_and_flags_override_the_file() {
        let home = tempfile::tempdir().unwrap();
        write_config(
            home.path(),
            "[server]\nbind = \"0.0.0.0\"\nport = 9000\n[log]\nfilter = \"debug\"\n",
        );
        let from_file =
            Config::load(Profile::Release, Some(home.path()), Overrides::default()).unwrap();
        assert_eq!(from_file.address, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(from_file.log_filter, "debug");

        let overrides = Overrides {
            bind: None,
            port: Some(9100),
            log: Some(String::from("warn")),
            web_dir: None,
            theme_catalog_local: false,
        };
        let overridden = Config::load(Profile::Release, Some(home.path()), overrides).unwrap();
        assert_eq!(overridden.address, "0.0.0.0:9100".parse().unwrap());
        assert_eq!(overridden.log_filter, "warn");
    }

    #[test]
    fn allows_configured_hosts_and_a_specific_bind_address() {
        let home = tempfile::tempdir().unwrap();
        write_config(
            home.path(),
            "[server]
bind = \"192.168.1.5\"
allowed_hosts = [\"workbench.lan\"]
",
        );
        let config =
            Config::load(Profile::Release, Some(home.path()), Overrides::default()).unwrap();
        assert_eq!(config.allowed_hosts, vec!["workbench.lan", "192.168.1.5"]);
    }

    #[test]
    fn web_dir_comes_from_flags_then_file_then_bundle() {
        let home = tempfile::tempdir().unwrap();
        let bundled = Some(PathBuf::from("bundle/web"));
        let merged = |file: &str, flag: Option<&str>| {
            let file: FileConfig = toml::from_str(file).unwrap();
            let overrides = Overrides {
                web_dir: flag.map(PathBuf::from),
                ..Overrides::default()
            };
            let paths = Paths::below(home.path());
            Config::merge(Profile::Dev, paths, file, overrides, bundled.clone()).web_dir
        };
        assert_eq!(merged("", None), bundled);
        assert_eq!(
            merged(
                "[web]
dir = \"site\"
",
                None
            ),
            Some(PathBuf::from("site"))
        );
        assert_eq!(
            merged(
                "[web]
dir = \"site\"
",
                Some("flag")
            ),
            Some(PathBuf::from("flag"))
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "[server]\nprot = 1\n");
        let error =
            Config::load(Profile::Release, Some(home.path()), Overrides::default()).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }));
    }

    #[test]
    fn build_profile_follows_debug_assertions() {
        let expected = if cfg!(debug_assertions) {
            Profile::Dev
        } else {
            Profile::Release
        };
        assert_eq!(Profile::of_build(), expected);
        assert_eq!(Profile::Dev.app_name(), "nullradix-dev");
    }
}
