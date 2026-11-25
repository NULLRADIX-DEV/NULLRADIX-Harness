use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Subcommand, ValueEnum};
use nullradix_api::{PreviewPrompt, PromptDto, PromptKindDto, SetPrompt};
use nullradix_client::Client;

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum KindArg {
    Work,
    Plan,
    Research,
    Merge,
}

impl From<KindArg> for PromptKindDto {
    fn from(kind: KindArg) -> Self {
        match kind {
            KindArg::Work => Self::Work,
            KindArg::Plan => Self::Plan,
            KindArg::Research => Self::Research,
            KindArg::Merge => Self::Merge,
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum PromptCommand {
    /// List the prompt templates with their placeholders and required parts
    List {
        /// Print JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Print the template in charge: the own one, else the shipped default
    Show {
        kind: KindArg,
        /// Print the shipped default even when an own template is set
        #[arg(long)]
        default: bool,
    },
    /// Set an own template from a file
    Set {
        kind: KindArg,
        /// File with the template, or `-` for standard input; blank goes back to the default
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
    },
    /// Drop the own template; the shipped default applies again
    Reset { kind: KindArg },
    /// Render a template for a ticket (a question for research), or with sample values
    Preview {
        kind: KindArg,
        /// Ticket key such as ARC-7, a question key for research; sample values without
        key: Option<String>,
        /// Render the template in this file instead of the stored one, without saving it
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// Print JSON instead of the instruction
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(command: PromptCommand, client: &Client) -> anyhow::Result<ExitCode> {
    match command {
        PromptCommand::List { json } => {
            let list = client.dispatcher_prompts()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                print!("{}", describe(&list.items, list.version));
            }
        }
        PromptCommand::Show { kind, default } => {
            let detail = client.dispatcher_prompt(kind.into())?;
            let text = match (&detail.prompt.template, default) {
                (Some(own), false) => own,
                _ => &detail.prompt.default,
            };
            println!("{text}");
        }
        PromptCommand::Set { kind, file } => {
            let template = read_text(&file)?;
            let current = client.dispatcher_prompt(kind.into())?;
            let saved = client.set_dispatcher_prompt(
                kind.into(),
                &SetPrompt {
                    template,
                    version: current.version,
                },
            )?;
            println!("{}", saved_line(&saved.prompt, saved.version));
        }
        PromptCommand::Reset { kind } => {
            let current = client.dispatcher_prompt(kind.into())?;
            if current.prompt.template.is_none() {
                println!(
                    "the {} prompt already uses the default",
                    current.prompt.kind.as_str()
                );
            } else {
                let saved = client.reset_dispatcher_prompt(kind.into(), current.version)?;
                println!("{}", saved_line(&saved.prompt, saved.version));
            }
        }
        PromptCommand::Preview {
            kind,
            key,
            file,
            json,
        } => {
            let template = file.as_deref().map(read_text).transpose()?;
            let preview = client.preview_dispatcher_prompt(&PreviewPrompt {
                kind: kind.into(),
                key,
                template,
            })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&preview)?);
            } else {
                println!("{}", preview.prompt);
                if preview.sample {
                    eprintln!("note: sample values, give a ticket key to use a real one");
                }
                if !preview.stand_ins.is_empty() {
                    eprintln!(
                        "note: stand-in text for {}, there is no data for it yet",
                        preview.stand_ins.join(", ")
                    );
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn read_text(path: &Path) -> anyhow::Result<String> {
    let text = if path == Path::new("-") {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context("cannot read standard input")?;
        text
    } else {
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?
    };
    if text.contains('\0') {
        bail!("{} is not a text file", path.display());
    }
    Ok(text)
}

fn saved_line(prompt: &PromptDto, version: i64) -> String {
    let state = if prompt.template.is_some() {
        "own template"
    } else {
        "default"
    };
    format!(
        "the {} prompt uses the {state}, configuration version {version}",
        prompt.kind.as_str()
    )
}

fn describe(items: &[PromptDto], version: i64) -> String {
    let mut out = format!("prompt templates, configuration version {version}\n");
    for item in items {
        let state = if item.template.is_some() {
            "own"
        } else {
            "default"
        };
        let _ = writeln!(out, "{:<9} {state}", item.kind.as_str());
        let names: Vec<&str> = item
            .placeholders
            .iter()
            .map(|placeholder| placeholder.name.as_str())
            .collect();
        let _ = writeln!(out, "  placeholders: {}", names.join(", "));
        for part in &item.required {
            let _ = writeln!(out, "  required: {part}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(template: Option<&str>) -> PromptDto {
        PromptDto {
            kind: PromptKindDto::Plan,
            label: String::from("Planning"),
            default: String::from("d"),
            template: template.map(str::to_owned),
            placeholders: vec![nullradix_api::PlaceholderDto {
                name: String::from("agent"),
                description: String::from("who"),
            }],
            required: vec![String::from("File plan.json")],
        }
    }

    #[test]
    fn the_list_names_the_state_placeholders_and_required_parts() {
        let text = describe(&[item(None), item(Some("x"))], 3);
        assert_eq!(
            text,
            "prompt templates, configuration version 3\n\
             plan      default\n  placeholders: agent\n  required: File plan.json\n\
             plan      own\n  placeholders: agent\n  required: File plan.json\n"
        );
    }

    #[test]
    fn a_saved_line_says_which_template_is_in_charge() {
        assert_eq!(
            saved_line(&item(Some("x")), 2),
            "the plan prompt uses the own template, configuration version 2"
        );
        assert_eq!(
            saved_line(&item(None), 3),
            "the plan prompt uses the default, configuration version 3"
        );
    }

    #[test]
    fn a_file_with_a_nul_byte_is_not_text() {
        let dir = std::env::temp_dir().join(format!("nullradix-prompt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("binary.txt");
        std::fs::write(&path, b"a\0b").unwrap();
        assert!(
            read_text(&path)
                .unwrap_err()
                .to_string()
                .contains("not a text file")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
