use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::Violation;

const CONFIG: &str = "release-please-config.json";
const LOCKS: &[&str] = &["Cargo.lock", "apps/desktop/Cargo.lock"];
const SELECTOR: &str = "@.name.value=='";

/// Names of lockfile packages that come from this repository (no registry source).
fn local_packages(lock: &str) -> Vec<String> {
    lock.split("[[package]]")
        .skip(1)
        .filter(|block| !block.lines().any(|line| line.starts_with("source = ")))
        .filter_map(|block| {
            let line = block.lines().find(|line| line.starts_with("name = \""))?;
            Some(
                line.trim_start_matches("name = \"")
                    .trim_end_matches('"')
                    .to_owned(),
            )
        })
        .collect()
}

/// Package names the config bumps in the given lockfile.
fn bumped(config: &Value, lock_path: &str) -> Vec<String> {
    let files = config["packages"]["."]["extra-files"].as_array();
    files
        .into_iter()
        .flatten()
        .filter(|file| file["path"] == lock_path)
        .filter_map(|file| {
            let path = file["jsonpath"].as_str()?;
            let rest = &path[path.find(SELECTOR)? + SELECTOR.len()..];
            Some(rest[..rest.find('\'')?].to_owned())
        })
        .collect()
}

fn missing(config: &Value, lock_path: &str, lock: &str) -> Vec<String> {
    let bumped = bumped(config, lock_path);
    local_packages(lock)
        .into_iter()
        .filter(|name| !bumped.contains(name))
        .collect()
}

/// Every workspace crate in a lockfile must be bumped by release-please, or --locked builds fail.
pub(super) fn run(root: &Path) -> Result<Vec<Violation>> {
    let config_path = root.join(CONFIG);
    if !config_path.is_file() {
        return Ok(Vec::new());
    }
    let config: Value = serde_json::from_str(
        &fs::read_to_string(&config_path).with_context(|| format!("failed to read {CONFIG}"))?,
    )
    .with_context(|| format!("failed to parse {CONFIG}"))?;
    let mut violations = Vec::new();
    for lock_path in LOCKS {
        let lock = fs::read_to_string(root.join(lock_path))
            .with_context(|| format!("failed to read {lock_path}"))?;
        for name in missing(&config, lock_path, &lock) {
            violations.push(Violation {
                path: PathBuf::from(CONFIG),
                line: 1,
                message: format!("{lock_path} entry of {name} is not bumped by release-please"),
            });
        }
    }
    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = "[[package]]\nname = \"nullradix-api\"\nversion = \"0.1.0\"\n\n\
        [[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\n\n\
        [[package]]\nname = \"xtask\"\nversion = \"0.1.0\"\n";

    fn config(names: &[&str]) -> Value {
        let files: Vec<Value> = names
            .iter()
            .map(|name| {
                serde_json::json!({
                    "type": "toml",
                    "path": "Cargo.lock",
                    "jsonpath": format!("$.package[?(@.name.value=='{name}')].version"),
                })
            })
            .collect();
        serde_json::json!({ "packages": { ".": { "extra-files": files } } })
    }

    #[test]
    fn lists_only_repository_packages() {
        assert_eq!(local_packages(LOCK), ["nullradix-api", "xtask"]);
    }

    #[test]
    fn accepts_a_fully_covered_lock() {
        assert!(missing(&config(&["nullradix-api", "xtask"]), "Cargo.lock", LOCK).is_empty());
    }

    #[test]
    fn reports_a_crate_without_entry() {
        assert_eq!(
            missing(&config(&["nullradix-api"]), "Cargo.lock", LOCK),
            ["xtask"]
        );
    }

    #[test]
    fn ignores_entries_of_another_lock() {
        assert_eq!(
            missing(
                &config(&["nullradix-api", "xtask"]),
                "apps/desktop/Cargo.lock",
                LOCK
            )
            .len(),
            2
        );
    }

    #[test]
    fn rejects_the_untagged_selector() {
        let config = serde_json::json!({ "packages": { ".": { "extra-files": [{
            "path": "Cargo.lock",
            "jsonpath": "$.package[?(@.name=='xtask')].version",
        }] } } });
        assert_eq!(missing(&config, "Cargo.lock", LOCK).len(), 2);
    }
}
