use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use nullradix_client::{Client, ClientError, DiscoveryFile};
use nullradix_os::Paths;

use crate::config::Profile;
use crate::token;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(5);

/// Opens the web UI in the browser with a fresh session.
pub(crate) fn run(profile: Profile, home: Option<&Path>) -> anyhow::Result<ExitCode> {
    let paths = Paths::resolve(profile.app_name(), home)?;
    let Some(discovery) = DiscoveryFile::in_dir(&paths.data).read()? else {
        println!("daemon is not running, start it with `nullradix serve`");
        return Ok(ExitCode::FAILURE);
    };
    let token = token::read(&token::path(&paths))?.ok_or(ClientError::MissingToken)?;
    let client = Client::new(discovery.address, CONNECT_TIMEOUT, TIMEOUT).with_token(token);
    client.handshake()?;
    let handoff = client.create_handoff()?;
    let url = format!("http://{}{}", discovery.address, handoff.path);
    nullradix_os::open_url(&url).context("cannot open the browser")?;
    println!("opened http://{} in the browser", discovery.address);
    Ok(ExitCode::SUCCESS)
}
