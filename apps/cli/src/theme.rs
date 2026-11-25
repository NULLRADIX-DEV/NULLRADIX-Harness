use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Subcommand;
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
    /// Check a theme folder and pack it into a `.nullradix` package; needs no daemon
    Pack {
        /// Folder with manifest.json and theme.css
        dir: PathBuf,
        /// Where to write the package; `<id>.nullradix` in the current folder when left out
        #[arg(long)]
        out: Option<PathBuf>,
    },
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
        ThemeCommand::Pack { .. } => bail!("pack runs without a daemon"),
    }
    Ok(ExitCode::SUCCESS)
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
