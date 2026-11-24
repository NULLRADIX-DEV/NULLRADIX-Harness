use std::path::Path;
use std::process::ExitCode;

use libtest_mimic::{Arguments, Failed, Trial};
use nullradix_acceptance::{Binaries, check, scenarios};

fn main() -> ExitCode {
    let arguments = Arguments::from_args();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let trials = match scenarios(&root) {
        Ok(found) => found
            .into_iter()
            .map(|(name, path)| {
                Trial::test(name, move || {
                    let binaries =
                        Binaries::locate().map_err(|error| Failed::from(format!("{error:#}")))?;
                    check(&path, &binaries).map_err(|error| Failed::from(format!("{error:#}")))
                })
            })
            .collect(),
        Err(error) => vec![Trial::test("scenarios", move || {
            Err(Failed::from(format!("{error:#}")))
        })],
    };
    libtest_mimic::run(&arguments, trials).exit_code()
}
