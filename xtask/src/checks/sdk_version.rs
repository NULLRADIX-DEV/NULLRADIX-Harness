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

/// Line (1-based) and text between `prefix` and the closing quote of the line starting with it.
fn quoted_after(source: &str, prefix: &str) -> Option<(usize, String)> {
    source.lines().enumerate().find_map(|(index, line)| {
        let rest = line.trim_start().strip_prefix(prefix)?;
        let end = rest.find(['\'', '"'])?;
        Some((index + 1, rest[..end].to_owned()))
    })
}

/// Reports every place whose SDK version differs from the package version.
fn mismatches(
    package: &str,
    found: [(&'static str, Option<(usize, String)>); 2],
) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (path, constant) in found {
        let (line, message) = match constant {
            None => (1, format!("{path} has no SDK_VERSION constant")),
            Some((line, version)) if version != package => (
                line,
                format!("SDK_VERSION {version} differs from {package} in {PACKAGE}"),
            ),
            Some(_) => continue,
        };
        violations.push(Violation {
            path: PathBuf::from(path),
            line,
            message,
        });
    }
    violations
}

/// The SDK version of the package, the TS constant and the domain constant must be the same.
pub(super) fn run(root: &Path) -> Result<Vec<Violation>> {
    let read = |path: &str| {
        fs::read_to_string(root.join(path)).with_context(|| format!("failed to read {path}"))
    };
    let package: Value = serde_json::from_str(&read(PACKAGE)?)
        .with_context(|| format!("failed to parse {PACKAGE}"))?;
    let Some(version) = package["version"].as_str() else {
        return Ok(vec![Violation {
            path: PathBuf::from(PACKAGE),
            line: 1,
            message: String::from("package.json has no version"),
        }]);
    };
    let found = [
        (TS_VERSION, quoted_after(&read(TS_VERSION)?, TS_PREFIX)),
        (
            RUST_VERSION,
            quoted_after(&read(RUST_VERSION)?, RUST_PREFIX),
        ),
    ];
    Ok(mismatches(version, found))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(
        ts: Option<(usize, &str)>,
        rust: Option<(usize, &str)>,
    ) -> [(&'static str, Option<(usize, String)>); 2] {
        let owned =
            |value: Option<(usize, &str)>| value.map(|(line, text)| (line, text.to_owned()));
        [(TS_VERSION, owned(ts)), (RUST_VERSION, owned(rust))]
    }

    #[test]
    fn reads_both_constants_with_their_line() {
        assert_eq!(
            quoted_after(
                "// c
export const SDK_VERSION = '1.3.0';
",
                TS_PREFIX
            ),
            Some((2, String::from("1.3.0")))
        );
        assert_eq!(
            quoted_after("pub const SDK_VERSION: &str = \"1.3.0\";", RUST_PREFIX),
            Some((1, String::from("1.3.0")))
        );
        assert_eq!(
            quoted_after("const OTHER: &str = \"1\";", RUST_PREFIX),
            None
        );
    }

    #[test]
    fn accepts_equal_versions() {
        let equal = found(Some((2, "1.3.0")), Some((35, "1.3.0")));
        assert_eq!(mismatches("1.3.0", equal), Vec::new());
    }

    #[test]
    fn reports_a_differing_constant_where_it_is() {
        let violations = mismatches("1.3.0", found(Some((2, "1.3.0")), Some((35, "1.4.0"))));
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path, PathBuf::from(RUST_VERSION));
        assert_eq!(violations[0].line, 35);
    }

    #[test]
    fn reports_a_missing_constant() {
        assert_eq!(
            mismatches("1.3.0", found(None, Some((35, "1.3.0")))).len(),
            1
        );
    }
}
