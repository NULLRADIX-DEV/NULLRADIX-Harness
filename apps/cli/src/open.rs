use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;

use crate::config::Profile;
use crate::connect::connect;

/// Opens the web UI in the browser with a fresh session.
pub(crate) fn run(profile: Profile, home: Option<&Path>) -> anyhow::Result<ExitCode> {
    let client = connect(profile, home)?;
    let handoff = client.create_handoff()?;
    let url = format!("http://{}{}", client.address(), handoff.path);
    nullradix_os::open_url(&url).context("cannot open the browser")?;
    println!("opened http://{} in the browser", client.address());
    Ok(ExitCode::SUCCESS)
}
