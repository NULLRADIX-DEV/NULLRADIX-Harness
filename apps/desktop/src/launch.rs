use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use nullradix_client::{Client, ClientError, Discovery, DiscoveryError, DiscoveryFile, token};
use nullradix_os::{Paths, PathsError, spawn_detached};
use url::Url;

const STARTUP: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(150);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchError {
    #[error(transparent)]
    Paths(#[from] PathsError),
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    #[error("cannot start the daemon {}: {source}", path.display())]
    Spawn { path: PathBuf, source: io::Error },
    #[error("the daemon did not start within {} seconds", STARTUP.as_secs())]
    Timeout,
    #[error("no local token in {}; the daemon writes it when it starts", .0.display())]
    MissingToken(PathBuf),
    #[error("cannot read the local token: {0}")]
    Token(#[from] io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("the daemon sent an unusable sign-in path: {0}")]
    Url(#[from] url::ParseError),
}

/// The data profile: the one this shell was built as, unless `NULLRADIX_PROFILE` names another.
#[must_use]
pub(crate) fn profile(requested: Option<&str>, debug_build: bool) -> &'static str {
    match requested {
        Some("release") => "release",
        Some("dev") => "dev",
        _ if debug_build => "dev",
        _ => "release",
    }
}

#[must_use]
pub(crate) const fn app_name(profile: &str) -> &'static str {
    if matches!(profile.as_bytes(), b"release") {
        "nullradix"
    } else {
        "nullradix-dev"
    }
}

/// The settings this process was started with.
#[derive(Debug, Clone)]
pub(crate) struct Setup {
    pub(crate) profile: &'static str,
    pub(crate) home: Option<PathBuf>,
    pub(crate) daemon: PathBuf,
}

impl Setup {
    /// `NULLRADIX_BIN` names the daemon binary; otherwise it sits next to this program.
    pub(crate) fn from_env() -> io::Result<Self> {
        let requested = env::var("NULLRADIX_PROFILE").ok();
        let home = env::var_os("NULLRADIX_HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        let daemon = match env::var_os("NULLRADIX_BIN").filter(|bin| !bin.is_empty()) {
            Some(bin) => PathBuf::from(bin),
            None => beside(&env::current_exe()?),
        };
        Ok(Self {
            profile: profile(requested.as_deref(), cfg!(debug_assertions)),
            home,
            daemon,
        })
    }

    pub(crate) fn paths(&self) -> Result<Paths, PathsError> {
        Paths::resolve(app_name(self.profile), self.home.as_deref())
    }

    /// Arguments that make the daemon use the same data as this shell.
    #[must_use]
    pub(crate) fn serve_args(&self) -> Vec<OsString> {
        let mut args = vec![OsString::from("--profile"), OsString::from(self.profile)];
        if let Some(home) = &self.home {
            args.push(OsString::from("--home"));
            args.push(home.clone().into_os_string());
        }
        args.push(OsString::from("serve"));
        args
    }
}

#[must_use]
pub(crate) fn beside(program: &Path) -> PathBuf {
    program.with_file_name(format!("nullradix{}", env::consts::EXE_SUFFIX))
}

/// Finds the running daemon or starts one, then asks it for a one-time sign-in URL.
pub(crate) fn sign_in_url(setup: &Setup) -> Result<Url, LaunchError> {
    let paths = setup.paths()?;
    let discovery = match running(&paths)? {
        Some(discovery) => discovery,
        None => start(setup, &paths)?,
    };
    let token_path = token::path(&paths);
    let token = token::read(&token_path)?.ok_or(LaunchError::MissingToken(token_path))?;
    let client = Client::new(discovery.address, CONNECT_TIMEOUT, TIMEOUT).with_token(token);
    let handoff = client.create_handoff()?;
    Ok(Url::parse(&format!("http://{}", discovery.address))?.join(&handoff.path)?)
}

/// The daemon of these paths when it runs and speaks our API version.
fn running(paths: &Paths) -> Result<Option<Discovery>, LaunchError> {
    let Some(discovery) = DiscoveryFile::in_dir(&paths.data).read()? else {
        return Ok(None);
    };
    if !discovery.process_alive() {
        return Ok(None);
    }
    let client = Client::new(discovery.address, CONNECT_TIMEOUT, TIMEOUT);
    match client.handshake() {
        Ok(_) => Ok(Some(discovery)),
        Err(ClientError::VersionMismatch { found, expected }) => {
            Err(ClientError::VersionMismatch { found, expected }.into())
        }
        Err(_) => Ok(None),
    }
}

/// Starts a detached daemon, which outlives this shell, and waits until it answers.
fn start(setup: &Setup, paths: &Paths) -> Result<Discovery, LaunchError> {
    fs::create_dir_all(&paths.data)?;
    spawn_detached(&setup.daemon, &setup.serve_args(), &paths.data).map_err(|source| {
        LaunchError::Spawn {
            path: setup.daemon.clone(),
            source,
        }
    })?;
    let deadline = Instant::now() + STARTUP;
    while Instant::now() < deadline {
        if let Some(discovery) = running(paths)? {
            return Ok(discovery);
        }
        thread::sleep(POLL);
    }
    Err(LaunchError::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_follows_the_request_or_the_build() {
        assert_eq!(profile(Some("release"), true), "release");
        assert_eq!(profile(Some("dev"), false), "dev");
        assert_eq!(profile(Some("other"), true), "dev");
        assert_eq!(profile(None, false), "release");
        assert_eq!(app_name("release"), "nullradix");
        assert_eq!(app_name("dev"), "nullradix-dev");
    }

    #[test]
    fn the_daemon_shares_profile_and_home_with_the_shell() {
        let setup = Setup {
            profile: "dev",
            home: Some(PathBuf::from("/data/home")),
            daemon: beside(Path::new("/opt/nullradix/nullradix-desktop")),
        };
        let args: Vec<String> = setup
            .serve_args()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--profile", "dev", "--home", "/data/home", "serve"]);
        assert_eq!(
            setup.daemon,
            Path::new("/opt/nullradix").join(format!("nullradix{}", env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn a_missing_daemon_binary_is_reported_with_its_path() {
        let home = tempfile::tempdir().unwrap();
        let setup = Setup {
            profile: "dev",
            home: Some(home.path().to_path_buf()),
            daemon: home.path().join("no-such-nullradix"),
        };
        let error = sign_in_url(&setup).err().map(|error| error.to_string());
        assert!(
            error
                .as_deref()
                .is_some_and(|text| text.contains("no-such-nullradix")),
            "{error:?}"
        );
    }
}
