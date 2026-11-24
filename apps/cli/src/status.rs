use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use nullradix_client::{Client, ClientError, DiscoveryError, DiscoveryFile};
use nullradix_os::Paths;

use crate::config::Profile;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(profile: Profile, home: Option<&Path>) -> anyhow::Result<ExitCode> {
    let paths = Paths::resolve(profile.app_name(), home)?;
    let discovery = match DiscoveryFile::in_dir(&paths.data).read() {
        Ok(Some(discovery)) => discovery,
        Ok(None) => return Ok(report("daemon is not running")),
        Err(DiscoveryError::Invalid { .. }) => return Ok(stale()),
        Err(error) => return Err(error.into()),
    };
    if !discovery.process_alive() {
        return Ok(stale());
    }
    match Client::new(discovery.address, CONNECT_TIMEOUT, TIMEOUT).handshake() {
        Ok(health) => {
            println!(
                "daemon {} is running at {} (pid {})",
                health.version, discovery.address, discovery.pid
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(ClientError::Unreachable { .. } | ClientError::Protocol(_)) => Ok(stale()),
        Err(error) => Err(error.into()),
    }
}

fn stale() -> ExitCode {
    report("daemon is not running, its discovery file is stale")
}

fn report(message: &str) -> ExitCode {
    println!("{message}");
    ExitCode::FAILURE
}
