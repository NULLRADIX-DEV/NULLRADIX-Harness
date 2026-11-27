use anyhow::bail;
use clap::Subcommand;
use nullradix_api::{CatalogDto, CatalogStateDto};
use nullradix_client::Client;

#[derive(Subcommand)]
pub(crate) enum CatalogCommand {
    /// List the themes the sources offer and whether they are installed or have an update
    List {
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Show the sources
    Sources {
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Add a source: an https address of an index file
    Add { url: String },
    /// Remove a source by its id
    Remove { id: String },
    /// Install a theme from a source, or update it
    Install {
        /// Theme id as the catalog lists it
        theme: String,
        /// Source id; needed only when several sources offer the theme
        #[arg(long)]
        source: Option<String>,
        /// Replace a theme of the same id that was installed from another source or by upload
        #[arg(long)]
        replace: bool,
        /// Checksum from the refusal; confirms the code, domains or Claude part of that package
        #[arg(long, value_name = "SHA256")]
        confirm: Option<String>,
    },
}

pub(crate) fn run(command: CatalogCommand, client: &Client) -> anyhow::Result<()> {
    match command {
        CatalogCommand::List { json } => list(client, json),
        CatalogCommand::Sources { json } => sources(client, json),
        CatalogCommand::Add { url } => {
            let source = client.add_catalog_source(&url)?;
            println!("added {} {}", plain(&source.id), plain(&source.url));
            Ok(())
        }
        CatalogCommand::Remove { id } => {
            let list = client.catalog_sources()?;
            if !list.items.iter().any(|source| source.id == id) {
                bail!("the source {id} is not in the list");
            }
            client.remove_catalog_source(&id, list.version)?;
            println!("removed {id}");
            Ok(())
        }
        CatalogCommand::Install {
            theme,
            source,
            replace,
            confirm,
        } => install(
            client,
            &theme,
            source.as_deref(),
            replace,
            confirm.as_deref(),
        ),
    }
}

/// The text with control and direction characters replaced, so an index cannot steer the terminal.
pub(crate) fn plain(text: &str) -> String {
    text.chars()
        .map(|c| {
            let direction = matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
            if c.is_control() || direction { '?' } else { c }
        })
        .collect()
}

fn state_text(state: CatalogStateDto, installed: Option<&str>) -> String {
    match (state, installed) {
        (CatalogStateDto::Available, _) => String::from("available"),
        (CatalogStateDto::Installed, version) => {
            format!("installed {}", plain(version.unwrap_or_default()))
        }
        (CatalogStateDto::Update, version) => {
            format!("update from {}", plain(version.unwrap_or_default()))
        }
        (CatalogStateDto::OtherSource, version) => format!(
            "installed {} from another source (--replace to take over)",
            plain(version.unwrap_or_default())
        ),
    }
}

fn list(client: &Client, json: bool) -> anyhow::Result<()> {
    let catalog: CatalogDto = client.browse_catalog()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&catalog)?);
        return Ok(());
    }
    for theme in &catalog.items {
        let fit = if theme.compatible {
            ""
        } else {
            " (needs another SDK)"
        };
        println!(
            "{:<20} {:<24} v{:<10} {} [{}]{fit}",
            plain(&theme.id),
            plain(&theme.name),
            plain(&theme.version),
            state_text(theme.state, theme.installed_version.as_deref()),
            plain(&theme.source_id)
        );
    }
    for status in &catalog.sources {
        if let Some(problem) = &status.problem {
            eprintln!(
                "source {} failed: {} ({})",
                plain(&status.source.url),
                plain(&problem.message),
                plain(&problem.code)
            );
        }
        for skipped in &status.skipped {
            eprintln!(
                "source {} skipped entry {}{}: {}",
                plain(&status.source.url),
                skipped.index,
                skipped
                    .id
                    .as_deref()
                    .map_or_else(String::new, |id| format!(" ({})", plain(id))),
                plain(&skipped.reason)
            );
        }
    }
    Ok(())
}

fn sources(client: &Client, json: bool) -> anyhow::Result<()> {
    let list = client.catalog_sources()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    for source in &list.items {
        println!("{} {}", plain(&source.id), plain(&source.url));
    }
    Ok(())
}

fn install(
    client: &Client,
    theme: &str,
    source: Option<&str>,
    replace: bool,
    confirm: Option<&str>,
) -> anyhow::Result<()> {
    let source = if let Some(source) = source {
        source.to_owned()
    } else {
        let catalog = client.browse_catalog()?;
        let offers: Vec<&str> = catalog
            .items
            .iter()
            .filter(|item| item.id == theme)
            .map(|item| item.source_id.as_str())
            .collect();
        match offers.as_slice() {
            [] => bail!("no source offers {}", plain(theme)),
            [only] => (*only).to_owned(),
            several => bail!(
                "several sources offer {}; pick one with --source: {}",
                plain(theme),
                several.join(", ")
            ),
        }
    };
    let installed = client
        .install_from_catalog(&source, theme, replace, confirm)
        .map_err(|error| {
            let install = crate::theme::CatalogInstall {
                theme,
                source: &source,
                replace,
            };
            crate::theme::ask_for_checksum(error, &install)
        })?;
    println!(
        "installed {} {}",
        plain(&installed.id),
        plain(&installed.package_version)
    );
    crate::theme::note_code_is_off(&installed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_and_direction_characters_never_reach_the_terminal() {
        let text = "a\u{1b}[2Jb\nc\u{202e}d\u{2066}e";
        assert_eq!(plain(text), "a?[2Jb?c?d?e");
        assert_eq!(plain("Night Blue 1.0"), "Night Blue 1.0");
    }
}
