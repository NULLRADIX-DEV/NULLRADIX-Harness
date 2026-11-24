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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) profile: Profile,
    pub(crate) paths: Paths,
    pub(crate) address: SocketAddr,
    pub(crate) allowed_hosts: Vec<String>,
    pub(crate) log_filter: String,
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
        Ok(Self::merge(profile, paths, file, overrides))
    }

    fn merge(profile: Profile, paths: Paths, file: FileConfig, overrides: Overrides) -> Self {
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
        Self {
            profile,
            paths,
            address: SocketAddr::new(bind, port),
            allowed_hosts,
            log_filter,
        }
    }
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
