use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

use anyhow::Result;

use crate::codegen::{self, OPENAPI_PATH};
use crate::db;

const SCHEMA_PATH: &str = "web/src/shared/api/schema.ts";
const LINUX: &str = "x86_64-unknown-linux-gnu";

enum Outcome {
    Passed,
    Failed(String),
    Skipped(String),
}

struct Report {
    name: &'static str,
    outcome: Outcome,
    seconds: u64,
}

fn npm() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

fn present(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn run(root: &Path, program: &str, args: &[&str]) -> Outcome {
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    // keeps local builds small and off the incremental cache, like CI
    for (key, value) in [("CARGO_INCREMENTAL", "0"), ("CARGO_PROFILE_DEV_DEBUG", "0")] {
        if std::env::var_os(key).is_none() {
            command.env(key, value);
        }
    }
    match command.status() {
        Ok(status) if status.success() => Outcome::Passed,
        Ok(status) => Outcome::Failed(format!("exited with {status}")),
        Err(error) => Outcome::Failed(format!("could not start {program}: {error}")),
    }
}

fn from_code(result: Result<ExitCode>) -> Outcome {
    match result {
        Ok(code) if code == ExitCode::SUCCESS => Outcome::Passed,
        Ok(_) => Outcome::Failed("reported problems".to_owned()),
        Err(error) => Outcome::Failed(format!("{error:#}")),
    }
}

fn generated(root: &Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        fs::read(root.join(OPENAPI_PATH)).ok(),
        fs::read(root.join(SCHEMA_PATH)).ok(),
    )
}

fn codegen_drift(root: &Path) -> Outcome {
    let before = generated(root);
    if let Err(error) = codegen::codegen(root) {
        return Outcome::Failed(format!("{error:#}"));
    }
    if generated(root) == before {
        Outcome::Passed
    } else {
        Outcome::Failed(format!(
            "{OPENAPI_PATH} or {SCHEMA_PATH} changed; review and commit them"
        ))
    }
}

fn linux_clippy(root: &Path) -> Outcome {
    if !present("cargo-zigbuild", &["--version"]) {
        return Outcome::Skipped("cargo-zigbuild is not installed".to_owned());
    }
    if !present("zig", &["version"]) {
        return Outcome::Skipped("zig is not installed".to_owned());
    }
    run(
        root,
        "cargo-zigbuild",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--target",
            LINUX,
            "--",
            "-D",
            "warnings",
        ],
    )
}

fn deny(root: &Path) -> Outcome {
    if !present("cargo", &["deny", "--version"]) {
        return Outcome::Skipped("cargo-deny is not installed".to_owned());
    }
    run(
        root,
        "cargo",
        &[
            "deny",
            "--all-features",
            "--config",
            ".config/deny.toml",
            "check",
        ],
    )
}

fn step(name: &'static str, reports: &mut Vec<Report>, work: impl FnOnce() -> Outcome) {
    println!("\n==> gate: {name}");
    let started = Instant::now();
    let outcome = work();
    reports.push(Report {
        name,
        outcome,
        seconds: started.elapsed().as_secs(),
    });
}

/// Runs every check CI runs, keeps going after a failure and prints one summary.
pub(crate) fn gate(root: &Path, skip_web: bool) -> ExitCode {
    let mut reports = Vec::new();
    let npm = npm();
    step("rustfmt", &mut reports, || {
        run(root, "cargo", &["fmt", "--all", "--check"])
    });
    step("repository checks", &mut reports, || {
        from_code(crate::check(root))
    });
    step("prettier", &mut reports, || {
        run(root, npm, &["run", "format:check"])
    });
    step("clippy", &mut reports, || {
        run(
            root,
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        )
    });
    step("clippy linux", &mut reports, || linux_clippy(root));
    step("query cache", &mut reports, || from_code(db::check(root)));
    step("codegen", &mut reports, || codegen_drift(root));
    step("deny", &mut reports, || deny(root));
    step("tests", &mut reports, || {
        run(
            root,
            "cargo",
            &[
                "nextest",
                "run",
                "--workspace",
                "--locked",
                "--no-fail-fast",
            ],
        )
    });
    if skip_web {
        for name in ["eslint", "typecheck", "web tests", "web build"] {
            reports.push(Report {
                name,
                outcome: Outcome::Skipped("--skip-web".to_owned()),
                seconds: 0,
            });
        }
    } else {
        step("eslint", &mut reports, || run(root, npm, &["run", "lint"]));
        step("typecheck", &mut reports, || {
            run(root, npm, &["run", "typecheck"])
        });
        step("web tests", &mut reports, || run(root, npm, &["test"]));
        step("web build", &mut reports, || {
            run(root, npm, &["run", "build"])
        });
    }
    summarize(&reports)
}

fn summarize(reports: &[Report]) -> ExitCode {
    println!("\ngate summary");
    let mut failed = 0;
    for report in reports {
        let (label, detail) = match &report.outcome {
            Outcome::Passed => ("passed ", String::new()),
            Outcome::Failed(why) => {
                failed += 1;
                ("FAILED ", format!(": {why}"))
            }
            Outcome::Skipped(why) => ("skipped", format!(": {why}")),
        };
        println!("  {label} {} ({}s){detail}", report.name, report.seconds);
    }
    if failed == 0 {
        println!("gate passed");
        ExitCode::SUCCESS
    } else {
        println!("gate failed: {failed} step(s)");
        ExitCode::FAILURE
    }
}
