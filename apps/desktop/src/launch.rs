use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use nullradix_client::{Client, ClientError, Discovery, DiscoveryError, DiscoveryFile, token};
use nullradix_os::{Paths, PathsError, process_alive, spawn_detached};
use url::Url;

const STARTUP: Duration = Duration::from_secs(20);
/// Shutdown waits up to 10 s each for requests, the scheduler, a pass and setups.
const STOP: Duration = Duration::from_secs(60);
const RELEASE_TRIES: u32 = 10;
const RELEASE_PAUSE: Duration = Duration::from_millis(300);
const POLL: Duration = Duration::from_millis(150);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchError {
    #[error(transparent)]
    Paths(#[from] PathsError),
    #[error("cannot find this program: {0}")]
    Exe(io::Error),
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    #[error("cannot create {}: {source}", path.display())]
    DataDir { path: PathBuf, source: io::Error },
    #[error("cannot start the daemon {}: {source}", path.display())]
    Spawn { path: PathBuf, source: io::Error },
    #[error("the daemon stopped right after it started; its log is in {}", .0.display())]
    Stopped(PathBuf),
    #[error("the daemon did not start within {} seconds; its log is in {}", STARTUP.as_secs(), .0.display())]
    Timeout(PathBuf),
    #[error("the daemon did not stop within {} seconds", STOP.as_secs())]
    StopTimeout,
    #[error(
        "the running daemon (pid {pid}) speaks API version {found}, this app needs {expected}; quit that daemon and open NULLRADIX again"
    )]
    Outdated { pid: u32, found: u32, expected: u32 },
    #[error("no local token in {}; the daemon writes it when it starts", .0.display())]
    MissingToken(PathBuf),
    #[error("cannot read the local token: {0}")]
    Token(io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("the daemon sent an unusable sign-in path: {0}")]
    Url(#[from] url::ParseError),
}

/// The data profile: the one this shell was built as, unless asked for another.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Setup {
    pub(crate) profile: &'static str,
    pub(crate) home: Option<PathBuf>,
    pub(crate) daemon: PathBuf,
    /// The web UI this app bundles, passed to the daemon it starts.
    pub(crate) web_dir: Option<PathBuf>,
}

impl Setup {
    /// `--profile` and `--home` win over `NULLRADIX_PROFILE` and `NULLRADIX_HOME`.
    pub(crate) fn from_env() -> Result<Self, LaunchError> {
        let args: Vec<OsString> = env::args_os().skip(1).collect();
        let program = env::current_exe().map_err(LaunchError::Exe)?;
        Ok(Self::resolve(
            &args,
            env::var("NULLRADIX_PROFILE").ok().as_deref(),
            env::var_os("NULLRADIX_HOME"),
            env::var_os("NULLRADIX_BIN"),
            &program,
        ))
    }

    fn resolve(
        args: &[OsString],
        env_profile: Option<&str>,
        env_home: Option<OsString>,
        env_bin: Option<OsString>,
        program: &Path,
    ) -> Self {
        let profile_arg = flag(args, "--profile").map(|value| value.to_string_lossy().into_owned());
        let home = flag(args, "--home")
            .or(env_home)
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        let daemon = env_bin
            .filter(|bin| !bin.is_empty())
            .map_or_else(|| daemon_beside(program), PathBuf::from);
        Self {
            profile: profile(
                profile_arg.as_deref().or(env_profile),
                cfg!(debug_assertions),
            ),
            home,
            daemon,
            web_dir: None,
        }
    }

    /// Looks for the bundled web UI below the resource folder, unless `NULLRADIX_WEB_DIR` is set.
    #[must_use]
    pub(crate) fn with_resources(mut self, resources: Option<&Path>) -> Self {
        self.web_dir = bundled_web(resources, env::var_os("NULLRADIX_WEB_DIR").as_ref());
        self
    }

    pub(crate) fn paths(&self) -> Result<Paths, PathsError> {
        Paths::resolve(app_name(self.profile), self.home.as_deref())
    }

    /// Arguments that select the same data.
    #[must_use]
    pub(crate) fn data_args(&self) -> Vec<OsString> {
        let mut args = vec![OsString::from("--profile"), OsString::from(self.profile)];
        if let Some(home) = &self.home {
            args.push(OsString::from("--home"));
            args.push(home.clone().into_os_string());
        }
        args
    }

    /// Arguments of the daemon this app starts.
    #[must_use]
    pub(crate) fn serve_args(&self) -> Vec<OsString> {
        let mut args = self.data_args();
        args.push(OsString::from("serve"));
        if let Some(dir) = &self.web_dir {
            args.push(OsString::from("--web-dir"));
            args.push(dir.clone().into_os_string());
        }
        args
    }
}

/// `web/` below the resource folder, where bundles put it; an explicit setting of the user wins.
fn bundled_web(resources: Option<&Path>, env_web: Option<&OsString>) -> Option<PathBuf> {
    if env_web.is_some_and(|dir| !dir.is_empty()) {
        return None;
    }
    let dir = resources?.join("web");
    dir.join("index.html").is_file().then_some(dir)
}

/// The value after `name` in `args`, also written as `name=value`.
fn flag(args: &[OsString], name: &str) -> Option<OsString> {
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let text = arg.to_string_lossy();
        if text == name {
            return rest.next().cloned();
        }
        if let Some(value) = text
            .strip_prefix(name)
            .and_then(|tail| tail.strip_prefix('='))
        {
            return Some(OsString::from(value));
        }
    }
    None
}

/// The daemon binary next to this program; a development build also finds the main target.
fn daemon_beside(program: &Path) -> PathBuf {
    let name = format!("nullradix{}", env::consts::EXE_SUFFIX);
    let beside = program.with_file_name(&name);
    if cfg!(debug_assertions) && !beside.is_file() {
        let main_target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(&name);
        if main_target.is_file() {
            return main_target;
        }
    }
    beside
}

/// A client with the local token, after starting a daemon when none runs.
fn signed_client(setup: &Setup) -> Result<(Client, Discovery), LaunchError> {
    let paths = setup.paths()?;
    let discovery = match running(&paths)? {
        Some(discovery) => discovery,
        None => start(setup, &paths)?,
    };
    let client = local_client(&paths, &discovery)?;
    Ok((client, discovery))
}

fn local_client(paths: &Paths, discovery: &Discovery) -> Result<Client, LaunchError> {
    let token_path = token::path(paths);
    let token = token::read(&token_path)
        .map_err(LaunchError::Token)?
        .ok_or(LaunchError::MissingToken(token_path))?;
    Ok(Client::new(discovery.address, CONNECT_TIMEOUT, TIMEOUT).with_token(token))
}

/// The daemon of these paths while its process lives, whatever API version it speaks.
fn alive(paths: &Paths) -> Result<Option<Discovery>, LaunchError> {
    Ok(DiscoveryFile::in_dir(&paths.data)
        .read()?
        .filter(Discovery::process_alive))
}

/// Whether another process holds the program open, so an installer could not replace it.
pub(crate) fn program_in_use(program: &Path) -> bool {
    match fs::OpenOptions::new().write(true).open(program) {
        Ok(_) => false,
        Err(error) => is_busy(&error),
    }
}

/// A running image or a lock refuses write access; other failures say nothing about a holder.
fn is_busy(error: &io::Error) -> bool {
    if cfg!(windows) {
        // sharing violation, lock violation, file with a mapped image
        matches!(error.raw_os_error(), Some(32 | 33 | 1224))
    } else {
        error.kind() == io::ErrorKind::ExecutableFileBusy
    }
}

/// Waits briefly for the last holder of the program to let go; false when one stays.
pub(crate) fn program_released(program: &Path) -> bool {
    for _ in 0..RELEASE_TRIES {
        if !program_in_use(program) {
            return true;
        }
        thread::sleep(RELEASE_PAUSE);
    }
    !program_in_use(program)
}

/// Asks the daemon to exit and waits until its process is gone; no daemon means nothing to do.
pub(crate) fn stop_daemon(setup: &Setup) -> Result<(), LaunchError> {
    let paths = setup.paths()?;
    let Some(discovery) = alive(&paths)? else {
        return Ok(());
    };
    local_client(&paths, &discovery)?.stop_daemon()?;
    let deadline = Instant::now() + STOP;
    while Instant::now() < deadline {
        if !process_alive(discovery.pid) {
            return Ok(());
        }
        thread::sleep(POLL);
    }
    Err(LaunchError::StopTimeout)
}

/// Finds the running daemon or starts one, then asks it for a one-time sign-in URL.
pub(crate) fn sign_in_url(setup: &Setup) -> Result<Url, LaunchError> {
    let (client, discovery) = signed_client(setup)?;
    let handoff = client.create_handoff()?;
    Ok(daemon_origin(&discovery)?.join(&handoff.path)?)
}

/// A one-time code to confirm the signed-in window for command changes.
pub(crate) fn step_up_code(setup: &Setup) -> Result<String, LaunchError> {
    let (client, _) = signed_client(setup)?;
    Ok(client.create_step_up_code()?.code)
}

pub(crate) fn daemon_origin(discovery: &Discovery) -> Result<Url, url::ParseError> {
    Url::parse(&format!("http://{}/", discovery.address))
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
        Err(ClientError::VersionMismatch { found, expected }) => Err(LaunchError::Outdated {
            pid: discovery.pid,
            found,
            expected,
        }),
        Err(_) => Ok(None),
    }
}

/// Starts a detached daemon, which outlives this shell, and waits until it answers.
fn start(setup: &Setup, paths: &Paths) -> Result<Discovery, LaunchError> {
    fs::create_dir_all(&paths.data).map_err(|source| LaunchError::DataDir {
        path: paths.data.clone(),
        source,
    })?;
    let args = setup.serve_args();
    let pid =
        spawn_detached(&setup.daemon, &args, &paths.data).map_err(|source| LaunchError::Spawn {
            path: setup.daemon.clone(),
            source,
        })?;
    let deadline = Instant::now() + STARTUP;
    while Instant::now() < deadline {
        if let Some(discovery) = running(paths)? {
            return Ok(discovery);
        }
        // another daemon of these paths may have won the start, so only give up when none runs
        if !process_alive(pid) && DiscoveryFile::in_dir(&paths.data).read()?.is_none() {
            return Err(LaunchError::Stopped(paths.logs.clone()));
        }
        thread::sleep(POLL);
    }
    Err(LaunchError::Timeout(paths.logs.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &[&str]) -> Vec<OsString> {
        text.iter().map(OsString::from).collect()
    }

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
    fn arguments_win_over_the_environment() {
        let program = Path::new("/opt/nullradix/nullradix-desktop");
        let setup = Setup::resolve(
            &args(&["--hidden", "--profile=release", "--home", "/data/home"]),
            Some("dev"),
            Some(OsString::from("/env/home")),
            Some(OsString::from("/bin/nullradix")),
            program,
        );
        assert_eq!(setup.profile, "release");
        assert_eq!(setup.home, Some(PathBuf::from("/data/home")));
        assert_eq!(setup.daemon, PathBuf::from("/bin/nullradix"));
        let data: Vec<String> = setup
            .data_args()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(data, ["--profile", "release", "--home", "/data/home"]);

        let plain = Setup::resolve(&[], Some("dev"), Some(OsString::new()), None, program);
        assert_eq!((plain.profile, plain.home), ("dev", None));
    }

    #[test]
    fn the_daemon_gets_the_bundled_web_ui() {
        let resources = tempfile::tempdir().unwrap();
        let web = resources.path().join("web");
        fs::create_dir_all(&web).unwrap();
        // a folder without index.html is not a web UI
        assert_eq!(bundled_web(Some(resources.path()), None), None);
        fs::write(web.join("index.html"), "<html></html>").unwrap();
        assert_eq!(bundled_web(Some(resources.path()), None), Some(web.clone()));
        assert_eq!(bundled_web(None, None), None);
        // the user's own setting is left to the daemon
        assert_eq!(
            bundled_web(Some(resources.path()), Some(&OsString::from("/mine"))),
            None
        );
        assert_eq!(
            bundled_web(Some(resources.path()), Some(&OsString::new())),
            Some(web.clone())
        );

        let setup = Setup {
            profile: "release",
            home: None,
            daemon: PathBuf::from("/bin/nullradix"),
            web_dir: Some(web.clone()),
        };
        let args: Vec<OsString> = setup.serve_args();
        assert_eq!(
            args,
            [
                OsString::from("--profile"),
                OsString::from("release"),
                OsString::from("serve"),
                OsString::from("--web-dir"),
                web.into_os_string(),
            ]
        );
        let plain = Setup {
            web_dir: None,
            ..setup
        };
        assert_eq!(plain.serve_args().last(), Some(&OsString::from("serve")));
    }

    #[test]
    fn a_missing_daemon_binary_is_reported_with_its_path() {
        let home = tempfile::tempdir().unwrap();
        let setup = Setup {
            profile: "dev",
            home: Some(home.path().to_path_buf()),
            daemon: home.path().join("no-such-nullradix"),
            web_dir: None,
        };
        let error = sign_in_url(&setup).err().map(|error| error.to_string());
        assert!(
            error
                .as_deref()
                .is_some_and(|text| text.contains("no-such-nullradix")),
            "{error:?}"
        );
    }

    #[test]
    fn stopping_needs_no_daemon() {
        let home = tempfile::tempdir().unwrap();
        let setup = Setup {
            profile: "dev",
            home: Some(home.path().to_path_buf()),
            daemon: home.path().join("nullradix"),
            web_dir: None,
        };
        stop_daemon(&setup).unwrap();
    }

    #[test]
    fn a_program_nobody_holds_is_free() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("nullradix");
        assert!(!program_in_use(&program), "missing");
        fs::write(&program, "x").unwrap();
        assert!(!program_in_use(&program));
        assert!(program_released(&program));
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn a_running_program_is_in_use() {
        let running = env::current_exe().unwrap();
        assert!(program_in_use(&running));
        assert!(!program_released(&running));
    }

    #[cfg(windows)]
    #[test]
    fn a_program_held_without_write_sharing_is_in_use_until_it_is_closed() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("nullradix.exe");
        fs::write(&program, "x").unwrap();
        let holder = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&program)
            .unwrap();
        assert!(program_in_use(&program));
        drop(holder);
        assert!(!program_in_use(&program));
    }

    #[test]
    fn only_busy_errors_mean_a_holder() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(!is_busy(&denied));
        assert!(!is_busy(&io::Error::from(io::ErrorKind::NotFound)));
        if cfg!(windows) {
            assert!(is_busy(&io::Error::from_raw_os_error(32)));
            assert!(is_busy(&io::Error::from_raw_os_error(1224)));
        } else {
            assert!(is_busy(&io::Error::from(io::ErrorKind::ExecutableFileBusy)));
        }
    }

    #[test]
    fn a_dead_daemon_in_the_discovery_file_counts_as_none() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::below(home.path());
        fs::create_dir_all(&paths.data).unwrap();
        let record = Discovery {
            pid: u32::MAX - 1,
            address: std::net::SocketAddr::from(([127, 0, 0, 1], 9)),
            version: String::from("0.1.0"),
            api_version: 1,
            started_at: 0,
        };
        DiscoveryFile::in_dir(&paths.data).write(&record).unwrap();
        assert!(alive(&paths).unwrap().is_none());
        let live = Discovery {
            pid: std::process::id(),
            ..record
        };
        DiscoveryFile::in_dir(&paths.data).write(&live).unwrap();
        assert_eq!(
            alive(&paths).unwrap().map(|found| found.pid),
            Some(live.pid)
        );
    }

    #[test]
    fn a_stale_discovery_file_means_no_daemon() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::below(home.path());
        fs::create_dir_all(&paths.data).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        for pid in [u32::MAX - 1, std::process::id()] {
            let record = Discovery {
                pid,
                address,
                version: String::from("0.1.0"),
                api_version: 1,
                started_at: 0,
            };
            DiscoveryFile::in_dir(&paths.data).write(&record).unwrap();
            assert!(running(&paths).unwrap().is_none(), "pid {pid}");
        }
    }
}
