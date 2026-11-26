use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Subcommand;
use nullradix_api::{SaveThemeSettings, ThemeDto, ThemeSettingDto, ThemeSettingValueDto};
use nullradix_app::theme_package::{PackageFile, check_package};
use nullradix_client::Client;
use nullradix_domain::{BUILTIN_THEMES, MAX_PACKAGE_BYTES};

#[derive(Subcommand)]
pub(crate) enum ThemeCommand {
    /// List the built-in and installed themes; the chosen one is marked
    List {
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Install a `.nullradix` package, or replace the installed theme with the same id
    Install {
        /// The package file
        file: PathBuf,
    },
    /// Choose the theme the web UI shows
    Use {
        /// A built-in theme (nullradix, system, light, dark) or an installed id
        id: String,
    },
    /// Uninstall a theme; when it was chosen the default takes over
    Remove { id: String },
    /// Show the settings of an installed theme, set some or reset them all
    Settings {
        id: String,
        #[command(subcommand)]
        action: Option<SettingsAction>,
    },
    /// Check a theme folder and pack it into a `.nullradix` package; needs no daemon
    Pack {
        /// Folder with manifest.json and theme.css
        dir: PathBuf,
        /// Where to write the package; `<id>.nullradix` in the current folder when left out
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SettingsAction {
    /// Set settings and keep the others as they are
    Set {
        #[arg(required = true, value_name = "KEY=VALUE")]
        pairs: Vec<String>,
    },
    /// Back to the defaults of the manifest
    Reset,
}

pub(crate) fn run(command: ThemeCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        ThemeCommand::List { json } => list(client, json)?,
        ThemeCommand::Install { file } => {
            let size = std::fs::metadata(&file)
                .with_context(|| format!("cannot read {}", file.display()))?
                .len();
            if size > MAX_PACKAGE_BYTES {
                bail!(
                    "{} is larger than {MAX_PACKAGE_BYTES} bytes",
                    file.display()
                );
            }
            let bytes =
                std::fs::read(&file).with_context(|| format!("cannot read {}", file.display()))?;
            let theme = client.install_theme(&bytes)?;
            println!("installed {} {}", theme.id, theme.package_version);
        }
        ThemeCommand::Use { id } => {
            let chosen = client.choose_theme(&id)?;
            println!("now using {}", chosen.theme);
        }
        ThemeCommand::Remove { id } => {
            let installed = client.list_themes()?.items;
            let Some(theme) = installed.iter().find(|theme| theme.id == id) else {
                bail!("the theme {id} is not installed");
            };
            client.remove_theme(&id, theme.version)?;
            println!("removed {id}");
        }
        ThemeCommand::Settings { id, action } => settings(client, &id, action)?,
        ThemeCommand::Pack { .. } => bail!("pack runs without a daemon"),
    }
    Ok(ExitCode::SUCCESS)
}

fn key_of(setting: &ThemeSettingDto) -> &str {
    match setting {
        ThemeSettingDto::Boolean { key, .. } | ThemeSettingDto::Select { key, .. } => key,
    }
}

/// Reads `key=value`; a switch takes true or false, anything else is text for the server to check.
fn parse_pair(
    declared: &[ThemeSettingDto],
    pair: &str,
) -> anyhow::Result<(String, ThemeSettingValueDto)> {
    let Some((key, text)) = pair.split_once('=') else {
        bail!("{pair} is not KEY=VALUE");
    };
    let is_switch = declared.iter().any(|setting| {
        key_of(setting) == key && matches!(setting, ThemeSettingDto::Boolean { .. })
    });
    let value = if is_switch {
        match text {
            "true" | "on" => ThemeSettingValueDto::Switch(true),
            "false" | "off" => ThemeSettingValueDto::Switch(false),
            _ => bail!("{key} takes true or false"),
        }
    } else {
        ThemeSettingValueDto::Choice(text.to_owned())
    };
    Ok((key.to_owned(), value))
}

fn shown(value: &ThemeSettingValueDto) -> String {
    match value {
        ThemeSettingValueDto::Switch(on) => on.to_string(),
        ThemeSettingValueDto::Choice(text) => text.clone(),
    }
}

/// Keeps the values that still fit a declared setting.
fn fitting(
    declared: &[ThemeSettingDto],
    values: BTreeMap<String, ThemeSettingValueDto>,
) -> BTreeMap<String, ThemeSettingValueDto> {
    values
        .into_iter()
        .filter(|(key, value)| {
            declared.iter().any(|setting| match (setting, value) {
                (ThemeSettingDto::Boolean { key: k, .. }, ThemeSettingValueDto::Switch(_)) => {
                    k == key
                }
                (
                    ThemeSettingDto::Select {
                        key: k, options, ..
                    },
                    ThemeSettingValueDto::Choice(text),
                ) => k == key && options.iter().any(|option| &option.value == text),
                _ => false,
            })
        })
        .collect()
}

fn settings_lines(
    theme: &ThemeDto,
    values: &BTreeMap<String, ThemeSettingValueDto>,
) -> Vec<String> {
    if theme.settings.is_empty() {
        return vec![format!("{} declares no settings", theme.id)];
    }
    let values = fitting(&theme.settings, values.clone());
    let mut lines = Vec::new();
    for setting in &theme.settings {
        let (label, default) = match setting {
            ThemeSettingDto::Boolean { label, default, .. } => (label, default.to_string()),
            ThemeSettingDto::Select { label, default, .. } => (label, default.clone()),
        };
        let key = key_of(setting);
        let value = values.get(key).map_or_else(|| default.clone(), shown);
        lines.push(format!("{key:<20} {value:<16} {label} (default {default})"));
    }
    lines
}

fn print_settings(theme: &ThemeDto, values: &BTreeMap<String, ThemeSettingValueDto>) {
    for line in settings_lines(theme, values) {
        println!("{line}");
    }
}

fn settings(client: &Client, id: &str, action: Option<SettingsAction>) -> anyhow::Result<()> {
    let themes = client.list_themes()?.items;
    let Some(theme) = themes.iter().find(|theme| theme.id == id) else {
        bail!("the theme {id} is not installed");
    };
    let current = client.theme_settings(id)?;
    match action {
        None => print_settings(theme, &current.values),
        Some(SettingsAction::Set { pairs }) => {
            let mut values = fitting(&theme.settings, current.values);
            for pair in &pairs {
                let (key, value) = parse_pair(&theme.settings, pair)?;
                values.insert(key, value);
            }
            let input = SaveThemeSettings {
                values,
                version: current.version,
            };
            let saved = client.save_theme_settings(id, &input)?;
            print_settings(theme, &saved.values);
        }
        Some(SettingsAction::Reset) => {
            client.reset_theme_settings(id, current.version)?;
            println!("reset the settings of {id}");
        }
    }
    Ok(())
}

fn list(client: &Client, json: bool) -> anyhow::Result<()> {
    let list = client.list_themes()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    let mark = |id: &str| if list.active == id { "*" } else { " " };
    for id in BUILTIN_THEMES {
        println!("{} {id:<20} built in", mark(id));
    }
    for theme in &list.items {
        println!(
            "{} {:<20} {} v{} by {}",
            mark(&theme.id),
            theme.id,
            theme.name,
            theme.package_version,
            theme.author
        );
    }
    Ok(())
}

/// Builds a package from a folder with the checks of an installation.
pub(crate) fn pack(dir: &Path, out: Option<&Path>) -> anyhow::Result<ExitCode> {
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let entries = nullradix_os::read_tree(dir, crate::themes::LIMITS)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", dir.display()))?;
    let files: Vec<PackageFile> = entries
        .iter()
        .map(|entry| PackageFile {
            path: entry.path.clone(),
            data: entry.data.clone(),
        })
        .collect();
    let manifest = check_package(&files, true).map_err(|error| {
        let fields: Vec<String> = error
            .fields()
            .iter()
            .map(|field| format!("  {}: {}", field.field, field.message))
            .collect();
        if fields.is_empty() {
            anyhow::anyhow!("{}", error.message())
        } else {
            anyhow::anyhow!("the manifest is invalid\n{}", fields.join("\n"))
        }
    })?;
    let bytes = nullradix_os::pack_zip(&entries)?;
    let target = out.map_or_else(
        || PathBuf::from(format!("{}.nullradix", manifest.id())),
        Path::to_path_buf,
    );
    std::fs::write(&target, &bytes)
        .with_context(|| format!("cannot write {}", target.display()))?;
    println!(
        "packed {} {} ({} files) into {}",
        manifest.id(),
        manifest.record().version,
        files.len(),
        target.display()
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        "id": "night-blue", "name": "Night Blue", "version": "1.0.0", "author": "Ada",
        "sdkVersion": "^1.3", "colorScheme": "dark", "slots": [], "permissions": { "network": [] }
    }"#;

    fn folder(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, data) in files {
            let target = dir.path().join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, data).unwrap();
        }
        dir
    }

    fn declared() -> Vec<ThemeSettingDto> {
        vec![
            ThemeSettingDto::Boolean {
                key: String::from("dim"),
                label: String::from("Dim"),
                hint: None,
                default: true,
            },
            ThemeSettingDto::Select {
                key: String::from("mode"),
                label: String::from("Mode"),
                hint: None,
                options: Vec::new(),
                default: String::from("a"),
            },
        ]
    }

    fn theme_with(settings: Vec<ThemeSettingDto>) -> ThemeDto {
        ThemeDto {
            id: String::from("night-blue"),
            name: String::from("Night Blue"),
            package_version: String::from("1.0.0"),
            author: String::from("Ada"),
            sdk_version: String::from("^1.3"),
            color_scheme: nullradix_api::ColorSchemeDto::Dark,
            start_color: None,
            preview_url: None,
            style_url: String::new(),
            slots: Vec::new(),
            network: Vec::new(),
            settings,
            claude_theme: false,
            installed_at: 0,
            updated_at: 0,
            version: 1,
        }
    }

    #[test]
    fn stale_values_are_neither_shown_nor_sent_back() {
        let mut theme = theme_with(declared());
        if let ThemeSettingDto::Select { options, .. } = &mut theme.settings[1] {
            options.push(nullradix_api::ThemeOptionDto {
                value: String::from("a"),
                label: String::from("A"),
            });
        }
        let stale = BTreeMap::from([
            (
                String::from("dim"),
                ThemeSettingValueDto::Choice(String::from("x")),
            ),
            (
                String::from("mode"),
                ThemeSettingValueDto::Choice(String::from("gone")),
            ),
            (String::from("old"), ThemeSettingValueDto::Switch(true)),
        ]);
        assert!(fitting(&theme.settings, stale.clone()).is_empty());
        let lines = settings_lines(&theme, &stale);
        assert!(lines[0].contains("true"), "{lines:?}");
        assert!(
            lines[1].starts_with("mode") && lines[1].contains(" a "),
            "{lines:?}"
        );
    }

    #[test]
    fn pairs_are_typed_by_the_declared_setting() {
        let declared = declared();
        let (key, value) = parse_pair(&declared, "dim=off").unwrap();
        assert_eq!(
            (key.as_str(), value),
            ("dim", ThemeSettingValueDto::Switch(false))
        );
        let (_, value) = parse_pair(&declared, "mode=b").unwrap();
        assert_eq!(value, ThemeSettingValueDto::Choice(String::from("b")));
        let (_, value) = parse_pair(&declared, "other=true=x").unwrap();
        assert_eq!(value, ThemeSettingValueDto::Choice(String::from("true=x")));
        assert!(parse_pair(&declared, "dim=maybe").is_err());
        assert!(parse_pair(&declared, "dim").is_err());
    }

    #[test]
    fn a_folder_becomes_a_package_the_installer_accepts() {
        let dir = folder(&[
            ("manifest.json", MANIFEST),
            ("theme.css", "a { color: red }"),
            ("assets/a.png", "png"),
        ]);
        let out = dir.path().join("out.nullradix");
        let source = dir.path().join("theme-src");
        std::fs::create_dir(&source).unwrap();
        for name in ["manifest.json", "theme.css"] {
            std::fs::rename(dir.path().join(name), source.join(name)).unwrap();
        }
        std::fs::rename(dir.path().join("assets"), source.join("assets")).unwrap();
        let _ = pack(&source, Some(&out)).unwrap();
        let files =
            nullradix_os::unpack_zip(&std::fs::read(&out).unwrap(), crate::themes::LIMITS).unwrap();
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["assets/a.png", "manifest.json", "theme.css"]);
    }

    #[test]
    fn a_folder_with_problems_is_not_packed() {
        let missing_css = folder(&[("manifest.json", MANIFEST)]);
        let error = pack(
            missing_css.path(),
            Some(&missing_css.path().join("x.nullradix")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("theme.css"));
        let hidden = folder(&[
            ("manifest.json", MANIFEST),
            ("theme.css", "a {}"),
            (".secret/key.json", "{}"),
        ]);
        let error = pack(hidden.path(), Some(&hidden.path().join("x.nullradix"))).unwrap_err();
        assert!(error.to_string().contains("hidden"), "{error}");
        let bad_manifest = folder(&[("manifest.json", "{\"id\": \"X\"}"), ("theme.css", "a {}")]);
        let error = pack(
            bad_manifest.path(),
            Some(&bad_manifest.path().join("x.nullradix")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("id:"), "{error}");
        assert!(pack(Path::new("no-such-folder"), None).is_err());
    }
}
