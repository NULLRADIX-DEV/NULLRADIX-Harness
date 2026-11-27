use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

use anyhow::Result;

use crate::codegen::{self, OPENAPI_PATH};
use crate::db;

const SCHEMA_PATH: &str = "web/src/shared/api/schema.ts";
const LINUX: &str = "x86_64-unknown-linux-gnu";
// same scope as the coverage gate in CI
const COVERAGE_IGNORE: &str = r"(apps|tests|xtask|crates[/\\](api|client|os|runner|store))[/\\]";

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
    // the build settings this repository uses locally, so the gate reuses that cache
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

fn committed(root: &Path, path: &str) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(["show", &format!("HEAD:{path}")])
        .current_dir(root)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Regenerates the client and compares it with the last commit, as CI does on a clean checkout.
fn codegen_drift(root: &Path) -> Outcome {
    if let Err(error) = codegen::codegen(root) {
        return Outcome::Failed(format!("{error:#}"));
    }
    let stale: Vec<&str> = [OPENAPI_PATH, SCHEMA_PATH]
        .into_iter()
        .filter(|path| fs::read(root.join(path)).ok() != committed(root, path))
        .collect();
    if stale.is_empty() {
        Outcome::Passed
    } else {
        Outcome::Failed(format!(
            "{} differ from the last commit; review and commit them",
            stale.join(", ")
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
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .current_dir(root)
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(LINUX));
    if !installed {
        return Outcome::Skipped(format!("run `rustup target add {LINUX}`"));
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
        // CI always runs it, so a missing tool is no pass
        return Outcome::Failed("cargo-deny is not installed".to_owned());
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

/// Runs the tests under coverage like CI, or plainly when cargo-llvm-cov is missing.
fn tests(root: &Path, reports: &mut Vec<Report>) {
    let covered = present("cargo", &["llvm-cov", "--version"]);
    step("tests", reports, || {
        if covered {
            run(
                root,
                "cargo",
                &[
                    "llvm-cov",
                    "nextest",
                    "--workspace",
                    "--locked",
                    "--no-fail-fast",
                ],
            )
        } else {
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
        }
    });
    step("coverage", reports, || {
        if !covered {
            return Outcome::Failed("cargo-llvm-cov is not installed".to_owned());
        }
        run(
            root,
            "cargo",
            &[
                "llvm-cov",
                "report",
                "--summary-only",
                "--fail-under-lines",
                "80",
                "--ignore-filename-regex",
                COVERAGE_IGNORE,
            ],
        )
    });
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

/// Every step must see what CI sees, so uncommitted work fails the gate.
fn clean_tree(root: &Path) -> Outcome {
    match Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
    {
        Ok(output) if output.status.success() && output.stdout.is_empty() => Outcome::Passed,
        Ok(output) if output.status.success() => Outcome::Failed(
            "uncommitted changes; commit them so the gate checks what CI checks".to_owned(),
        ),
        Ok(output) => Outcome::Failed(format!("git status exited with {}", output.status)),
        Err(error) => Outcome::Failed(format!("could not start git: {error}")),
    }
}

/// Runs every check CI runs, keeps going after a failure and prints one summary.
pub(crate) fn gate(root: &Path, skip_web: bool) -> ExitCode {
    let mut reports = Vec::new();
    let npm = npm();
    step("committed", &mut reports, || clean_tree(root));
    step("rustfmt", &mut reports, || {
        run(root, "cargo", &["fmt", "--all", "--check"])
    });
    step("repository checks", &mut reports, || {
        from_code(crate::check(root))
    });
    step("npm ci", &mut reports, || run(root, npm, &["ci"]));
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
    // the desktop shell is its own workspace; CI builds it with --locked
    step("desktop lock", &mut reports, || {
        run(
            root,
            "cargo",
            &[
                "tree",
                "--locked",
                "--depth",
                "0",
                "--manifest-path",
                "apps/desktop/Cargo.toml",
            ],
        )
    });
    tests(root, &mut reports);
    if skip_web {
        for name in ["sdk build", "eslint", "typecheck", "web tests", "web build"] {
            reports.push(Report {
                name,
                outcome: Outcome::Skipped("--skip-web".to_owned()),
                seconds: 0,
            });
        }
    } else {
        // the web app imports the built SDK, so lint, types, tests and build need it
        step("sdk build", &mut reports, || {
            run(root, npm, &["run", "build", "-w", "sdk/theme"])
        });
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
    let mut skipped = 0;
    for report in reports {
        let (label, detail) = match &report.outcome {
            Outcome::Passed => ("passed ", String::new()),
            Outcome::Failed(why) => {
                failed += 1;
                ("FAILED ", format!(": {why}"))
            }
            Outcome::Skipped(why) => {
                skipped += 1;
                ("skipped", format!(": {why}"))
            }
        };
        println!("  {label} {} ({}s){detail}", report.name, report.seconds);
    }
    if failed == 0 && skipped == 0 {
        println!("gate passed");
        ExitCode::SUCCESS
    } else if failed == 0 {
        println!("gate passed with {skipped} skipped step(s)");
        ExitCode::SUCCESS
    } else {
        println!("gate failed: {failed} step(s)");
        ExitCode::FAILURE
    }
}
