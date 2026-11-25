//! Plays recorded scenarios against the real binaries and compares them with golden files.

mod daemon;
mod normalize;
mod runner;
mod scenario;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_json::Value;

pub use daemon::Binaries;
use scenario::Scenario;

/// Set to record golden files instead of comparing with them.
pub const ACCEPT: &str = "NULLRADIX_ACCEPT";
const SCENARIO: &str = ".scenario.json";
const GOLDEN: &str = ".golden.json";

/// Every scenario below `root` with a name relative to it, sorted.
pub fn scenarios(root: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    collect(root, root, &mut found)?;
    found.sort();
    Ok(found)
}

fn collect(root: &Path, dir: &Path, found: &mut Vec<(String, PathBuf)>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("cannot list {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, found)?;
        } else if let Some(name) = path
            .strip_prefix(root)?
            .to_string_lossy()
            .strip_suffix(SCENARIO)
        {
            found.push((name.replace('\\', "/"), path.clone()));
        }
    }
    Ok(())
}

/// Plays one scenario and checks, or with `NULLRADIX_ACCEPT` records, its golden file.
pub fn check(path: &Path, binaries: &Binaries) -> anyhow::Result<()> {
    let scenario = Scenario::load(path)?;
    let recording = runner::play(&scenario, binaries).with_context(|| scenario.title.clone())?;
    ensure_no_tokens(&recording)?;
    let golden = golden_path(path)?;
    if env::var_os(ACCEPT).is_some_and(|value| value == "1") {
        let text = serde_json::to_string_pretty(&recording)? + "\n";
        fs::write(&golden, text).with_context(|| format!("cannot write {}", golden.display()))?;
        return Ok(());
    }
    let text = fs::read_to_string(&golden)
        .with_context(|| format!("{} is missing; record it with {ACCEPT}=1", golden.display()))?;
    let expected: Vec<Value> = serde_json::from_str(&text)?;
    compare(&expected, &recording)
}

/// Refuses a recording that holds a Claude run token, so goldens never keep one.
fn ensure_no_tokens(recording: &[Value]) -> anyhow::Result<()> {
    let token = regex::Regex::new(r"sk-ant-oat[0-9]*-[A-Za-z0-9_-]{4,}")?;
    for (index, entry) in recording.iter().enumerate() {
        if token.is_match(&entry.to_string()) {
            bail!("step {} of the recording holds a run token", index + 1);
        }
    }
    Ok(())
}

fn golden_path(path: &Path) -> anyhow::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(SCENARIO))
        .context("not a scenario file")?;
    Ok(path.with_file_name(format!("{name}{GOLDEN}")))
}

fn compare(expected: &[Value], recorded: &[Value]) -> anyhow::Result<()> {
    for (index, (want, got)) in expected.iter().zip(recorded).enumerate() {
        if want != got {
            bail!(
                "step {} differs from the golden file\nexpected: {}\nrecorded: {}",
                index + 1,
                serde_json::to_string_pretty(want)?,
                serde_json::to_string_pretty(got)?
            );
        }
    }
    if expected.len() != recorded.len() {
        bail!(
            "the golden file has {} steps, the scenario recorded {}",
            expected.len(),
            recorded.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn names_goldens_after_scenarios_and_reports_the_first_difference() {
        let golden = golden_path(Path::new("a/runs.scenario.json")).unwrap();
        assert_eq!(golden, Path::new("a/runs.golden.json"));
        let one = vec![json!({ "name": "a" }), json!({ "name": "b" })];
        assert!(compare(&one, &one).is_ok());
        let other = vec![json!({ "name": "a" }), json!({ "name": "c" })];
        let error = compare(&one, &other).unwrap_err().to_string();
        assert!(error.starts_with("step 2 differs"), "{error}");
        assert!(compare(&one, &one[..1]).is_err());
    }

    #[test]
    fn recordings_with_a_run_token_are_refused() {
        let clean =
            vec![json!({ "stderr": "a run token starts with sk-ant-oat and has no spaces" })];
        assert!(ensure_no_tokens(&clean).is_ok());
        let leaking = vec![json!({ "request": { "body": { "token": "sk-ant-oat01-abcdef" } } })];
        let error = ensure_no_tokens(&leaking).unwrap_err().to_string();
        assert_eq!(error, "step 1 of the recording holds a run token");
    }
}
