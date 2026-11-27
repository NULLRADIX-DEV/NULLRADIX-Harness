use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use async_trait::async_trait;
use nullradix_api::{CustomizationDto, ExtensionPointDto, OverlayStateDto};
use nullradix_app::AppError;
use nullradix_app::customizations::StandardCatalog;
use nullradix_domain::{ExtensionPoint, OverlayName};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Profile;

const FILES: &[(&str, &str)] = &[
    (
        ".claude-plugin/plugin.json",
        include_str!("../../../plugin/.claude-plugin/plugin.json"),
    ),
    (
        "hooks/hooks.json",
        include_str!("../../../plugin/hooks/hooks.json"),
    ),
    (
        "output-styles/software-engineer.md",
        include_str!("../../../plugin/output-styles/software-engineer.md"),
    ),
    (
        "rules/board.md",
        include_str!("../../../plugin/rules/board.md"),
    ),
    (
        "rules/docs.md",
        include_str!("../../../plugin/rules/docs.md"),
    ),
    (
        "skills/board/SKILL.md",
        include_str!("../../../plugin/skills/board/SKILL.md"),
    ),
    (
        "skills/customize/SKILL.md",
        include_str!("../../../plugin/skills/customize/SKILL.md"),
    ),
    (
        "skills/grill/SKILL.md",
        include_str!("../../../plugin/skills/grill/SKILL.md"),
    ),
    (
        "skills/implement/SKILL.md",
        include_str!("../../../plugin/skills/implement/SKILL.md"),
    ),
    (
        "skills/question/SKILL.md",
        include_str!("../../../plugin/skills/question/SKILL.md"),
    ),
    (
        "skills/setup/SKILL.md",
        include_str!("../../../plugin/skills/setup/SKILL.md"),
    ),
    (
        "skills/tasks/SKILL.md",
        include_str!("../../../plugin/skills/tasks/SKILL.md"),
    ),
    (
        "skills/wayfinder/SKILL.md",
        include_str!("../../../plugin/skills/wayfinder/SKILL.md"),
    ),
];

const MANIFEST: &str = ".claude-plugin/plugin.json";
const HOOKS: &str = "hooks/hooks.json";
const STATE_FILE: &str = ".nullradix-stage.json";
const BINARY: &str = if cfg!(windows) {
    "nullradix.exe"
} else {
    "nullradix"
};

/// The shipped text of a plugin file such as `rules/board.md`.
pub(crate) fn shipped(path: &str) -> Option<&'static str> {
    FILES
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, text)| *text)
}

fn skill_file(name: &str) -> String {
    format!("skills/{name}/SKILL.md")
}

fn style_file(name: &str) -> String {
    format!("output-styles/{name}.md")
}

/// The names of the shipped skills or output styles, sorted.
#[cfg(test)]
fn shipped_names(point: ExtensionPoint) -> Vec<String> {
    let (prefix, suffix) = match point {
        ExtensionPoint::Skill => ("skills/", "/SKILL.md"),
        ExtensionPoint::OutputStyle => ("output-styles/", ".md"),
    };
    let mut names: Vec<String> = FILES
        .iter()
        .filter_map(|(path, _)| path.strip_prefix(prefix)?.strip_suffix(suffix))
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

/// The shipped skills and output styles as the files a customization can replace.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ShippedStandards;

#[async_trait]
impl StandardCatalog for ShippedStandards {
    async fn get(
        &self,
        point: ExtensionPoint,
        name: &OverlayName,
    ) -> Result<Option<String>, AppError> {
        let name = name.to_string();
        let path = match point {
            ExtensionPoint::Skill => skill_file(&name),
            ExtensionPoint::OutputStyle => style_file(&name),
        };
        Ok(shipped(&path).map(str::to_owned))
    }
}

/// A customization that goes on top of the shipped files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Custom {
    path: String,
    content: String,
}

impl Custom {
    /// The file of an active customization; one that does not apply gives `None`.
    pub(crate) fn of(line: &CustomizationDto) -> Option<Self> {
        if line.state != OverlayStateDto::Active {
            return None;
        }
        let name = OverlayName::parse(&line.name).ok()?;
        let path = match line.point {
            ExtensionPointDto::Skill => skill_file(name.as_str()),
            ExtensionPointDto::OutputStyle => style_file(name.as_str()),
        };
        Some(Self {
            path,
            content: line.content.clone(),
        })
    }
}

/// Where the plugin is staged: a local marketplace folder that holds the plugin folder.
pub(crate) fn marketplace_dir(data: &Path) -> PathBuf {
    data.join("plugin")
}

fn text(value: &Value) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(value)? + "\n")
}

/// Every plugin file as it is staged for `profile`; the manifest and the hooks are adapted.
pub(crate) fn render(
    profile: Profile,
    version: &str,
    home: Option<&Path>,
    customs: &[Custom],
) -> anyhow::Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    for (path, shipped) in FILES {
        let content = match *path {
            MANIFEST => text(&manifest(profile, version, shipped)?)?,
            HOOKS => text(&hooks(profile, home, shipped)?)?,
            _ => (*shipped).to_owned(),
        };
        files.push(((*path).to_owned(), content));
    }
    for custom in customs {
        match files.iter_mut().find(|(path, _)| *path == custom.path) {
            Some(file) => file.1.clone_from(&custom.content),
            None => files.push((custom.path.clone(), custom.content.clone())),
        }
    }
    Ok(files)
}

fn manifest(profile: Profile, version: &str, shipped: &str) -> anyhow::Result<Value> {
    let mut manifest: Value = serde_json::from_str(shipped)?;
    manifest["name"] = json!(profile.plugin_name());
    manifest["displayName"] = json!(profile.display_name());
    manifest["version"] = json!(version);
    Ok(manifest)
}

/// Hooks name their profile, so a hook of a dev plugin never reports to the installed daemon.
fn hooks(profile: Profile, home: Option<&Path>, shipped: &str) -> anyhow::Result<Value> {
    let mut hooks: Value = serde_json::from_str(shipped)?;
    let events = hooks["hooks"]
        .as_object_mut()
        .context("the shipped hooks have no events")?;
    for groups in events.values_mut() {
        for hook in hooks_of(groups) {
            let Some(args) = hook["args"].as_array_mut() else {
                bail!("a shipped hook has no args");
            };
            let mut flags = vec![json!("--profile"), json!(profile.flag())];
            if let Some(home) = home {
                let absolute = std::path::absolute(home)
                    .with_context(|| format!("cannot resolve {}", home.display()))?;
                flags.extend([json!("--home"), json!(absolute.display().to_string())]);
            }
            args.splice(0..0, flags);
        }
    }
    Ok(hooks)
}

fn hooks_of(groups: &mut Value) -> impl Iterator<Item = &mut Value> {
    groups
        .as_array_mut()
        .into_iter()
        .flatten()
        .filter_map(|group| group["hooks"].as_array_mut())
        .flatten()
}

fn marketplace(profile: Profile, version: &str) -> anyhow::Result<String> {
    text(&json!({
        "name": profile.marketplace_name(),
        "description": "Local marketplace of NULLRADIX that serves the plugin installed by nullradix install",
        "owner": { "name": "NULLRADIX" },
        "plugins": [{
            "name": profile.plugin_name(),
            "version": version,
            "source": format!("./{}", profile.plugin_name()),
            "description": "Board, tickets, sessions and worktrees for parallel Claude Code sessions",
            "defaultEnabled": false,
        }],
    }))
}

#[derive(Debug, Serialize, Deserialize)]
struct State {
    hash: u64,
    version: String,
}

/// What staging did, or would do without `dry_run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Staged {
    pub(crate) version: String,
    pub(crate) written: usize,
    pub(crate) removed: usize,
    pub(crate) binary_copied: bool,
}

impl Staged {
    pub(crate) const fn changed(&self) -> bool {
        self.written > 0 || self.removed > 0 || self.binary_copied
    }
}

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn len_bytes(len: usize) -> [u8; 8] {
    u64::try_from(len).unwrap_or(u64::MAX).to_le_bytes()
}

fn content_hash(files: &[(String, String)], binary: &Path) -> anyhow::Result<u64> {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for (path, content) in files {
        hash = fnv(hash, &len_bytes(path.len()));
        hash = fnv(hash, path.as_bytes());
        hash = fnv(hash, &len_bytes(content.len()));
        hash = fnv(hash, content.as_bytes());
    }
    let bytes = fs::read(binary).with_context(|| format!("cannot read {}", binary.display()))?;
    Ok(fnv(hash, &bytes))
}

fn read_state(root: &Path) -> Option<State> {
    serde_json::from_slice(&fs::read(root.join(STATE_FILE)).ok()?).ok()
}

/// The version of a new content: the workspace version with the staging time as patch.
fn next_version(now_seconds: u64) -> String {
    let mut parts = env!("CARGO_PKG_VERSION").split('.');
    let major = parts.next().unwrap_or("0");
    let minor = parts.next().unwrap_or("0");
    format!("{major}.{minor}.{now_seconds}")
}

fn write_if_changed(path: &Path, content: &[u8], dry_run: bool) -> anyhow::Result<bool> {
    if fs::read(path).is_ok_and(|old| old == content) {
        return Ok(false);
    }
    if !dry_run {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        nullradix_os::write_atomic(path, content)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(true)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::read(a), fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn copy_binary(from: &Path, to: &Path, dry_run: bool) -> anyhow::Result<bool> {
    if same_file(from, to) {
        return Ok(false);
    }
    if !dry_run {
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let staging = to.with_extension("staging");
        let placed = fs::copy(from, &staging).and_then(|_| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&staging, fs::Permissions::from_mode(0o755))?;
            }
            fs::rename(&staging, to)
        });
        if let Err(error) = placed {
            let _ = fs::remove_file(&staging);
            return Err(error).with_context(|| {
                format!(
                    "cannot replace {}; end the Claude sessions that run it and try again",
                    to.display()
                )
            });
        }
    }
    Ok(true)
}

fn files_below(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            files_below(&path, found);
        } else {
            found.push(path);
        }
    }
}

fn remove_stale(plugin: &Path, keep: &[PathBuf], dry_run: bool) -> anyhow::Result<usize> {
    let mut found = Vec::new();
    files_below(plugin, &mut found);
    let bin = plugin.join("bin");
    let mut removed = 0;
    for path in found {
        if path.starts_with(&bin) || keep.contains(&path) {
            continue;
        }
        if !dry_run {
            fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
            let mut parent = path.parent();
            while let Some(dir) = parent.filter(|dir| *dir != plugin) {
                if fs::remove_dir(dir).is_err() {
                    break;
                }
                parent = dir.parent();
            }
        }
        removed += 1;
    }
    Ok(removed)
}

/// Lays the plugin out below `root`; unchanged content keeps its version, so a repeat is a no-op.
pub(crate) fn stage(
    root: &Path,
    profile: Profile,
    binary: &Path,
    home: Option<&Path>,
    customs: &[Custom],
    now_seconds: u64,
    dry_run: bool,
) -> anyhow::Result<Staged> {
    let hash = content_hash(&render(profile, "", home, customs)?, binary)?;
    let version = match read_state(root) {
        Some(state) if state.hash == hash => state.version,
        _ => next_version(now_seconds),
    };
    let files = render(profile, &version, home, customs)?;
    let plugin = root.join(profile.plugin_name());
    let mut staged = Staged {
        version: version.clone(),
        written: 0,
        removed: 0,
        binary_copied: false,
    };
    staged.binary_copied = copy_binary(binary, &plugin.join("bin").join(BINARY), dry_run)?;
    let mut keep = Vec::new();
    for (path, content) in &files {
        let target = plugin.join(path);
        staged.written += usize::from(write_if_changed(&target, content.as_bytes(), dry_run)?);
        keep.push(target);
    }
    staged.removed = remove_stale(&plugin, &keep, dry_run)?;
    let market = root.join(".claude-plugin").join("marketplace.json");
    staged.written += usize::from(write_if_changed(
        &market,
        marketplace(profile, &version)?.as_bytes(),
        dry_run,
    )?);
    let state = serde_json::to_vec(&State { hash, version })?;
    write_if_changed(&root.join(STATE_FILE), &state, dry_run)?;
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary(dir: &Path, bytes: &[u8]) -> PathBuf {
        let path = dir.join("nullradix-copy");
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn the_embedded_files_are_every_file_of_the_plugin_folder() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin");
        let mut found = Vec::new();
        files_below(&folder, &mut found);
        let mut on_disk: Vec<String> = found
            .iter()
            .map(|path| {
                path.strip_prefix(&folder)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        on_disk.sort();
        let mut embedded: Vec<&str> = FILES.iter().map(|(path, _)| *path).collect();
        embedded.sort_unstable();
        assert_eq!(on_disk, embedded);
    }

    #[test]
    fn the_release_plugin_has_the_release_names_and_hook_profile() {
        let files = render(Profile::Release, "0.1.5", None, &[]).unwrap();
        let get = |path: &str| {
            files
                .iter()
                .find(|(name, _)| name == path)
                .map(|(_, content)| serde_json::from_str::<Value>(content).unwrap())
                .unwrap()
        };
        let manifest = get(MANIFEST);
        assert_eq!(manifest["name"], "nullradix");
        assert_eq!(manifest["version"], "0.1.5");
        let hooks = get(HOOKS);
        assert_eq!(
            hooks["hooks"]["Stop"][0]["hooks"][0]["args"],
            json!(["--profile", "release", "hook", "stop"])
        );
    }

    #[test]
    fn the_staged_hook_arguments_are_a_hook_call_of_the_daemon_binary() {
        for profile in [Profile::Release, Profile::Dev] {
            let files = render(profile, "0.1.5", None, &[]).unwrap();
            let hooks: Value =
                serde_json::from_str(&files.iter().find(|(name, _)| name == HOOKS).unwrap().1)
                    .unwrap();
            for groups in hooks["hooks"].as_object().unwrap().values() {
                let args: Vec<std::ffi::OsString> = groups[0]["hooks"][0]["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| std::ffi::OsString::from(arg.as_str().unwrap()))
                    .collect();
                assert!(crate::hook::HookCall::parse(&args).is_some(), "{args:?}");
            }
        }
    }

    #[test]
    fn hooks_carry_the_home_the_install_ran_with() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("with space");
        let files = render(Profile::Dev, "0.1.5", Some(&home), &[]).unwrap();
        let hooks: Value =
            serde_json::from_str(&files.iter().find(|(name, _)| name == HOOKS).unwrap().1).unwrap();
        let args = &hooks["hooks"]["Stop"][0]["hooks"][0]["args"];
        assert_eq!(
            *args,
            json!([
                "--profile",
                "dev",
                "--home",
                home.display().to_string(),
                "hook",
                "stop"
            ])
        );
        let parsed: Vec<std::ffi::OsString> = args
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| std::ffi::OsString::from(arg.as_str().unwrap()))
            .collect();
        assert!(crate::hook::HookCall::parse(&parsed).is_some());
    }

    #[test]
    fn a_relative_home_is_written_as_an_absolute_one() {
        let files = render(Profile::Dev, "0.1.5", Some(Path::new("relative/home")), &[]).unwrap();
        let hooks: Value =
            serde_json::from_str(&files.iter().find(|(name, _)| name == HOOKS).unwrap().1).unwrap();
        let args = hooks["hooks"]["Stop"][0]["hooks"][0]["args"]
            .as_array()
            .unwrap();
        let written = Path::new(args[3].as_str().unwrap());
        assert!(written.is_absolute(), "{written:?}");
        assert!(written.ends_with("relative/home"));
    }

    #[test]
    fn the_marketplace_entry_is_off_by_default() {
        let market: Value =
            serde_json::from_str(&marketplace(Profile::Release, "0.1.5").unwrap()).unwrap();
        assert_eq!(market["plugins"][0]["defaultEnabled"], false);
    }

    #[test]
    fn the_binary_is_placed_whole_and_leaves_no_staging_file() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary(temp.path(), b"binary one");
        let root = temp.path().join("plugin");

        stage(&root, Profile::Release, &binary, None, &[], 1000, false).unwrap();

        let bin = root.join("nullradix/bin");
        let names: Vec<String> = fs::read_dir(&bin)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [BINARY]);
    }

    #[cfg(unix)]
    #[test]
    fn stale_removal_never_follows_a_symlinked_folder() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary(temp.path(), b"binary one");
        let root = temp.path().join("plugin");
        stage(&root, Profile::Release, &binary, None, &[], 1000, false).unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "mine").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("nullradix/skills/linked")).unwrap();

        stage(&root, Profile::Release, &binary, None, &[], 2000, false).unwrap();

        assert!(outside.join("keep.txt").is_file());
    }

    #[test]
    fn the_dev_plugin_is_a_separate_plugin_that_reports_to_the_dev_daemon() {
        let files = render(Profile::Dev, "0.1.5", None, &[]).unwrap();
        let manifest: Value =
            serde_json::from_str(&files.iter().find(|(name, _)| name == MANIFEST).unwrap().1)
                .unwrap();
        assert_eq!(manifest["name"], "nullradix-dev");
        assert_eq!(manifest["displayName"], "NULLRADIX Dev");
        let hooks = &files.iter().find(|(name, _)| name == HOOKS).unwrap().1;
        assert_eq!(hooks.matches("\"dev\"").count(), 7);
        assert!(!hooks.contains("\"release\""));
        assert!(
            marketplace(Profile::Dev, "0.1.5")
                .unwrap()
                .contains("nullradix-dev-local")
        );
    }

    #[test]
    fn staging_twice_changes_nothing_and_keeps_the_version() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary(temp.path(), b"binary one");
        let root = temp.path().join("plugin");

        let first = stage(&root, Profile::Dev, &binary, None, &[], 1000, false).unwrap();
        assert!(first.changed());
        assert_eq!(
            first.version,
            format!("{}.1000", &env!("CARGO_PKG_VERSION")[..3])
        );
        let skill = root.join("nullradix-dev/skills/board/SKILL.md");
        assert!(skill.is_file());
        assert!(root.join("nullradix-dev/bin").join(BINARY).is_file());
        assert!(root.join(".claude-plugin/marketplace.json").is_file());

        let second = stage(&root, Profile::Dev, &binary, None, &[], 2000, false).unwrap();
        assert!(!second.changed(), "{second:?}");
        assert_eq!(second.version, first.version);
    }

    #[test]
    fn a_new_binary_or_edited_file_gets_a_new_version_and_stale_files_go() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary(temp.path(), b"binary one");
        let root = temp.path().join("plugin");
        let first = stage(&root, Profile::Release, &binary, None, &[], 1000, false).unwrap();
        let plugin = root.join("nullradix");
        fs::write(plugin.join("skills/board/SKILL.md"), "edited").unwrap();
        fs::create_dir_all(plugin.join("skills/old")).unwrap();
        fs::write(plugin.join("skills/old/SKILL.md"), "stale").unwrap();

        let repaired = stage(&root, Profile::Release, &binary, None, &[], 2000, false).unwrap();
        assert_eq!(repaired.written, 1);
        assert_eq!(repaired.removed, 1);
        assert_eq!(repaired.version, first.version);
        assert!(!plugin.join("skills/old").exists());

        fs::write(&binary, b"binary two").unwrap();
        let updated = stage(&root, Profile::Release, &binary, None, &[], 3000, false).unwrap();
        assert!(updated.binary_copied);
        assert_ne!(updated.version, first.version);
        let manifest = fs::read_to_string(plugin.join(MANIFEST)).unwrap();
        assert!(manifest.contains(&updated.version));
    }

    #[test]
    fn a_dry_run_reports_the_changes_and_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary(temp.path(), b"binary one");
        let root = temp.path().join("plugin");

        let planned = stage(&root, Profile::Release, &binary, None, &[], 1000, true).unwrap();
        assert!(planned.changed());
        assert!(planned.written > 10);
        assert!(!root.exists());
    }

    async fn service() -> nullradix_app::customizations::CustomizationService {
        let store = std::sync::Arc::new(nullradix_store::Store::in_memory().await.unwrap());
        nullradix_app::customizations::CustomizationService::new(
            store,
            std::sync::Arc::new(ShippedStandards),
            std::sync::Arc::new(nullradix_app::SystemClock),
            std::sync::Arc::new(nullradix_app::UuidV7Generator),
            nullradix_app::ChangeSignal::new(),
        )
    }

    fn customs(lines: &[nullradix_app::customizations::OverlayLine]) -> Vec<Custom> {
        lines
            .iter()
            .filter_map(|line| Custom::of(&CustomizationDto::from(line)))
            .collect()
    }

    const BOARD: &str = "---\nname: board\ndescription: My board\n---\nMine\n";
    const MINE: &str = "---\nname: mine\ndescription: Own\n---\nOwn\n";

    #[test]
    fn the_catalog_lists_the_shipped_skills_and_output_styles() {
        assert_eq!(
            shipped_names(ExtensionPoint::Skill),
            [
                "board",
                "customize",
                "grill",
                "implement",
                "question",
                "setup",
                "tasks",
                "wayfinder"
            ]
        );
        assert_eq!(
            shipped_names(ExtensionPoint::OutputStyle),
            ["software-engineer"]
        );
    }

    #[tokio::test]
    async fn a_copy_starts_from_the_shipped_text() {
        let service = service().await;
        let line = service
            .create(ExtensionPoint::Skill, "board", None)
            .await
            .unwrap();
        assert_eq!(
            line.overlay.record().content,
            shipped("skills/board/SKILL.md").unwrap()
        );
        assert!(line.standard);
        let none = service.create(ExtensionPoint::Skill, "nope", None).await;
        assert!(none.is_err());
    }

    #[tokio::test]
    async fn a_customized_skill_is_staged_and_changes_the_version_only_when_it_changes() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary(temp.path(), b"binary one");
        let root = temp.path().join("plugin");
        let plain = stage(&root, Profile::Release, &binary, None, &[], 1000, false).unwrap();

        let service = service().await;
        let line = service
            .create(ExtensionPoint::Skill, "board", Some(BOARD))
            .await
            .unwrap();
        service
            .create(ExtensionPoint::Skill, "mine", Some(MINE))
            .await
            .unwrap();
        let disabled = service
            .create(ExtensionPoint::OutputStyle, "software-engineer", None)
            .await
            .unwrap();
        service
            .update(
                ExtensionPoint::OutputStyle,
                "software-engineer",
                None,
                Some(false),
                disabled.overlay.record().version,
            )
            .await
            .unwrap();
        assert_eq!(line.problems.len(), 0);
        let applied = customs(&service.list().await.unwrap());
        assert_eq!(applied.len(), 2);

        let first = stage(
            &root,
            Profile::Release,
            &binary,
            None,
            &applied,
            2000,
            false,
        )
        .unwrap();
        assert_ne!(first.version, plain.version);
        let plugin = root.join("nullradix");
        assert_eq!(
            fs::read_to_string(plugin.join("skills/board/SKILL.md")).unwrap(),
            BOARD
        );
        assert!(plugin.join("skills/mine/SKILL.md").is_file());
        assert_eq!(
            fs::read_to_string(plugin.join("output-styles/software-engineer.md")).unwrap(),
            shipped("output-styles/software-engineer.md").unwrap()
        );

        let again = stage(
            &root,
            Profile::Release,
            &binary,
            None,
            &applied,
            3000,
            false,
        )
        .unwrap();
        assert!(!again.changed(), "{again:?}");
        assert_eq!(again.version, first.version);

        let back = stage(&root, Profile::Release, &binary, None, &[], 4000, false).unwrap();
        assert_ne!(back.version, first.version);
        assert!(!plugin.join("skills/mine").exists());
    }

    fn dto(point: &str, name: &str, state: &str) -> CustomizationDto {
        serde_json::from_value(json!({
            "point": point, "name": name, "kind": "new", "state": state, "content": "x",
            "enabled": true, "base": "none", "standard": false, "problems": [],
            "warnings": [], "createdAt": 0, "updatedAt": 0, "version": 1,
        }))
        .unwrap()
    }

    #[test]
    fn a_name_that_is_not_a_valid_name_is_never_staged() {
        for name in ["nul", "../x", "a/b", "A"] {
            assert_eq!(Custom::of(&dto("skill", name, "active")), None, "{name}");
        }
        assert!(Custom::of(&dto("skill", "fine", "active")).is_some());
        assert_eq!(Custom::of(&dto("skill", "fine", "fallback")), None);
    }

    #[test]
    fn moving_a_byte_between_path_and_content_changes_the_hash() {
        let temp = tempfile::tempdir().unwrap();
        let binary = binary(temp.path(), b"b");
        let one = [("ab".to_owned(), "c".to_owned())];
        let two = [("a".to_owned(), "bc".to_owned())];
        assert_ne!(
            content_hash(&one, &binary).unwrap(),
            content_hash(&two, &binary).unwrap()
        );
    }
}
