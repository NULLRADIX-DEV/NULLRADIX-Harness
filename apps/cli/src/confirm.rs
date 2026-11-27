use std::path::Path;
use std::process::ExitCode;

use crate::config::Profile;
use crate::connect::connect;

/// Prints a code that confirms the open browser session for a few minutes, and opens its link.
pub(crate) fn run(profile: Profile, home: Option<&Path>) -> anyhow::Result<ExitCode> {
    let client = connect(profile, home)?;
    let confirmation = client.create_step_up_code()?;
    let url = format!("http://{}{}", client.address(), confirmation.path);
    println!("confirmation code: {}", confirmation.code);
    println!("it works once, for 2 minutes, and then lets the session change commands for 10");
    match nullradix_os::open_url(&url) {
        Ok(()) => println!("opened the confirmation in the browser"),
        Err(error) => {
            println!("cannot open the browser: {error}");
            println!("open {url} where NULLRADIX is open, or type the code into the app");
        }
    }
    Ok(ExitCode::SUCCESS)
}
