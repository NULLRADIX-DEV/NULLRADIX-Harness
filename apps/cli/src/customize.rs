use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Subcommand, ValueEnum};
use nullradix_api::{
    BaseStateDto, CreateCustomization, CustomizationDto, ExtensionPointDto, OverlayKindDto,
    OverlayStateDto, UpdateCustomization,
};
use nullradix_client::{Client, ClientError};

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum PointArg {
    Skill,
    OutputStyle,
}

impl From<PointArg> for ExtensionPointDto {
    fn from(point: PointArg) -> Self {
        match point {
            PointArg::Skill => Self::Skill,
            PointArg::OutputStyle => Self::OutputStyle,
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum CustomizeCommand {
    /// List the customizations with their state, problems and hints
    Overview {
        /// Print JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Print the text in charge of a name: the customization, else the shipped file
    Show { point: PointArg, name: String },
    /// Start from the shipped file, or with --rebase make it the new starting point
    Copy {
        point: PointArg,
        name: String,
        /// Keep the content, renew the starting point
        #[arg(long)]
        rebase: bool,
    },
    /// Create a customization from a file, or replace the content of an existing one
    Set {
        point: PointArg,
        name: String,
        /// File with the whole text and its head, `-` reads standard input
        #[arg(long)]
        file: PathBuf,
    },
    /// Let a customization apply again
    Enable { point: PointArg, name: String },
    /// Keep a customization without applying it
    Disable { point: PointArg, name: String },
    /// Drop a customization; the shipped file is in charge again
    Reset { point: PointArg, name: String },
}

const fn kind_label(kind: OverlayKindDto) -> &'static str {
    match kind {
        OverlayKindDto::Replace => "replace",
        OverlayKindDto::New => "new",
    }
}

const fn state_label(state: OverlayStateDto) -> &'static str {
    match state {
        OverlayStateDto::Active => "active",
        OverlayStateDto::Fallback => "fallback",
        OverlayStateDto::Disabled => "disabled",
    }
}

const fn base_label(base: BaseStateDto) -> &'static str {
    match base {
        BaseStateDto::None => "-",
        BaseStateDto::Current => "current",
        BaseStateDto::Outdated => "outdated",
        BaseStateDto::Unknown => "unknown",
    }
}

const fn point_label(point: ExtensionPointDto) -> &'static str {
    match point {
        ExtensionPointDto::Skill => "skill",
        ExtensionPointDto::OutputStyle => "output-style",
    }
}

fn read_text(file: &PathBuf) -> anyhow::Result<String> {
    if file.as_os_str() == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context("cannot read standard input")?;
        return Ok(text);
    }
    std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))
}

fn report(line: &CustomizationDto) {
    println!(
        "{} {}: {}, {}, base {}",
        point_label(line.point),
        line.name,
        kind_label(line.kind),
        state_label(line.state),
        base_label(line.base)
    );
    for problem in &line.problems {
        println!("  problem {}: {}", problem.code, problem.message);
    }
    for warning in &line.warnings {
        println!("  hint {}: {}", warning.code, warning.message);
    }
}

fn find(
    client: &Client,
    point: ExtensionPointDto,
    name: &str,
) -> anyhow::Result<Option<CustomizationDto>> {
    match client.get_customization(point, name) {
        Ok(line) => Ok(Some(line)),
        Err(ClientError::Refused { status: 404, .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn run(command: CustomizeCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        CustomizeCommand::Overview { json } => {
            let items = client.list_customizations()?.items;
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else if items.is_empty() {
                println!("nothing is customized, start with `nullradix customize set`");
            }
            if !json {
                for line in &items {
                    report(line);
                }
            }
        }
        CustomizeCommand::Show { point, name } => {
            let effective = client.effective_customization(point.into(), &name)?;
            print!("{}", effective.content);
        }
        CustomizeCommand::Copy {
            point,
            name,
            rebase,
        } => {
            let point = ExtensionPointDto::from(point);
            let line = if rebase {
                let current = client.get_customization(point, &name)?;
                client.rebase_customization(point, &name, current.version)?
            } else {
                client.create_customization(&CreateCustomization {
                    point,
                    name,
                    content: None,
                })?
            };
            report(&line);
        }
        CustomizeCommand::Set { point, name, file } => {
            let point = ExtensionPointDto::from(point);
            let content = read_text(&file)?;
            let line = match find(client, point, &name)? {
                Some(current) => client.update_customization(
                    point,
                    &name,
                    &UpdateCustomization {
                        content: Some(content),
                        enabled: None,
                        version: current.version,
                    },
                )?,
                None => client.create_customization(&CreateCustomization {
                    point,
                    name,
                    content: Some(content),
                })?,
            };
            report(&line);
        }
        CustomizeCommand::Enable { point, name } => switch(client, point, &name, true)?,
        CustomizeCommand::Disable { point, name } => switch(client, point, &name, false)?,
        CustomizeCommand::Reset { point, name } => {
            let point = ExtensionPointDto::from(point);
            let current = client.get_customization(point, &name)?;
            client.delete_customization(point, &name, current.version)?;
            println!("dropped {} {name}", point_label(point));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn switch(client: &Client, point: PointArg, name: &str, enabled: bool) -> anyhow::Result<()> {
    let point = ExtensionPointDto::from(point);
    let current = client.get_customization(point, name)?;
    let line = client.update_customization(
        point,
        name,
        &UpdateCustomization {
            content: None,
            enabled: Some(enabled),
            version: current.version,
        },
    )?;
    report(&line);
    Ok(())
}
