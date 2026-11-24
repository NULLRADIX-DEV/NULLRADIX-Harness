use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;

/// A story told against a fresh daemon; its recording is compared with the golden file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scenario {
    pub(crate) title: String,
    /// Environment of the daemon and the CLI, such as `FAKE_CLAUDE_MODE`.
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Step {
    pub(crate) name: String,
    #[serde(flatten)]
    pub(crate) action: Action,
    /// Values to keep for later steps: `json.<path>` of a response or `stdout`.
    #[serde(default)]
    pub(crate) save: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Action {
    Http(Http),
    Cli(Cli),
    Wait(Wait),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Auth {
    #[default]
    Token,
    None,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Http {
    pub(crate) method: String,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) body: Option<Value>,
    #[serde(default)]
    pub(crate) auth: Auth,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Cli {
    pub(crate) args: Vec<String>,
    #[serde(default)]
    pub(crate) stdin: Option<Value>,
    /// Folder to run in; the scenario's work folder when left out.
    #[serde(default)]
    pub(crate) cwd: Option<String>,
}

/// Polls `path` until the JSON value at `field` equals `equals`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Wait {
    pub(crate) path: String,
    pub(crate) field: String,
    pub(crate) equals: Value,
    #[serde(default = "default_timeout")]
    pub(crate) timeout_ms: u64,
}

const fn default_timeout() -> u64 {
    20_000
}

impl Scenario {
    pub(crate) fn load(path: &Path) -> anyhow::Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("invalid scenario {}", path.display()))
    }
}
