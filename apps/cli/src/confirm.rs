use std::path::Path;
use std::process::ExitCode;

use clap::Args;

use crate::config::Profile;
use crate::connect::connect;

#[derive(Args)]
pub(crate) struct ConfirmArgs {
    /// Print only the code for typing it into the app.
    #[arg(long)]
    no_open: bool,
    /// Print a one-use code for turning the check off, never as a link.
    #[arg(long, conflicts_with = "no_open")]
    turn_off: bool,
}

/// Prints a code to confirm the open browser session and opens its link.
pub(crate) fn run(
    args: &ConfirmArgs,
    profile: Profile,
    home: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    let client = connect(profile, home)?;
    if args.turn_off {
        let turn_off = client.create_turn_off_code()?;
        println!("turn-off code: {}", turn_off.code);
        println!(
            "it works once and for 2 minutes; enter it where the app asks to turn the check off"
        );
        return Ok(ExitCode::SUCCESS);
    }
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
