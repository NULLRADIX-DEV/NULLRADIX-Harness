//! Rules for ticket labels, dependencies and text limits, ported 1:1 from the legacy board.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use crate::{TicketKey, utf16_len};

pub const LABEL_MAX: usize = 100;
/// How much of a too long label an error shows, as in legacy.
const LABEL_SHOWN: usize = 30;

/// The longest start of `text` within `units` UTF-16 code units, never splitting a character.
fn utf16_prefix(text: &str, units: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|letter| {
            used += letter.len_utf16();
            used <= units
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    /// Carries only the start of the label, so a huge one never floods a message.
    #[error("label {label}... is too long ({length} characters, at most {LABEL_MAX})")]
    LabelTooLong { label: String, length: usize },
    #[error("{0} cannot depend on itself")]
    SelfDependency(TicketKey),
    #[error("dependency {0} does not exist")]
    UnknownDependency(TicketKey),
    #[error("dependencies would run in a circle: {}", path(.0))]
    DependencyCycle(Vec<TicketKey>),
}

fn path(keys: &[TicketKey]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Splits on commas, trims, drops blanks and keeps the first spelling of each label.
pub fn normalize_labels<S: AsRef<str>>(inputs: &[S]) -> Result<Vec<String>, RuleError> {
    let mut seen = HashSet::new();
    let mut labels = Vec::new();
    for input in inputs {
        for part in input.as_ref().split(',') {
            let label = part.trim();
            if label.is_empty() || !seen.insert(label.to_lowercase()) {
                continue;
            }
            let length = utf16_len(label);
            if length > LABEL_MAX {
                return Err(RuleError::LabelTooLong {
                    label: utf16_prefix(label, LABEL_SHOWN),
                    length,
                });
            }
            labels.push(label.to_owned());
        }
    }
    Ok(labels)
}

/// Drops duplicates and keeps the order in which dependencies were given.
#[must_use]
pub fn normalize_dependencies(keys: Vec<TicketKey>) -> Vec<TicketKey> {
    let mut seen = HashSet::new();
    keys.into_iter()
        .filter(|key| seen.insert(key.clone()))
        .collect()
}

/// Checks the new dependencies of `ticket` against all others; `edges` maps a ticket to its own.
pub fn validate_dependencies<S: BuildHasher>(
    ticket: &TicketKey,
    dependencies: &[TicketKey],
    edges: &HashMap<TicketKey, Vec<TicketKey>, S>,
) -> Result<(), RuleError> {
    if dependencies.contains(ticket) {
        return Err(RuleError::SelfDependency(ticket.clone()));
    }
    if let Some(unknown) = dependencies.iter().find(|key| !edges.contains_key(*key)) {
        return Err(RuleError::UnknownDependency(unknown.clone()));
    }
    // a node whose walk missed the ticket once never reaches it, so one set serves all walks
    let mut visited = HashSet::new();
    for dependency in dependencies {
        if let Some(path) = path_back(dependency, ticket, edges, &mut visited) {
            let mut trail = vec![ticket.clone()];
            trail.extend(path);
            return Err(RuleError::DependencyCycle(trail));
        }
    }
    Ok(())
}

/// The way from `from` along dependencies to `target`, walked with a stack instead of recursion.
fn path_back<'a, S: BuildHasher>(
    from: &'a TicketKey,
    target: &TicketKey,
    edges: &'a HashMap<TicketKey, Vec<TicketKey>, S>,
    visited: &mut HashSet<TicketKey>,
) -> Option<Vec<TicketKey>> {
    if from == target {
        return Some(vec![from.clone()]);
    }
    if !visited.insert(from.clone()) {
        return None;
    }
    // each entry is a node on the current path and the index of its next dependency to try
    let mut stack: Vec<(&TicketKey, usize)> = vec![(from, 0)];
    while let Some(top) = stack.len().checked_sub(1) {
        let (node, next) = stack[top];
        let Some(child) = edges.get(node).and_then(|children| children.get(next)) else {
            stack.pop();
            continue;
        };
        stack[top].1 += 1;
        if child == target {
            let mut path: Vec<TicketKey> = stack.iter().map(|(key, _)| (*key).clone()).collect();
            path.push(child.clone());
            return Some(path);
        }
        if visited.insert(child.clone()) {
            stack.push((child, 0));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> TicketKey {
        TicketKey::parse(text).unwrap()
    }

    fn keys(texts: &[&str]) -> Vec<TicketKey> {
        texts.iter().map(|text| key(text)).collect()
    }

    #[test]
    fn labels_split_trim_and_keep_the_first_spelling() {
        let labels = normalize_labels(&["ui, Backend", " ", "UI,api,,backend"]).unwrap();
        assert_eq!(labels, ["ui", "Backend", "api"]);
    }

    #[test]
    fn too_long_labels_are_named_by_their_start_only() {
        let long = "\u{1f600}".repeat(51);
        assert_eq!(utf16_len(&long), 102);
        // 15 emoji fill 30 units; a 16th would split the limit
        assert_eq!(
            normalize_labels(&[long.as_str()]),
            Err(RuleError::LabelTooLong {
                label: "\u{1f600}".repeat(15),
                length: 102
            })
        );
        assert_eq!(utf16_prefix("abc", 30), "abc");
    }

    #[test]
    fn dependencies_drop_duplicates_in_order() {
        let given = vec![key("ARC-2"), key("arc-1"), key("ARC-0002")];
        assert_eq!(normalize_dependencies(given), keys(&["ARC-2", "ARC-1"]));
    }

    #[test]
    fn dependencies_refuse_self_before_unknown_and_circles_with_the_path() {
        let edges = HashMap::from([
            (key("ARC-1"), vec![]),
            (key("ARC-2"), keys(&["ARC-1"])),
            (key("ARC-3"), keys(&["ARC-2"])),
            (key("ARC-4"), vec![]),
        ]);
        assert_eq!(
            validate_dependencies(&key("ARC-1"), &keys(&["ARC-9", "ARC-1"]), &edges),
            Err(RuleError::SelfDependency(key("ARC-1")))
        );
        assert_eq!(
            validate_dependencies(&key("ARC-1"), &keys(&["ARC-9"]), &edges),
            Err(RuleError::UnknownDependency(key("ARC-9")))
        );
        let circle = validate_dependencies(&key("ARC-1"), &keys(&["ARC-4", "ARC-3"]), &edges);
        assert_eq!(
            circle,
            Err(RuleError::DependencyCycle(keys(&[
                "ARC-1", "ARC-3", "ARC-2", "ARC-1"
            ])))
        );
        assert_eq!(
            circle.unwrap_err().to_string(),
            "dependencies would run in a circle: ARC-0001 -> ARC-0003 -> ARC-0002 -> ARC-0001"
        );
        assert_eq!(
            validate_dependencies(&key("ARC-4"), &keys(&["ARC-3"]), &edges),
            Ok(())
        );
    }

    #[test]
    fn walks_end_on_diamonds_and_on_circles_elsewhere() {
        let edges = HashMap::from([
            (key("ARC-1"), vec![]),
            (key("ARC-2"), keys(&["ARC-1"])),
            (key("ARC-3"), keys(&["ARC-1"])),
            (key("ARC-5"), keys(&["ARC-6"])),
            (key("ARC-6"), keys(&["ARC-5"])),
            (key("ARC-7"), keys(&["ARC-2", "ARC-3", "ARC-5"])),
            (key("ARC-8"), vec![]),
        ]);
        assert_eq!(
            validate_dependencies(&key("ARC-8"), &keys(&["ARC-7", "ARC-5"]), &edges),
            Ok(())
        );
        assert_eq!(
            validate_dependencies(&key("ARC-1"), &keys(&["ARC-7"]), &edges),
            Err(RuleError::DependencyCycle(keys(&[
                "ARC-1", "ARC-7", "ARC-2", "ARC-1"
            ])))
        );
    }

    #[test]
    fn long_chains_do_not_exhaust_the_stack() {
        let length = 200_000;
        let mut edges: HashMap<TicketKey, Vec<TicketKey>> = HashMap::new();
        let project = crate::ProjectKey::parse("ARC").unwrap();
        for number in 1..=length {
            let below = if number == 1 {
                vec![]
            } else {
                vec![TicketKey::new(project.clone(), number - 1)]
            };
            edges.insert(TicketKey::new(project.clone(), number), below);
        }
        let top = TicketKey::new(project.clone(), length);
        let bottom = TicketKey::new(project, 1);
        let result = validate_dependencies(&bottom, &[top], &edges);
        assert!(matches!(&result, Err(RuleError::DependencyCycle(path)) if path.len() == 200_001));
    }
}
