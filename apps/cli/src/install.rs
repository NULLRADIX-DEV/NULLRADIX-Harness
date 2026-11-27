use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use clap::Args;
use nullradix_api::AccountDto;
use nullradix_os::Paths;
use serde::Serialize;
use serde_json::Value;

use crate::claude_slots::default_claude_dir;
use crate::config::Profile;
use crate::connect;
use crate::plugin::{self, Staged};

#[derive(Args)]
pub(crate) struct InstallArgs {
    /// Show what would change without changing anything
    #[arg(long)]
    dry_run: bool,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

/// What `claude` answered to one call.
pub(crate) struct Answer {
    pub(crate) ok: bool,
    pub(crate) text: String,
}

/// The command line of Claude Code; a fake stands in for it in tests.
pub(crate) trait ClaudeCli {
    /// Runs `claude <args>` with `CLAUDE_CONFIG_DIR` set to `config_dir` when given.
    fn run(&self, config_dir: Option<&Path>, args: &[&str]) -> Answer;
}

struct SystemClaude;

impl ClaudeCli for SystemClaude {
    fn run(&self, config_dir: Option<&Path>, args: &[&str]) -> Answer {
        // on Windows claude is a batch file that only cmd finds
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.arg("/C").arg("claude").args(args);
            command
        } else {
            let mut command = Command::new("claude");
            command.args(args);
            command
        };
        command.stdin(Stdio::null());
        if let Some(dir) = config_dir {
            command.env("CLAUDE_CONFIG_DIR", dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        match command.output() {
            Ok(output) => Answer {
                ok: output.status.success(),
                text: format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            },
            Err(error) => Answer {
                ok: false,
                text: error.to_string(),
            },
        }
    }
}

/// A Claude folder the plugin goes into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) label: String,
    pub(crate) dir: PathBuf,
}

fn key(dir: &Path) -> String {
    let text = dir.display().to_string();
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text
    }
}

/// The folder of every account, each once; the account without a folder uses the default one.
pub(crate) fn targets(accounts: &[AccountDto], default_dir: Option<&Path>) -> Vec<Target> {
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    for account in accounts {
        let dir = match (&account.config_dir, default_dir) {
            (Some(dir), _) => PathBuf::from(dir),
            (None, Some(default)) => default.to_path_buf(),
            (None, None) => continue,
        };
        if seen.insert(key(&dir)) {
            found.push(Target {
                label: account.name.clone(),
                dir,
            });
        }
    }
    found
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Outcome {
    Installed,
    Updated,
    UpToDate,
    WouldInstall,
    WouldUpdate,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TargetReport {
    pub(crate) name: String,
    pub(crate) dir: String,
    pub(crate) outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) problem: Option<String>,
}

/// Version of the plugin Claude installed in `dir`, empty when the entry names none.
fn installed_version(dir: &Path, id: &str) -> Option<String> {
    let text = fs::read_to_string(dir.join("plugins").join("installed_plugins.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let entries = value.get("plugins")?.get(id)?.as_array()?;
    let entry = entries.first()?;
    Some(
        entry
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

fn failed(target: &Target, problem: String) -> TargetReport {
    report(target, Outcome::Failed, Some(problem))
}

fn report(target: &Target, outcome: Outcome, problem: Option<String>) -> TargetReport {
    TargetReport {
        name: target.label.clone(),
        dir: target.dir.display().to_string(),
        outcome,
        problem,
    }
}

/// What installing into one folder does, or would do with `dry_run`.
pub(crate) fn install_into(
    claude: &dyn ClaudeCli,
    target: &Target,
    profile: Profile,
    marketplace_root: &Path,
    staged: &Staged,
    dry_run: bool,
) -> TargetReport {
    let id = format!("{}@{}", profile.plugin_name(), profile.marketplace_name());
    let installed = installed_version(&target.dir, &id);
    let current = installed.as_deref() == Some(staged.version.as_str());
    if current && !staged.changed() {
        return report(target, Outcome::UpToDate, None);
    }
    if dry_run {
        let outcome = if installed.is_some() {
            Outcome::WouldUpdate
        } else {
            Outcome::WouldInstall
        };
        return report(target, outcome, None);
    }
    let dir = Some(target.dir.as_path());
    let market = profile.marketplace_name();
    let root = marketplace_root.display().to_string();
    let listed = claude.run(dir, &["plugin", "marketplace", "list"]);
    if !listed.text.contains(market) {
        let added = claude.run(dir, &["plugin", "marketplace", "add", &root]);
        if !added.ok {
            return failed(
                target,
                format!("cannot add the marketplace: {}", added.text.trim()),
            );
        }
    } else if !listed.text.contains(&root) {
        return failed(
            target,
            format!(
                "the marketplace {market} points to another folder; remove it with `claude plugin marketplace remove {market}` and run install again"
            ),
        );
    }
    claude.run(dir, &["plugin", "marketplace", "update", market]);
    let fresh = installed.is_none() && !claude.run(dir, &["plugin", "list"]).text.contains(&id);
    let step = if fresh { "install" } else { "update" };
    let done = claude.run(dir, &["plugin", step, &id]);
    if !done.ok {
        return failed(
            target,
            format!("claude plugin {step} failed: {}", done.text.trim()),
        );
    }
    if fresh {
        // init turns the plugin on per project, so it costs nothing elsewhere
        claude.run(dir, &["plugin", "disable", &id, "--scope", "user"]);
        return report(target, Outcome::Installed, None);
    }
    report(target, Outcome::Updated, None)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    dry_run: bool,
    plugin: Staged,
    targets: Vec<TargetReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

fn plan_lines(summary: &Summary) -> Vec<String> {
    let plugin = &summary.plugin;
    let prefix = if summary.dry_run { "would " } else { "" };
    let mut lines = Vec::new();
    if plugin.changed() {
        lines.push(format!(
            "plugin files: {prefix}write {}, remove {}{} (version {})",
            plugin.written,
            plugin.removed,
            if plugin.binary_copied {
                ", copy the binary"
            } else {
                ""
            },
            plugin.version
        ));
    } else {
        lines.push(format!(
            "plugin files up to date (version {})",
            plugin.version
        ));
    }
    for target in &summary.targets {
        let state = match target.outcome {
            Outcome::Installed => "installed",
            Outcome::Updated => "updated",
            Outcome::UpToDate => "up to date",
            Outcome::WouldInstall => "would install",
            Outcome::WouldUpdate => "would update",
            Outcome::Failed => "failed",
        };
        let mut line = format!("{}: {state} ({})", target.name, target.dir);
        if let Some(problem) = &target.problem {
            line = format!("{line} - {problem}");
        }
        lines.push(line);
    }
    if let Some(note) = &summary.note {
        lines.push(note.clone());
    }
    lines
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The folders to install into: every account's through the daemon, else the default one.
fn collect_targets(
    profile: Profile,
    home: Option<&Path>,
) -> anyhow::Result<(Vec<Target>, Option<String>)> {
    let default_dir = default_claude_dir(home);
    match connect::connect(profile, home) {
        Ok(client) => {
            let accounts = client.list_accounts()?.items;
            Ok((targets(&accounts, default_dir.as_deref()), None))
        }
        Err(error) => {
            let Some(dir) = default_dir else {
                bail!("no Claude folder found and no daemon to ask for accounts: {error:#}");
            };
            let note = format!(
                "daemon not reachable ({error:#}); only the default Claude folder was used, run install again with the daemon running to include every account"
            );
            Ok((
                vec![Target {
                    label: String::from("default"),
                    dir,
                }],
                Some(note),
            ))
        }
    }
}

pub(crate) fn run(
    args: &InstallArgs,
    profile: Profile,
    home: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    let paths = Paths::resolve(profile.app_name(), home)?;
    let binary = std::env::current_exe().context("cannot find the running program")?;
    let (targets, note) = collect_targets(profile, home)?;
    let claude = SystemClaude;
    if !args.dry_run && !claude.run(None, &["--version"]).ok {
        let root = plugin::marketplace_dir(&paths.data);
        bail!(
            "Claude Code (the command `claude`) was not found; install it, then run this again, or add the plugin by hand:\n  claude plugin marketplace add \"{}\"\n  claude plugin install {}@{}",
            root.display(),
            profile.plugin_name(),
            profile.marketplace_name()
        );
    }
    let summary = install(
        &claude,
        &targets,
        note,
        profile,
        &paths,
        &binary,
        args.dry_run,
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        for line in plan_lines(&summary) {
            println!("{line}");
        }
    }
    let any_failed = summary
        .targets
        .iter()
        .any(|target| target.outcome == Outcome::Failed);
    Ok(if any_failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn install(
    claude: &dyn ClaudeCli,
    targets: &[Target],
    note: Option<String>,
    profile: Profile,
    paths: &Paths,
    binary: &Path,
    dry_run: bool,
) -> anyhow::Result<Summary> {
    let root = plugin::marketplace_dir(&paths.data);
    let staged = plugin::stage(&root, profile, binary, now_seconds(), dry_run)?;
    let reports = targets
        .iter()
        .map(|target| install_into(claude, target, profile, &root, &staged, dry_run))
        .collect();
    Ok(Summary {
        dry_run,
        plugin: staged,
        targets: reports,
        note,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn account(name: &str, dir: Option<&str>) -> AccountDto {
        serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::now_v7(),
            "slug": name.to_lowercase(),
            "name": name,
            "configDir": dir,
            "enabled": true,
            "fallback": false,
            "limitFiveHour": null,
            "limitWeek": null,
            "hasRunToken": false,
            "pausedUntil": null,
            "pauseReason": null,
            "createdAt": 0,
            "updatedAt": 0,
            "version": 1,
        }))
        .unwrap()
    }

    /// Claude with a state per config folder: the marketplaces and plugins it knows.
    #[derive(Default)]
    struct FakeClaude {
        calls: RefCell<Vec<(String, String)>>,
        marketplace_listing: RefCell<String>,
        broken: bool,
    }

    impl FakeClaude {
        fn calls(&self) -> Vec<String> {
            self.calls
                .borrow()
                .iter()
                .map(|(dir, call)| format!("{dir}|{call}"))
                .collect()
        }
    }

    impl ClaudeCli for FakeClaude {
        fn run(&self, config_dir: Option<&Path>, args: &[&str]) -> Answer {
            let dir = config_dir.map_or_default(|dir| dir.display().to_string());
            let call = args.join(" ");
            self.calls.borrow_mut().push((dir.clone(), call.clone()));
            let mut ok = true;
            let mut text = String::new();
            if call == "plugin marketplace list" {
                text = self.marketplace_listing.borrow().clone();
            } else if let Some(root) = call.strip_prefix("plugin marketplace add ") {
                *self.marketplace_listing.borrow_mut() = format!("nullradix-local {root}");
            } else if call.starts_with("plugin install") {
                ok = !self.broken;
                let dir = config_dir.unwrap();
                fs::create_dir_all(dir.join("plugins")).unwrap();
                fs::write(
                    dir.join("plugins/installed_plugins.json"),
                    r#"{"plugins":{"nullradix@nullradix-local":[{"scope":"user","version":"VERSION"}]}}"#,
                )
                .unwrap();
            }
            Answer { ok, text }
        }
    }

    fn setup() -> (tempfile::TempDir, Paths, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::below(temp.path());
        let binary = temp.path().join("binary");
        fs::write(&binary, b"binary").unwrap();
        (temp, paths, binary)
    }

    fn mark_installed(dir: &Path, version: &str) {
        fs::create_dir_all(dir.join("plugins")).unwrap();
        fs::write(
            dir.join("plugins/installed_plugins.json"),
            format!(
                r#"{{"plugins":{{"nullradix@nullradix-local":[{{"scope":"user","version":"{version}"}}]}}}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn every_account_folder_counts_once_and_the_account_without_one_uses_the_default() {
        let accounts = [
            account("Default", None),
            account("Work", Some("/claude/work")),
            account("Work again", Some("/claude/work")),
            account("Home", Some("/claude/home")),
        ];
        let found = targets(&accounts, Some(Path::new("/home/.claude")));
        let dirs: Vec<String> = found
            .iter()
            .map(|target| target.dir.display().to_string())
            .collect();
        assert_eq!(dirs.len(), 3);
        assert!(dirs[0].ends_with(".claude"));
        assert_eq!(found[1].label, "Work");
        assert_eq!(targets(&accounts[..1], None), Vec::new());
    }

    #[test]
    fn installs_into_each_folder_with_its_own_config_dir_and_stays_idempotent() {
        let (temp, paths, binary) = setup();
        let work = temp.path().join("work");
        let home = temp.path().join("home");
        let targets = [
            Target {
                label: "Work".into(),
                dir: work.clone(),
            },
            Target {
                label: "Home".into(),
                dir: home.clone(),
            },
        ];
        let claude = FakeClaude::default();

        let first = install(
            &claude,
            &targets,
            None,
            Profile::Release,
            &paths,
            &binary,
            false,
        )
        .unwrap();
        assert!(first.plugin.changed());
        assert!(
            first
                .targets
                .iter()
                .all(|target| target.outcome == Outcome::Installed)
        );
        let calls = claude.calls();
        let work_calls: Vec<&String> = calls
            .iter()
            .filter(|call| call.starts_with(&work.display().to_string()))
            .collect();
        assert!(
            work_calls
                .iter()
                .any(|call| call.ends_with("plugin install nullradix@nullradix-local"))
        );
        assert!(work_calls.iter().any(|call| call.ends_with("plugin disable nullradix@nullradix-local --scope user")));
        assert!(
            calls
                .iter()
                .any(|call| call.starts_with(&home.display().to_string()))
        );

        for dir in [&work, &home] {
            mark_installed(dir, &first.plugin.version);
        }
        claude.calls.borrow_mut().clear();
        let second = install(
            &claude,
            &targets,
            None,
            Profile::Release,
            &paths,
            &binary,
            false,
        )
        .unwrap();
        assert!(!second.plugin.changed());
        assert!(
            second
                .targets
                .iter()
                .all(|target| target.outcome == Outcome::UpToDate)
        );
        assert_eq!(claude.calls(), Vec::<String>::new());
    }

    #[test]
    fn a_new_build_updates_a_folder_that_has_the_plugin() {
        let (temp, paths, binary) = setup();
        let dir = temp.path().join("work");
        mark_installed(&dir, "0.0.1");
        let targets = [Target {
            label: "Work".into(),
            dir,
        }];
        let claude = FakeClaude::default();

        let summary = install(
            &claude,
            &targets,
            None,
            Profile::Release,
            &paths,
            &binary,
            false,
        )
        .unwrap();

        assert_eq!(summary.targets[0].outcome, Outcome::Updated);
        let calls = claude.calls();
        assert!(
            calls
                .iter()
                .any(|call| call.ends_with("plugin update nullradix@nullradix-local"))
        );
        assert!(!calls.iter().any(|call| call.contains("plugin disable")));
    }

    #[test]
    fn a_dry_run_names_the_work_and_changes_nothing() {
        let (temp, paths, binary) = setup();
        let installed = temp.path().join("installed");
        mark_installed(&installed, "0.0.1");
        let targets = [
            Target {
                label: "New".into(),
                dir: temp.path().join("new"),
            },
            Target {
                label: "Old".into(),
                dir: installed,
            },
        ];
        let claude = FakeClaude::default();

        let summary = install(
            &claude,
            &targets,
            None,
            Profile::Release,
            &paths,
            &binary,
            true,
        )
        .unwrap();

        assert_eq!(summary.targets[0].outcome, Outcome::WouldInstall);
        assert_eq!(summary.targets[1].outcome, Outcome::WouldUpdate);
        assert_eq!(claude.calls(), Vec::<String>::new());
        assert!(!plugin::marketplace_dir(&paths.data).exists());
        let lines = plan_lines(&summary);
        assert!(
            lines[0].starts_with("plugin files: would write"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_marketplace_of_another_install_is_reported_not_replaced() {
        let (temp, paths, binary) = setup();
        let targets = [Target {
            label: "Work".into(),
            dir: temp.path().join("work"),
        }];
        let claude = FakeClaude::default();
        *claude.marketplace_listing.borrow_mut() = String::from("nullradix-local /old/clone");

        let summary = install(
            &claude,
            &targets,
            None,
            Profile::Release,
            &paths,
            &binary,
            false,
        )
        .unwrap();

        assert_eq!(summary.targets[0].outcome, Outcome::Failed);
        let problem = summary.targets[0].problem.as_deref().unwrap();
        assert!(
            problem.contains("marketplace remove nullradix-local"),
            "{problem}"
        );
        assert!(
            !claude
                .calls()
                .iter()
                .any(|call| call.contains("plugin install"))
        );
    }

    #[test]
    fn a_failing_install_is_reported_for_that_folder_only() {
        let (temp, paths, binary) = setup();
        let targets = [Target {
            label: "Work".into(),
            dir: temp.path().join("work"),
        }];
        let claude = FakeClaude {
            broken: true,
            ..FakeClaude::default()
        };

        let summary = install(
            &claude,
            &targets,
            None,
            Profile::Release,
            &paths,
            &binary,
            false,
        )
        .unwrap();

        assert_eq!(summary.targets[0].outcome, Outcome::Failed);
        assert!(
            summary.targets[0]
                .problem
                .as_deref()
                .unwrap()
                .contains("plugin install failed")
        );
    }
}
