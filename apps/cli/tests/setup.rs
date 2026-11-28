#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

use common::{Daemon, nullradix};

fn run(home: &Path, args: &[&str]) -> Output {
    nullradix(home).args(args).output().unwrap()
}

fn run_in(home: &Path, folder: &Path, args: &[&str]) -> Output {
    nullradix(home)
        .args(args)
        .current_dir(folder)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn repository(parent: &Path) -> PathBuf {
    let folder = parent.join("repo");
    fs::create_dir_all(&folder).unwrap();
    let init = Command::new("git")
        .arg("init")
        .arg(&folder)
        .output()
        .unwrap();
    assert!(init.status.success());
    folder
}

#[test]
fn init_sets_up_the_repository_without_a_daemon_and_repeats_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let repo = repository(temp.path());

    let first = run_in(&home, &repo, &["init"]);
    assert!(first.status.success(), "{first:?}");
    let text = stdout(&first);
    assert!(text.contains("nullradix-board.md: written"), "{text}");
    assert!(text.contains("daemon is not reachable"), "{text}");
    assert!(repo.join(".claude/rules/nullradix-docs.md").is_file());
    let settings = fs::read_to_string(repo.join(".claude/settings.local.json")).unwrap();
    assert!(
        settings.contains("nullradix-dev@nullradix-dev-local"),
        "{settings}"
    );
    let exclude = fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("/.claude/rules/nullradix-board.md"));

    let second = run_in(&home, &repo, &["init", "--json"]);
    assert!(second.status.success(), "{second:?}");
    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["rules"][0]["state"], "unchanged");
    assert_eq!(report["pluginEnabled"], "unchanged");
    assert!(report["board"].is_null());
}

#[test]
fn init_outside_a_repository_fails_and_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let plain = temp.path().join("plain");
    fs::create_dir_all(&plain).unwrap();

    let output = run_in(&temp.path().join("home"), &plain, &["init"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not inside a git repository"));
    assert!(!plain.join(".claude").exists());
}

#[test]
fn init_binds_a_board_to_the_repository_and_names_it_afterwards() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let repo = repository(temp.path());
    let _daemon = Daemon::start(&home);
    assert!(
        run(&home, &["project", "create", "ARC", "Arches"])
            .status
            .success()
    );
    assert!(
        run(&home, &["board", "create", "ARC", "Main"])
            .status
            .success()
    );
    let listed = run(&home, &["board", "list", "--json"]);
    let boards: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let id = boards[0]["id"].as_str().unwrap().to_owned();

    let unbound = run_in(&home, &repo, &["init"]);
    assert!(
        stdout(&unbound).contains("no board is bound"),
        "{unbound:?}"
    );

    let bound = run_in(&home, &repo, &["init", "--board", &id]);
    assert!(bound.status.success(), "{bound:?}");
    assert!(
        stdout(&bound).contains(&format!("ARC Main ({id})")),
        "{bound:?}"
    );

    let again = run_in(&home, &repo, &["init"]);
    assert!(
        stdout(&again).contains(&format!("ARC Main ({id})")),
        "{again:?}"
    );
}

#[test]
fn init_checks_the_board_before_it_writes_anything() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let repo = repository(temp.path());
    let unknown = "018f0000-0000-7000-8000-000000000001";

    let offline = run_in(&home, &repo, &["init", "--board", unknown]);
    assert!(!offline.status.success());
    assert!(!repo.join(".claude").exists());

    let _daemon = Daemon::start(&home);
    let missing = run_in(&home, &repo, &["init", "--board", unknown]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no board has the id"));
    assert!(!repo.join(".claude").exists());
}

#[test]
fn install_dry_run_without_a_daemon_names_the_default_folder_and_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");

    let output = run(&home, &["install", "--dry-run"]);

    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("plugin files: would write"), "{text}");
    assert!(text.contains("default: would install"), "{text}");
    let default = home.join("data").join(".claude");
    assert!(text.contains(&default.display().to_string()), "{text}");
    assert!(text.contains("daemon not reachable"), "{text}");
    assert!(
        !home.exists(),
        "a dry run must not create {}",
        home.display()
    );
}

#[test]
fn a_token_cannot_add_an_account_with_a_folder_of_its_own() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let work = temp.path().join("claude-work");
    fs::create_dir_all(&work).unwrap();
    let _daemon = Daemon::start(&home);

    let refused = run(
        &home,
        &["account", "add", "Work", "--dir", work.to_str().unwrap()],
    );

    assert!(!refused.status.success(), "{refused:?}");
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("command-change-needs-person"), "{message}");
}

#[test]
fn install_dry_run_lists_the_folder_of_every_account() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let _daemon = Daemon::start(&home);
    let added = run(&home, &["account", "add", "Work"]);
    assert!(added.status.success(), "{added:?}");

    let output = run(&home, &["install", "--dry-run", "--json"]);

    assert!(output.status.success(), "{output:?}");
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let dirs: Vec<&str> = summary["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["dir"].as_str().unwrap())
        .collect();
    assert_eq!(dirs.len(), 2, "{dirs:?}");
    assert!(dirs.iter().any(|dir| dir.ends_with("work")), "{dirs:?}");
    let default = home.join("data").join(".claude").display().to_string();
    assert!(dirs.contains(&default.as_str()), "{dirs:?} lacks {default}");
    assert!(!home.join("data").join("plugin").exists());
}
