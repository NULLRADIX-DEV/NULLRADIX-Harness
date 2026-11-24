#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    match nullradix_desktop::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            nullradix_desktop::report_failure(&error);
            ExitCode::FAILURE
        }
    }
}
