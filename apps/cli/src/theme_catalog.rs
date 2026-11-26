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
    },
}

pub(crate) fn run(command: CatalogCommand, client: &Client) -> anyhow::Result<()> {
    match command {
        CatalogCommand::List { json } => list(client, json),
        CatalogCommand::Sources { json } => sources(client, json),
        CatalogCommand::Add { url } => {
            let source = client.add_catalog_source(&url)?;
            println!("added {} {}", source.id, source.url);
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
        CatalogCommand::Install { theme, source } => install(client, &theme, source.as_deref()),
    }
}

fn state_text(state: CatalogStateDto, installed: Option<&str>) -> String {
    match (state, installed) {
        (CatalogStateDto::Available, _) => String::from("available"),
        (CatalogStateDto::Installed, version) => {
            format!("installed {}", version.unwrap_or_default())
        }
        (CatalogStateDto::Update, version) => {
            format!("update from {}", version.unwrap_or_default())
        }
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
            theme.id,
            theme.name,
            theme.version,
            state_text(theme.state, theme.installed_version.as_deref()),
            theme.source_id
        );
    }
    for status in &catalog.sources {
        if let Some(problem) = &status.problem {
            eprintln!(
                "source {} failed: {} ({})",
                status.source.url, problem.message, problem.code
            );
        }
        for skipped in &status.skipped {
            eprintln!(
                "source {} skipped entry {}: {}",
                status.source.url, skipped.index, skipped.reason
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
        println!("{} {}", source.id, source.url);
    }
    Ok(())
}

fn install(client: &Client, theme: &str, source: Option<&str>) -> anyhow::Result<()> {
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
            [] => bail!("no source offers {theme}"),
            [only] => (*only).to_owned(),
            several => bail!(
                "several sources offer {theme}; pick one with --source: {}",
                several.join(", ")
            ),
        }
    };
    let installed = client.install_from_catalog(&source, theme)?;
    println!("installed {} {}", installed.id, installed.package_version);
    Ok(())
}
