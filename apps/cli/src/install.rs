use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use clap::Args;
use nullradix_api::AccountDto;
use nullradix_os::Paths;
use serde::Serialize;
use serde_json::Value;

use crate::claude::{ClaudeCli, SystemClaude};
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

/// A Claude folder the plugin goes into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) label: String,
    pub(crate) dir: PathBuf,
}

fn key(dir: &Path) -> String {
    let text = dir.components().collect::<PathBuf>().display().to_string();
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text
    }
}

/// The folders to install into, and the accounts whose folder cannot be used.
pub(crate) struct Targets {
    pub(crate) found: Vec<Target>,
    pub(crate) rejected: Vec<TargetReport>,
}

/// The folder of every account with `~` expanded, each once; no folder means the default one.
pub(crate) fn targets(
    accounts: &[AccountDto],
    default_dir: Option<&Path>,
    home: Option<&Path>,
) -> Targets {
    let mut seen = BTreeSet::new();
    let mut targets = Targets {
        found: Vec::new(),
        rejected: Vec::new(),
    };
    for account in accounts {
        let dir = match (&account.config_dir, default_dir) {
            (Some(typed), _) => nullradix_runner::expand_home(home, typed),
            (None, Some(default)) => default.to_path_buf(),
            (None, None) => continue,
        };
        if !dir.is_absolute() {
            targets.rejected.push(TargetReport {
                name: account.name.clone(),
                dir: dir.display().to_string(),
                outcome: Outcome::Failed,
                problem: Some(String::from("the Claude folder is not an absolute path")),
            });
        } else if seen.insert(key(&dir)) {
            targets.found.push(Target {
                label: account.name.clone(),
                dir,
            });
        }
    }
    targets
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
    } else if !same_text(&listed.text, &root) {
        return failed(target, other_install(&id, market, &target.dir));
    }
    let refreshed = claude.run(dir, &["plugin", "marketplace", "update", market]);
    if !refreshed.ok {
        return failed(
            target,
            format!("cannot refresh the marketplace: {}", refreshed.text.trim()),
        );
    }
    let fresh = installed.is_none() && !claude.run(dir, &["plugin", "list"]).text.contains(&id);
    let step = if fresh { "install" } else { "update" };
    let done = claude.run(dir, &["plugin", step, &id]);
    if !done.ok {
        return failed(
            target,
            format!("claude plugin {step} failed: {}", done.text.trim()),
        );
    }
    match installed_version(&target.dir, &id) {
        Some(version) if version == staged.version => {}
        Some(version) => {
            return failed(
                target,
                format!(
                    "claude kept version {version} instead of {}; {}",
                    staged.version,
                    other_install(&id, market, &target.dir)
                ),
            );
        }
        None => {
            return failed(
                target,
                format!("claude reported success, but {id} is not installed in this folder"),
            );
        }
    }
    if fresh {
        // init turns the plugin on per project, so it costs nothing elsewhere
        let off = claude.run(dir, &["plugin", "disable", &id, "--scope", "user"]);
        if !off.ok {
            return failed(
                target,
                format!(
                    "installed, but turning it off at user scope failed: {}",
                    off.text.trim()
                ),
            );
        }
        return report(target, Outcome::Installed, None);
    }
    report(target, Outcome::Updated, None)
}

/// Case and separators do not tell two paths apart on the systems that ignore them.
fn same_text(haystack: &str, needle: &str) -> bool {
    let normal = |text: &str| {
        let text = text.replace('\\', "/");
        if cfg!(any(windows, target_os = "macos")) {
            text.to_lowercase()
        } else {
            text
        }
    };
    normal(haystack).contains(&normal(needle))
}

fn other_install(id: &str, market: &str, dir: &Path) -> String {
    format!(
        "another install owns {id}; with CLAUDE_CONFIG_DIR={} run `claude plugin uninstall {id}` and `claude plugin marketplace remove {market}`, then run install again",
        dir.display()
    )
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

/// The daemon's home for `~` and the default folder: its data folder when isolated.
fn claude_base<'a>(home: Option<&Path>, paths: &'a Paths) -> Option<&'a Path> {
    home.map(|_| paths.data.as_path())
}

/// The folders to install into: every account's through the daemon, else the default one.
fn collect_targets(
    profile: Profile,
    home: Option<&Path>,
    paths: &Paths,
) -> anyhow::Result<(Targets, Option<String>)> {
    let base = claude_base(home, paths);
    let default_dir = default_claude_dir(base);
    let home_dir = base.map(Path::to_path_buf).or_else(nullradix_os::home_dir);
    match connect::connect(profile, home) {
        Ok(client) => {
            let accounts = client.list_accounts()?.items;
            Ok((
                targets(&accounts, default_dir.as_deref(), home_dir.as_deref()),
                None,
            ))
        }
        Err(error) => {
            let Some(dir) = default_dir else {
                bail!("no Claude folder found and no daemon to ask for accounts: {error:#}");
            };
            let note = format!(
                "daemon not reachable ({error:#}); only the default Claude folder was used, run install again with the daemon running to include every account"
            );
            let found = vec![Target {
                label: String::from("default"),
                dir,
            }];
            Ok((
                Targets {
                    found,
                    rejected: Vec::new(),
                },
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
    let (targets, note) = collect_targets(profile, home, &paths)?;
    let claude = SystemClaude::find();
    if !args.dry_run && !claude.run(None, &["--version"]).ok {
        let root = plugin::marketplace_dir(&paths.data);
        bail!(
            "Claude Code (the command `claude`) was not found; install it, then run this again, or add the plugin by hand:\n  claude plugin marketplace add \"{}\"\n  claude plugin install {}@{}",
            root.display(),
            profile.plugin_name(),
            profile.marketplace_name()
        );
    }
    let plan = Plan {
        profile,
        paths: &paths,
        binary: &binary,
        home,
        dry_run: args.dry_run,
    };
    let summary = install(&claude, targets, note, &plan)?;
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

/// What one install run works with.
struct Plan<'a> {
    profile: Profile,
    paths: &'a Paths,
    binary: &'a Path,
    home: Option<&'a Path>,
    dry_run: bool,
}

fn install(
    claude: &dyn ClaudeCli,
    targets: Targets,
    note: Option<String>,
    plan: &Plan<'_>,
) -> anyhow::Result<Summary> {
    let Plan {
        profile,
        paths,
        binary,
        home,
        dry_run,
    } = *plan;
    let root = plugin::marketplace_dir(&paths.data);
    let staged = plugin::stage(&root, profile, binary, home, now_seconds(), dry_run)?;
    let mut reports = targets.rejected;
    reports.extend(
        targets
            .found
            .iter()
            .map(|target| install_into(claude, target, profile, &root, &staged, dry_run)),
    );
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
    use crate::claude::Answer;

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

    /// Claude that remembers the marketplace it was given and installs what that serves.
    #[derive(Default)]
    struct FakeClaude {
        calls: RefCell<Vec<(String, Vec<String>)>>,
        root: RefCell<Option<PathBuf>>,
        listing: RefCell<String>,
        /// Version written instead of the served one, as an older install would keep.
        keeps: Option<&'static str>,
        broken: Vec<&'static str>,
    }

    impl FakeClaude {
        fn calls(&self) -> Vec<String> {
            self.calls
                .borrow()
                .iter()
                .map(|(dir, args)| format!("{dir}|{}", args.join(" ")))
                .collect()
        }

        fn served_version(&self) -> String {
            let root = self.root.borrow().clone().unwrap();
            let text = fs::read_to_string(root.join(".claude-plugin/marketplace.json")).unwrap();
            let market: Value = serde_json::from_str(&text).unwrap();
            market["plugins"][0]["version"].as_str().unwrap().to_owned()
        }
    }

    impl ClaudeCli for FakeClaude {
        fn run(&self, config_dir: Option<&Path>, args: &[&str]) -> Answer {
            let dir = config_dir.map_or_default(|dir| dir.display().to_string());
            let owned: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            self.calls.borrow_mut().push((dir, owned));
            let call = args.join(" ");
            let ok = !self.broken.iter().any(|prefix| call.starts_with(prefix));
            let mut text = String::new();
            if call == "plugin marketplace list" {
                text.clone_from(&self.listing.borrow());
            } else if call.starts_with("plugin marketplace add") {
                *self.root.borrow_mut() = Some(PathBuf::from(args[3]));
                *self.listing.borrow_mut() = format!("nullradix-local {}", args[3]);
            } else if ok
                && (call.starts_with("plugin install") || call.starts_with("plugin update"))
            {
                let version = self
                    .keeps
                    .map_or_else(|| self.served_version(), str::to_owned);
                mark_installed(config_dir.unwrap(), &version);
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

    fn target(label: &str, dir: PathBuf) -> Target {
        Target {
            label: label.into(),
            dir,
        }
    }

    fn run_install(
        claude: &FakeClaude,
        targets: Vec<Target>,
        paths: &Paths,
        binary: &Path,
        dry_run: bool,
    ) -> Summary {
        let plan = Plan {
            profile: Profile::Release,
            paths,
            binary,
            home: None,
            dry_run,
        };
        let targets = Targets {
            found: targets,
            rejected: Vec::new(),
        };
        install(claude, targets, None, &plan).unwrap()
    }

    #[test]
    fn every_account_folder_counts_once_after_expanding_home() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("h");
        let at = |name: &str| format!("{}/{name}", home.display());
        let accounts = [
            account("Default", None),
            account("Work", Some(&at("claude/work"))),
            account("Work again", Some(&at("claude/work"))),
            account("Tilde", Some("~/claude/tilde")),
            account("Tilde again", Some(&at("claude/tilde"))),
        ];

        let all = targets(&accounts, Some(&home.join(".claude")), Some(&home));

        let names: Vec<&str> = all.found.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(names, ["Default", "Work", "Tilde"]);
        assert_eq!(all.found[2].dir, home.join("claude/tilde"));
        assert_eq!(all.rejected.len(), 0);
    }

    #[test]
    fn an_isolated_home_means_the_data_folder_for_tilde_and_the_default_folder() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::below(temp.path());

        let base = claude_base(Some(temp.path()), &paths).unwrap();

        assert_eq!(base, temp.path().join("data"));
        assert_eq!(
            default_claude_dir(Some(base)),
            Some(temp.path().join("data").join(".claude"))
        );
        assert_eq!(
            nullradix_runner::expand_home(Some(base), "~/work"),
            temp.path().join("data").join("work")
        );
        assert_eq!(claude_base(None, &paths), None);
    }

    #[test]
    fn a_relative_account_folder_is_reported_not_used() {
        let accounts = [
            account("Odd", Some("relative/dir")),
            account("Default", None),
        ];

        let home = tempfile::tempdir().unwrap();
        let default = home.path().join(".claude");

        let all = targets(&accounts, Some(&default), Some(home.path()));

        assert_eq!(all.found.len(), 1);
        assert_eq!(all.rejected.len(), 1);
        assert_eq!(all.rejected[0].outcome, Outcome::Failed);
        let problem = all.rejected[0].problem.as_deref().unwrap();
        assert!(problem.contains("absolute"));
        assert_eq!(targets(&accounts[..1], None, None).found.len(), 0);
    }

    #[test]
    fn installs_into_each_folder_with_its_own_config_dir_and_stays_idempotent() {
        let (temp, paths, binary) = setup();
        let work = temp.path().join("work");
        let home = temp.path().join("home");
        let targets = vec![target("Work", work.clone()), target("Home", home.clone())];
        let claude = FakeClaude::default();

        let first = run_install(&claude, targets.clone(), &paths, &binary, false);
        assert!(first.plugin.changed());
        assert!(
            first
                .targets
                .iter()
                .all(|t| t.outcome == Outcome::Installed),
            "{first:?}"
        );
        let calls = claude.calls();
        let prefix = work.display().to_string();
        let work_calls: Vec<&String> = calls.iter().filter(|c| c.starts_with(&prefix)).collect();
        assert!(
            work_calls
                .iter()
                .any(|c| c.ends_with("plugin install nullradix@nullradix-local"))
        );
        assert!(
            work_calls
                .iter()
                .any(|c| c.ends_with("plugin disable nullradix@nullradix-local --scope user"))
        );
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with(&home.display().to_string()))
        );

        claude.calls.borrow_mut().clear();
        let second = run_install(&claude, targets, &paths, &binary, false);
        assert!(!second.plugin.changed());
        assert!(
            second
                .targets
                .iter()
                .all(|t| t.outcome == Outcome::UpToDate)
        );
        assert_eq!(claude.calls(), Vec::<String>::new());
    }

    #[test]
    fn a_new_build_updates_a_folder_that_has_the_plugin() {
        let (temp, paths, binary) = setup();
        let dir = temp.path().join("work");
        mark_installed(&dir, "0.0.1");
        let claude = FakeClaude::default();

        let summary = run_install(&claude, vec![target("Work", dir)], &paths, &binary, false);

        assert_eq!(summary.targets[0].outcome, Outcome::Updated, "{summary:?}");
        let calls = claude.calls();
        assert!(
            calls
                .iter()
                .any(|c| c.ends_with("plugin update nullradix@nullradix-local"))
        );
        assert!(!calls.iter().any(|c| c.contains("plugin disable")));
    }

    #[test]
    fn an_older_version_that_stays_is_reported_with_the_remove_commands() {
        let (temp, paths, binary) = setup();
        let dir = temp.path().join("work");
        mark_installed(&dir, "0.3.6");
        let claude = FakeClaude {
            keeps: Some("0.3.6"),
            ..FakeClaude::default()
        };

        let summary = run_install(&claude, vec![target("Work", dir)], &paths, &binary, false);

        assert_eq!(summary.targets[0].outcome, Outcome::Failed);
        let problem = summary.targets[0].problem.as_deref().unwrap();
        assert!(problem.contains("kept version 0.3.6"), "{problem}");
        assert!(
            problem.contains("claude plugin uninstall nullradix@nullradix-local"),
            "{problem}"
        );
        assert!(
            problem.contains("marketplace remove nullradix-local"),
            "{problem}"
        );
    }

    #[test]
    fn a_failing_refresh_or_user_scope_switch_is_reported() {
        let (temp, paths, binary) = setup();
        let refresh = FakeClaude {
            broken: vec!["plugin marketplace update"],
            ..FakeClaude::default()
        };
        let targets = vec![target("Work", temp.path().join("a"))];
        let summary = run_install(&refresh, targets, &paths, &binary, false);
        let problem = summary.targets[0].problem.as_deref().unwrap();
        assert!(
            problem.contains("cannot refresh the marketplace"),
            "{problem}"
        );

        let switch = FakeClaude {
            broken: vec!["plugin disable"],
            ..FakeClaude::default()
        };
        let targets = vec![target("Work", temp.path().join("b"))];
        let summary = run_install(&switch, targets, &paths, &binary, false);
        assert_eq!(summary.targets[0].outcome, Outcome::Failed);
        let problem = summary.targets[0].problem.as_deref().unwrap();
        assert!(problem.contains("user scope"), "{problem}");
    }

    #[test]
    fn a_data_path_with_shell_characters_reaches_claude_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let odd = temp.path().join("a&b %PATH% ^c");
        let paths = Paths::below(&odd);
        let binary = temp.path().join("binary");
        fs::write(&binary, b"binary").unwrap();
        let claude = FakeClaude::default();
        let targets = vec![target("Work", temp.path().join("w"))];

        run_install(&claude, targets, &paths, &binary, false);

        let calls = claude.calls.borrow();
        let added = calls
            .iter()
            .find(|(_, args)| args.get(2).is_some_and(|arg| arg == "add"))
            .unwrap();
        let expected = plugin::marketplace_dir(&paths.data).display().to_string();
        assert_eq!(added.1[3], expected);
        assert!(added.1[3].contains("a&b %PATH% ^c"));
    }

    #[test]
    fn a_dry_run_names_the_work_and_changes_nothing() {
        let (temp, paths, binary) = setup();
        let installed = temp.path().join("installed");
        mark_installed(&installed, "0.0.1");
        let targets = vec![
            target("New", temp.path().join("new")),
            target("Old", installed),
        ];
        let claude = FakeClaude::default();

        let summary = run_install(&claude, targets, &paths, &binary, true);

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
        let claude = FakeClaude::default();
        *claude.listing.borrow_mut() = String::from("nullradix-local /old/clone");
        let targets = vec![target("Work", temp.path().join("work"))];

        let summary = run_install(&claude, targets, &paths, &binary, false);

        assert_eq!(summary.targets[0].outcome, Outcome::Failed);
        let problem = summary.targets[0].problem.as_deref().unwrap();
        assert!(
            problem.contains("marketplace remove nullradix-local"),
            "{problem}"
        );
        assert!(!claude.calls().iter().any(|c| c.contains("plugin install")));
    }

    #[test]
    fn the_listing_matches_the_folder_despite_separators() {
        assert!(same_text("x /data/plugin", "/data/plugin"));
        assert!(same_text("x C:\\Data\\Plugin", "C:/Data/Plugin"));
        assert!(!same_text("x /data/plugins", "/data/other"));
    }

    #[test]
    fn a_failing_install_is_reported_for_that_folder_only() {
        let (temp, paths, binary) = setup();
        let claude = FakeClaude {
            broken: vec!["plugin install"],
            ..FakeClaude::default()
        };
        let targets = vec![target("Work", temp.path().join("work"))];

        let summary = run_install(&claude, targets, &paths, &binary, false);

        assert_eq!(summary.targets[0].outcome, Outcome::Failed);
        let problem = summary.targets[0].problem.as_deref().unwrap();
        assert!(problem.contains("plugin install failed"), "{problem}");
    }
}
