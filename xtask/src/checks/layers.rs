use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::Violation;

/// Workspace crates a library may depend on and external crates it must not reach.
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
        package: "nullradix-client",
        internal: &["nullradix-api"],
        forbidden: &["sqlx"],
    },
    Rule {
        package: OS,
        internal: &[],
        forbidden: &["sqlx", "axum"],
    },
];

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    workspace_root: PathBuf,
    resolve: Resolve,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<NodeDependency>,
}

#[derive(Deserialize)]
struct NodeDependency {
    pkg: String,
    dep_kinds: Vec<DependencyKind>,
}

#[derive(Deserialize)]
struct DependencyKind {
    kind: Option<String>,
}

fn is_dev(kind: Option<&str>) -> bool {
    kind == Some("dev")
}

pub(super) fn run(root: &Path) -> Result<Vec<Violation>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked"])
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
    Ok(check(&metadata))
}

fn check(metadata: &Metadata) -> Vec<Violation> {
    let names: HashMap<&str, &str> = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package.name.as_str()))
        .collect();
    let mut violations = Vec::new();
    for package in metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
    {
        let Ok(path) = package.manifest_path.strip_prefix(&metadata.workspace_root) else {
            violations.push(violation(
                &package.manifest_path,
                "manifest outside the workspace",
            ));
            continue;
        };
        let Some(rule) = RULES.iter().find(|rule| rule.package == package.name) else {
            if path.components().next() == Some(Component::Normal("crates".as_ref())) {
                violations.push(violation(
                    path,
                    "library crate without a layer rule in xtask",
                ));
            }
            continue;
        };
        for dependency in &package.dependencies {
            let name = dependency.name.as_str();
            if !is_dev(dependency.kind.as_deref())
                && name.starts_with("nullradix-")
                && !rule.internal.contains(&name)
            {
                violations.push(violation(path, &format!("must not depend on {name}")));
            }
        }
        let mut reached: Vec<&str> = reachable(&metadata.resolve, &package.id)
            .into_iter()
            .filter_map(|id| names.get(id).copied())
            .filter(|name| rule.forbidden.contains(name))
            .collect();
        reached.sort_unstable();
        for name in reached {
            violations.push(violation(path, &format!("must not reach {name}")));
        }
    }
    violations
}

/// Packages reachable from `start` through normal and build dependencies.
fn reachable<'a>(resolve: &'a Resolve, start: &str) -> HashSet<&'a str> {
    let nodes: HashMap<&str, &Node> = resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut seen = HashSet::new();
    let mut pending = vec![start];
    while let Some(id) = pending.pop() {
        let Some(node) = nodes.get(id) else {
            continue;
        };
        for dependency in &node.deps {
            let runtime = dependency
                .dep_kinds
                .iter()
                .any(|kind| !is_dev(kind.kind.as_deref()));
            if runtime && seen.insert(dependency.pkg.as_str()) {
                pending.push(dependency.pkg.as_str());
            }
        }
    }
    seen
}

fn violation(path: &Path, message: &str) -> Violation {
    Violation {
        path: path.to_path_buf(),
        line: 1,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Graph {
        metadata: Metadata,
    }

    impl Graph {
        fn new() -> Self {
            Self {
                metadata: Metadata {
                    packages: Vec::new(),
                    workspace_members: Vec::new(),
                    workspace_root: PathBuf::from("/repo"),
                    resolve: Resolve { nodes: Vec::new() },
                },
            }
        }

        fn member(mut self, name: &str, dir: &str, deps: &[(&str, Option<&str>)]) -> Self {
            self.metadata.workspace_members.push(name.to_owned());
            self.add(name, &format!("/repo/{dir}/Cargo.toml"), deps)
        }

        fn external(self, name: &str, deps: &[(&str, Option<&str>)]) -> Self {
            self.add(name, "/registry/Cargo.toml", deps)
        }

        fn add(mut self, name: &str, manifest: &str, deps: &[(&str, Option<&str>)]) -> Self {
            self.metadata.packages.push(Package {
                id: name.to_owned(),
                name: name.to_owned(),
                manifest_path: PathBuf::from(manifest),
                dependencies: deps
                    .iter()
                    .map(|(dep, kind)| Dependency {
                        name: (*dep).to_owned(),
                        kind: kind.map(str::to_owned),
                    })
                    .collect(),
            });
            self.metadata.resolve.nodes.push(Node {
                id: name.to_owned(),
                deps: deps
                    .iter()
                    .map(|(dep, kind)| NodeDependency {
                        pkg: (*dep).to_owned(),
                        dep_kinds: vec![DependencyKind {
                            kind: kind.map(str::to_owned),
                        }],
                    })
                    .collect(),
            });
            self
        }

        fn messages(&self) -> Vec<String> {
            check(&self.metadata)
                .into_iter()
                .map(|v| v.message)
                .collect()
        }
    }

    #[test]
    fn accepts_dependencies_that_follow_the_layers() {
        let graph = Graph::new()
            .member(DOMAIN, "crates/domain", &[])
            .member(APP, "crates/app", &[(DOMAIN, None), ("tokio", None)])
            .member("nullradix-cli", "apps/cli", &[(APP, None)])
            .external("tokio", &[]);
        assert_eq!(graph.messages(), Vec::<String>::new());
    }

    #[test]
    fn rejects_dependencies_on_outer_layers() {
        let graph = Graph::new().member(APP, "crates/app", &[]).member(
            DOMAIN,
            "crates/domain",
            &[(APP, None)],
        );
        assert_eq!(graph.messages(), vec!["must not depend on nullradix-app"]);
    }

    #[test]
    fn rejects_forbidden_crates_reached_transitively() {
        let graph = Graph::new()
            .member(DOMAIN, "crates/domain", &[("helper", None)])
            .external("helper", &[("tokio", None)])
            .external("tokio", &[]);
        assert_eq!(graph.messages(), vec!["must not reach tokio"]);
    }

    #[test]
    fn ignores_dev_dependencies() {
        let graph = Graph::new()
            .member(
                APP,
                "crates/app",
                &[("nullradix-store", Some("dev")), ("sqlx", Some("dev"))],
            )
            .external("sqlx", &[]);
        assert_eq!(graph.messages(), Vec::<String>::new());
    }

    #[test]
    fn requires_a_rule_for_every_library() {
        let graph = Graph::new().member("nullradix-extra", "crates/extra", &[]);
        assert_eq!(
            graph.messages(),
            vec!["library crate without a layer rule in xtask"]
        );
    }
}
