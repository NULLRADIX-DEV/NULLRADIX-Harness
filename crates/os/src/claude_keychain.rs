use std::io;
use std::time::Duration;

use crate::command::run;

const SECURITY: &str = "/usr/bin/security";
const LIMIT: Duration = Duration::from_secs(5);

/// The password of a keychain item of Claude Code, which keeps its login there on macOS.
pub fn claude_keychain_password(service: &str) -> io::Result<Option<String>> {
    let args = ["find-generic-password", "-s", service, "-w"].map(str::to_owned);
    let reply = run(SECURITY, &args, None, LIMIT)?;
    let password = reply.stdout.trim();
    Ok((reply.success && !password.is_empty()).then(|| password.to_owned()))
}
