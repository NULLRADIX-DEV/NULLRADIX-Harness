use std::path::Path;
use std::time::Duration;

use anyhow::bail;
use nullradix_client::{AgentContext, Client, ClientError, DiscoveryFile};
use nullradix_os::Paths;

use crate::config::Profile;
use crate::token;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(30);

/// Client for the running daemon of `profile`, authenticated with the local token.
pub(crate) fn connect(profile: Profile, home: Option<&Path>) -> anyhow::Result<Client> {
    let paths = Paths::resolve(profile.app_name(), home)?;
    let Some(discovery) = DiscoveryFile::in_dir(&paths.data).read()? else {
        bail!("the daemon is not running, start it with `nullradix serve`");
    };
    if !discovery.process_alive() {
        bail!("the daemon is not running, start it with `nullradix serve`");
    }
    let token = token::read(&token::path(&paths))?.ok_or(ClientError::MissingToken)?;
    let client = Client::new(discovery.address, CONNECT_TIMEOUT, TIMEOUT)
        .with_token(token)
        .with_context(AgentContext::from_env());
    client.handshake()?;
    Ok(client)
}
