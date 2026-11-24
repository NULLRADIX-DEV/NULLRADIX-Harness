use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::Violation;

/// Workspace crates a library may depend on and external crates it must not use.
struct Rule {
    package: &'static str,
    internal: &'static [&'static str],
    forbidden: &'static [&'static str],
}

const DOMAIN: &str = "nullradix-domain";
const APP: &str = "nullradix-app";
const OS: &str = "nullradix-os";

const RULES: &[Rule] = &[
    Rule {
        package: DOMAIN,
        internal: &[],
        forbidden: &["tokio", "sqlx", "axum", "reqwest", "serde_json"],
    },
    Rule {
        package: APP,
        internal: &[DOMAIN],
        forbidden: &["sqlx", "axum", "reqwest"],
    },
    Rule {
        package: "nullradix-store",
        internal: &[DOMAIN, APP],
        forbidden: &["axum", "reqwest"],
    },
    Rule {
        package: "nullradix-api",
        internal: &[DOMAIN, APP],
        forbidden: &["sqlx"],
    },
    Rule {
        package: "nullradix-runner",
        internal: &[DOMAIN, APP, OS],
        forbidden: &["sqlx", "axum"],
    },
    Rule {
        package: OS,
        internal: &[],
        forbidden: &["sqlx", "axum"],
    },
];

const NO_RULE: &str = "library crate without a layer rule in xtask";
const INTERNAL: &str = "depends on a workspace crate its layer must not use";
const FORBIDDEN: &str = "depends on an external crate its layer must not use";

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    kind: Option<String>,
}

pub(super) fn run(root: &Path) -> Result<Vec<Violation>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .context("failed to run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("unexpected cargo metadata output")?;
    Ok(check(root, &metadata))
}

fn check(root: &Path, metadata: &Metadata) -> Vec<Violation> {
    let mut violations = Vec::new();
    for package in &metadata.packages {
        let path = package
            .manifest_path
            .strip_prefix(root)
            .unwrap_or(&package.manifest_path)
            .to_path_buf();
        let is_library = path.starts_with("crates");
        let Some(rule) = RULES.iter().find(|rule| rule.package == package.name) else {
            if is_library {
                violations.push(violation(path, NO_RULE));
            }
            continue;
        };
        for dependency in package.dependencies.iter().filter(|d| d.kind.is_none()) {
            let name = dependency.name.as_str();
            if name.starts_with("nullradix-") && !rule.internal.contains(&name) {
                violations.push(violation(path.clone(), INTERNAL));
            } else if rule.forbidden.contains(&name) {
                violations.push(violation(path.clone(), FORBIDDEN));
            }
        }
    }
    violations
}

fn violation(path: PathBuf, message: &'static str) -> Violation {
    Violation {
        path,
        line: 1,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, manifest: &str, dependencies: &[(&str, Option<&str>)]) -> Package {
        Package {
            name: name.to_owned(),
            manifest_path: PathBuf::from(manifest),
            dependencies: dependencies
                .iter()
                .map(|(name, kind)| Dependency {
                    name: (*name).to_owned(),
                    kind: kind.map(str::to_owned),
                })
                .collect(),
        }
    }

    fn messages(packages: Vec<Package>) -> Vec<&'static str> {
        check(Path::new("/repo"), &Metadata { packages })
            .into_iter()
            .map(|v| v.message)
            .collect()
    }

    #[test]
    fn accepts_dependencies_that_follow_the_layers() {
        let packages = vec![
            package(
                APP,
                "/repo/crates/app/Cargo.toml",
                &[(DOMAIN, None), ("tokio", None)],
            ),
            package(
                "nullradix-cli",
                "/repo/apps/cli/Cargo.toml",
                &[("nullradix-api", None)],
            ),
        ];
        assert_eq!(messages(packages), Vec::<&str>::new());
    }

    #[test]
    fn rejects_inward_dependencies_on_outer_layers() {
        let packages = vec![package(
            DOMAIN,
            "/repo/crates/domain/Cargo.toml",
            &[(APP, None), ("tokio", None)],
        )];
        assert_eq!(messages(packages), vec![INTERNAL, FORBIDDEN]);
    }

    #[test]
    fn allows_anything_for_tests() {
        let packages = vec![package(
            APP,
            "/repo/crates/app/Cargo.toml",
            &[("nullradix-store", Some("dev")), ("sqlx", Some("dev"))],
        )];
        assert_eq!(messages(packages), Vec::<&str>::new());
    }

    #[test]
    fn requires_a_rule_for_every_library() {
        let packages = vec![package(
            "nullradix-extra",
            "/repo/crates/extra/Cargo.toml",
            &[],
        )];
        assert_eq!(messages(packages), vec![NO_RULE]);
    }
}
