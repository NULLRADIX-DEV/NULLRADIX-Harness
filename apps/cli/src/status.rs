use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use nullradix_client::{Client, ClientError, DiscoveryFile};
use nullradix_os::Paths;

use crate::config::Profile;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(profile: Profile, home: Option<&Path>) -> anyhow::Result<ExitCode> {
    let paths = Paths::resolve(profile.app_name(), home)?;
    let Some(discovery) = DiscoveryFile::in_dir(&paths.data).read()? else {
        println!("daemon is not running");
        return Ok(ExitCode::FAILURE);
    };
    match Client::new(discovery.address, CONNECT_TIMEOUT, TIMEOUT).handshake() {
        Ok(health) => {
            println!(
                "daemon {} is running at {} (pid {})",
                health.version, discovery.address, discovery.pid
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(ClientError::Unreachable { .. }) => {
            println!("daemon is not running, its discovery file is stale");
            Ok(ExitCode::FAILURE)
        }
        Err(error) => Err(error.into()),
    }
}
