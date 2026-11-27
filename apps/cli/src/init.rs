use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, bail};
use clap::Args;
use nullradix_api::BoardDto;
use nullradix_client::Client;
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::board::{bind_repository, local_runner};
use crate::config::Profile;
use crate::connect;
use crate::plugin;

const RULE_MARKER: &str = "# Created by `nullradix init`, not versioned.";
const SETTINGS: &str = ".claude/settings.local.json";

#[derive(Args)]
pub(crate) struct InitArgs {
    /// Bind this board to the repository of the current folder
    #[arg(long)]
    board: Option<Uuid>,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

/// A rule file of the project: where it goes, which folders load it, which shipped text it holds.
struct Rule {
    path: &'static str,
    paths: &'static [&'static str],
    shipped: &'static str,
}

const RULES: [Rule; 2] = [
    Rule {
        path: ".claude/rules/nullradix-docs.md",
        paths: &["**/docs/**"],
        shipped: "rules/docs.md",
    },
    Rule {
        path: ".claude/rules/nullradix-board.md",
        paths: &[],
        shipped: "rules/board.md",
    },
];

impl Rule {
    fn content(&self) -> anyhow::Result<String> {
        let body = plugin::shipped(self.shipped).context("the shipped rule is missing")?;
        let mut text = format!("---\n{RULE_MARKER}\n");
        if !self.paths.is_empty() {
            text.push_str("paths:\n");
            for path in self.paths {
                text.push_str(&format!("  - \"{path}\"\n"));
            }
        }
        text.push_str("---\n");
        text.push_str(body);
        Ok(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum State {
    Written,
    Unchanged,
    /// A file NULLRADIX did not write; it is left alone.
    Kept,
}

impl State {
    const fn text(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Unchanged => "unchanged",
            Self::Kept => "kept, it was not written by NULLRADIX",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RuleReport {
    pub(crate) path: String,
    pub(crate) state: State,
}

/// What `init` did to the project folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Applied {
    pub(crate) rules: Vec<RuleReport>,
    pub(crate) plugin_enabled: State,
}

fn read_text(path: &Path) -> anyhow::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn write(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    nullradix_os::write_atomic(path, text.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))
}

fn apply_rule(root: &Path, rule: &Rule) -> anyhow::Result<State> {
    let file = root.join(rule.path);
    let content = rule.content()?;
    match read_text(&file)? {
        Some(old) if old == content => Ok(State::Unchanged),
        Some(old) if !old.lines().take(3).any(|line| line == RULE_MARKER) => Ok(State::Kept),
        _ => {
            write(&file, &content)?;
            Ok(State::Written)
        }
    }
}

/// Keeps the rule files out of Git for every worktree of the repository.
fn exclude_rules(common_dir: &Path) -> anyhow::Result<()> {
    let file = common_dir.join("info").join("exclude");
    let mut text = read_text(&file)?.unwrap_or_default();
    let mut changed = false;
    for rule in &RULES {
        let line = format!("/{}", rule.path);
        if !text.lines().any(|known| known == line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&line);
            text.push('\n');
            changed = true;
        }
    }
    if changed {
        write(&file, &text)?;
    }
    Ok(())
}

/// Turns the plugin on for the project; every other setting stays as it is.
fn enable_plugin(root: &Path, id: &str) -> anyhow::Result<State> {
    let file = root.join(SETTINGS);
    let mut settings = match read_text(&file)? {
        Some(text) if !text.trim().is_empty() => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(settings)) => settings,
            _ => bail!(
                "{} is not valid JSON; fix it and run init again, it was not changed",
                file.display()
            ),
        },
        _ => Map::new(),
    };
    let mut plugins = match settings.remove("enabledPlugins") {
        Some(Value::Object(plugins)) => plugins,
        _ => Map::new(),
    };
    if plugins.get(id) == Some(&Value::Bool(true)) {
        return Ok(State::Unchanged);
    }
    plugins.insert(id.to_owned(), Value::Bool(true));
    settings.insert("enabledPlugins".to_owned(), Value::Object(plugins));
    write(
        &file,
        &(serde_json::to_string_pretty(&Value::Object(settings))? + "\n"),
    )?;
    Ok(State::Written)
}

/// Writes the rules, keeps them out of Git and enables the plugin for the project.
pub(crate) fn apply(root: &Path, common_dir: &Path, plugin_id: &str) -> anyhow::Result<Applied> {
    let mut rules = Vec::new();
    for rule in &RULES {
        rules.push(RuleReport {
            path: rule.path.to_owned(),
            state: apply_rule(root, rule)?,
        });
    }
    exclude_rules(common_dir)?;
    Ok(Applied {
        rules,
        plugin_enabled: enable_plugin(root, plugin_id)?,
    })
}

fn git(folder: &Path, args: &[&str]) -> anyhow::Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(folder)
        .output()
        .context("cannot run git")?;
    if !output.status.success() {
        bail!("{} is not inside a git repository", folder.display());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(PathBuf::from)
        .collect())
}

/// The root of the working tree around `folder` and the common git folder of its repository.
pub(crate) fn repository_of(folder: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let found = git(
        folder,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ],
    )?;
    let [root, common] = <[PathBuf; 2]>::try_from(found)
        .map_err(|_| anyhow::anyhow!("git named no repository root"))?;
    Ok((root, common))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    root: String,
    plugin: String,
    #[serde(flatten)]
    applied: Applied,
    board: Option<BoardLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoardLine {
    id: Uuid,
    project_key: String,
    name: String,
}

impl From<&BoardDto> for BoardLine {
    fn from(board: &BoardDto) -> Self {
        Self {
            id: board.id,
            project_key: board.project_key.clone(),
            name: board.name.clone(),
        }
    }
}

enum Lookup {
    Bound(BoardDto),
    NoBoard,
    Offline(String),
}

/// The board of the repository, bound first when `bind` names one.
fn lookup(
    profile: Profile,
    home: Option<&Path>,
    root: &Path,
    bind: Option<Uuid>,
) -> anyhow::Result<Lookup> {
    let client = match connect::connect(profile, home) {
        Ok(client) => client,
        Err(error) if bind.is_none() => return Ok(Lookup::Offline(format!("{error:#}"))),
        Err(error) => return Err(error),
    };
    find_or_bind(&client, root, bind)
}

fn find_or_bind(client: &Client, root: &Path, bind: Option<Uuid>) -> anyhow::Result<Lookup> {
    let local = local_runner(client)?;
    if let Some(id) = bind {
        let Some(board) = client
            .list_all_boards()?
            .items
            .into_iter()
            .find(|board| board.id == id)
        else {
            bail!("no board has the id {id}");
        };
        return Ok(Lookup::Bound(bind_repository(
            client, &board, &local, root,
        )?));
    }
    let Some(text) = root.to_str() else {
        bail!("{} is not valid UTF-8", root.display());
    };
    let boards = client.list_boards_at(local.id, text)?.items;
    Ok(boards
        .into_iter()
        .min_by_key(|board| (board.created_at, board.id))
        .map_or(Lookup::NoBoard, Lookup::Bound))
}

fn lines(report: &Report, lookup: &Lookup) -> Vec<String> {
    let mut lines = vec![format!("NULLRADIX project: {}", report.root)];
    for rule in &report.applied.rules {
        lines.push(format!("rule {}: {}", rule.path, rule.state.text()));
    }
    lines.push(format!(
        "plugin {} in {SETTINGS}: {}",
        report.plugin,
        match report.applied.plugin_enabled {
            State::Written => "enabled for this project",
            _ => "already enabled",
        }
    ));
    if report.applied.plugin_enabled == State::Written {
        lines.push(String::from(
            "Restart Claude Code or enter /reload-plugins so the hooks run.",
        ));
    }
    match lookup {
        Lookup::Bound(board) => lines.push(format!(
            "board of this repository: {} {} ({})",
            board.project_key, board.name, board.id
        )),
        Lookup::NoBoard => lines.push(String::from(
            "no board is bound to this repository; `nullradix board list` shows the boards, `nullradix init --board <id>` binds one",
        )),
        Lookup::Offline(reason) => lines.push(format!(
            "board not looked up, the daemon is not reachable: {reason}"
        )),
    }
    lines
}

pub(crate) fn run(
    args: &InitArgs,
    profile: Profile,
    home: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    let folder = std::env::current_dir().context("cannot read the current folder")?;
    let (root, common) = repository_of(&folder)?;
    let id = format!("{}@{}", profile.plugin_name(), profile.marketplace_name());
    let applied = apply(&root, &common, &id)?;
    let found = lookup(profile, home, &root, args.board)?;
    let report = Report {
        root: root.display().to_string(),
        plugin: id,
        applied,
        board: match &found {
            Lookup::Bound(board) => Some(BoardLine::from(board)),
            _ => None,
        },
        note: match &found {
            Lookup::Offline(reason) => Some(reason.clone()),
            _ => None,
        },
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for line in lines(&report, &found) {
            println!("{line}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "nullradix@nullradix-local";

    fn project() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let common = root.join(".git");
        fs::create_dir_all(&common).unwrap();
        (temp, root, common)
    }

    #[test]
    fn writes_the_english_rules_and_enables_the_plugin() {
        let (_temp, root, common) = project();

        let applied = apply(&root, &common, ID).unwrap();

        assert!(
            applied
                .rules
                .iter()
                .all(|rule| rule.state == State::Written)
        );
        let board = fs::read_to_string(root.join(".claude/rules/nullradix-board.md")).unwrap();
        assert!(board.starts_with(&format!("---\n{RULE_MARKER}\n---\n")));
        assert!(board.contains("NULLRADIX board"));
        let docs = fs::read_to_string(root.join(".claude/rules/nullradix-docs.md")).unwrap();
        assert!(docs.contains("paths:\n  - \"**/docs/**\"\n---\n"));
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(root.join(SETTINGS)).unwrap()).unwrap();
        assert_eq!(settings["enabledPlugins"][ID], true);
        let exclude = fs::read_to_string(common.join("info/exclude")).unwrap();
        assert!(exclude.contains("/.claude/rules/nullradix-board.md"));
        assert!(exclude.contains("/.claude/rules/nullradix-docs.md"));
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let (_temp, root, common) = project();
        apply(&root, &common, ID).unwrap();
        let before = fs::read_to_string(common.join("info/exclude")).unwrap();

        let again = apply(&root, &common, ID).unwrap();

        assert!(
            again
                .rules
                .iter()
                .all(|rule| rule.state == State::Unchanged)
        );
        assert_eq!(again.plugin_enabled, State::Unchanged);
        assert_eq!(
            fs::read_to_string(common.join("info/exclude")).unwrap(),
            before
        );
    }

    #[test]
    fn an_outdated_rule_is_rewritten_and_a_hand_written_one_is_kept() {
        let (_temp, root, common) = project();
        apply(&root, &common, ID).unwrap();
        let board = root.join(".claude/rules/nullradix-board.md");
        fs::write(&board, format!("---\n{RULE_MARKER}\n---\nold text\n")).unwrap();
        let docs = root.join(".claude/rules/nullradix-docs.md");
        fs::write(&docs, "my own docs rule\n").unwrap();

        let applied = apply(&root, &common, ID).unwrap();

        assert_eq!(applied.rules[0].state, State::Kept);
        assert_eq!(fs::read_to_string(&docs).unwrap(), "my own docs rule\n");
        assert_eq!(applied.rules[1].state, State::Written);
        assert!(
            fs::read_to_string(&board)
                .unwrap()
                .contains("NULLRADIX board")
        );
    }

    #[test]
    fn other_settings_survive_and_broken_settings_are_never_overwritten() {
        let (_temp, root, common) = project();
        let settings = root.join(SETTINGS);
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(
            &settings,
            r#"{"permissions":{"allow":["Bash"]},"enabledPlugins":{"other@market":true}}"#,
        )
        .unwrap();

        apply(&root, &common, ID).unwrap();

        let kept: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(kept["permissions"]["allow"][0], "Bash");
        assert_eq!(kept["enabledPlugins"]["other@market"], true);
        assert_eq!(kept["enabledPlugins"][ID], true);

        fs::write(&settings, "{ not json").unwrap();
        let error = apply(&root, &common, ID).unwrap_err();
        assert!(format!("{error:#}").contains("not valid JSON"));
        assert_eq!(fs::read_to_string(&settings).unwrap(), "{ not json");
    }

    #[test]
    fn the_exclude_file_keeps_its_own_lines() {
        let (_temp, root, common) = project();
        fs::create_dir_all(common.join("info")).unwrap();
        fs::write(common.join("info/exclude"), "/mine").unwrap();

        apply(&root, &common, ID).unwrap();

        let exclude = fs::read_to_string(common.join("info/exclude")).unwrap();
        assert!(exclude.starts_with("/mine\n/.claude/rules/nullradix-docs.md\n"));
    }

    #[test]
    fn finds_the_repository_root_from_a_subfolder() {
        let temp = tempfile::tempdir().unwrap();
        let init = Command::new("git")
            .arg("init")
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(init.status.success());
        let sub = temp.path().join("a").join("b");
        fs::create_dir_all(&sub).unwrap();

        let (root, common) = repository_of(&sub).unwrap();

        assert_eq!(
            fs::canonicalize(root).unwrap(),
            fs::canonicalize(temp.path()).unwrap()
        );
        assert!(common.ends_with(".git"));
        let plain = tempfile::tempdir().unwrap();
        assert!(repository_of(plain.path()).is_err());
    }
}
