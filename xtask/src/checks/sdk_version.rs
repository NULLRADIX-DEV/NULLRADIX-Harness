use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::Violation;

const PACKAGE: &str = "sdk/theme/package.json";
const TS_VERSION: &str = "sdk/theme/src/version.ts";
const RUST_VERSION: &str = "crates/domain/src/theme.rs";
const TS_PREFIX: &str = "export const SDK_VERSION = '";
const RUST_PREFIX: &str = "pub const SDK_VERSION: &str = \"";

/// The text between `prefix` and the next closing quote on the line that starts with `prefix`.
fn quoted_after(source: &str, prefix: &str) -> Option<String> {
    let rest = source
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(prefix))?;
    let end = rest.find(['\'', '"'])?;
    Some(rest[..end].to_owned())
}

/// Reports every place whose SDK version differs from the package version.
fn mismatches(package: &str, ts: Option<&str>, rust: Option<&str>) -> Vec<String> {
    let mut messages = Vec::new();
    for (path, found) in [(TS_VERSION, ts), (RUST_VERSION, rust)] {
        match found {
            None => messages.push(format!("{path} has no SDK_VERSION constant")),
            Some(version) if version != package => messages.push(format!(
                "SDK_VERSION {version} in {path} differs from {package} in {PACKAGE}"
            )),
            Some(_) => {}
        }
    }
    messages
}

/// The SDK version of the package, the TS constant and the domain constant must be the same.
pub(super) fn run(root: &Path) -> Result<Vec<Violation>> {
    let read = |path: &str| {
        fs::read_to_string(root.join(path)).with_context(|| format!("failed to read {path}"))
    };
    let package: Value = serde_json::from_str(&read(PACKAGE)?)
        .with_context(|| format!("failed to parse {PACKAGE}"))?;
    let Some(version) = package["version"].as_str() else {
        return Ok(vec![violation(PACKAGE, "package.json has no version")]);
    };
    let ts = quoted_after(&read(TS_VERSION)?, TS_PREFIX);
    let rust = quoted_after(&read(RUST_VERSION)?, RUST_PREFIX);
    Ok(mismatches(version, ts.as_deref(), rust.as_deref())
        .iter()
        .map(|message| violation(PACKAGE, message))
        .collect())
}

fn violation(path: &str, message: &str) -> Violation {
    Violation {
        path: PathBuf::from(path),
        line: 1,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_constants() {
        assert_eq!(
            quoted_after("// c\nexport const SDK_VERSION = '1.3.0';\n", TS_PREFIX).as_deref(),
            Some("1.3.0")
        );
        assert_eq!(
            quoted_after("pub const SDK_VERSION: &str = \"1.3.0\";", RUST_PREFIX).as_deref(),
            Some("1.3.0")
        );
        assert_eq!(
            quoted_after("const OTHER: &str = \"1\";", RUST_PREFIX),
            None
        );
    }

    #[test]
    fn accepts_equal_versions() {
        assert_eq!(
            mismatches("1.3.0", Some("1.3.0"), Some("1.3.0")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn reports_a_differing_constant() {
        let found = mismatches("1.3.0", Some("1.3.0"), Some("1.4.0"));
        assert_eq!(found.len(), 1);
        assert!(found[0].contains(RUST_VERSION));
    }

    #[test]
    fn reports_a_missing_constant() {
        assert_eq!(mismatches("1.3.0", None, Some("1.3.0")).len(), 1);
    }
}
