use std::path::Path;
use std::process::ExitCode;

use clap::Args;

use crate::config::Profile;
use crate::connect::connect;

#[derive(Args)]
pub(crate) struct ConfirmArgs {
    /// Only print the code, for the box that turns the confirmation off
    #[arg(long)]
    no_open: bool,
}

/// Prints a code that confirms the open browser session for a few minutes, and opens its link.
pub(crate) fn run(
    args: &ConfirmArgs,
    profile: Profile,
    home: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    let client = connect(profile, home)?;
    let confirmation = client.create_step_up_code()?;
    println!("confirmation code: {}", confirmation.code);
    println!("it works once and for 2 minutes; a session it confirms may change commands for 10");
    if args.no_open {
        return Ok(ExitCode::SUCCESS);
    }
    let url = format!("http://{}{}", client.address(), confirmation.path);
    match nullradix_os::open_url(&url) {
        Ok(()) => println!("opened the confirmation in the browser"),
        Err(error) => {
            println!("cannot open the browser: {error}");
            println!("open {url} where NULLRADIX is open, or type the code into the app");
        }
    }
    Ok(ExitCode::SUCCESS)
}
