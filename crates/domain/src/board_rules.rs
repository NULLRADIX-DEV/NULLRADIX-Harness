//! Rules for ticket labels, dependencies and text limits, ported 1:1 from the legacy board.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use crate::TicketKey;

/// Length as the legacy board counted it: UTF-16 code units, like JavaScript.
#[must_use]
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub const LABEL_MAX: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("label {label} is too long ({length} characters, at most {LABEL_MAX})")]
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
                    label: label.to_owned(),
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
    for dependency in dependencies {
        if dependency == ticket {
            return Err(RuleError::SelfDependency(ticket.clone()));
        }
        if !edges.contains_key(dependency) {
            return Err(RuleError::UnknownDependency(dependency.clone()));
        }
    }
    for dependency in dependencies {
        let mut trail = vec![ticket.clone()];
        let mut visited = HashSet::new();
        if leads_back(dependency, ticket, edges, &mut trail, &mut visited) {
            return Err(RuleError::DependencyCycle(trail));
        }
    }
    Ok(())
}

/// Depth-first walk from `current`; on success `trail` holds the circle from `target` back to it.
fn leads_back<S: BuildHasher>(
    current: &TicketKey,
    target: &TicketKey,
    edges: &HashMap<TicketKey, Vec<TicketKey>, S>,
    trail: &mut Vec<TicketKey>,
    visited: &mut HashSet<TicketKey>,
) -> bool {
    trail.push(current.clone());
    if current == target {
        return true;
    }
    if visited.insert(current.clone()) {
        for next in edges.get(current).into_iter().flatten() {
            if leads_back(next, target, edges, trail, visited) {
                return true;
            }
        }
    }
    trail.pop();
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> TicketKey {
        TicketKey::parse(text).unwrap()
    }

    #[test]
    fn labels_split_trim_and_keep_the_first_spelling() {
        let labels = normalize_labels(&["ui, Backend", " ", "UI,api,,backend"]).unwrap();
        assert_eq!(labels, ["ui", "Backend", "api"]);
        let long = "\u{1f600}".repeat(51);
        assert_eq!(utf16_len(&long), 102);
        assert_eq!(
            normalize_labels(&[long.as_str()]),
            Err(RuleError::LabelTooLong {
                label: long.clone(),
                length: 102
            })
        );
    }

    #[test]
    fn dependencies_drop_duplicates_in_order() {
        let keys = vec![key("ARC-2"), key("arc-1"), key("ARC-0002")];
        assert_eq!(normalize_dependencies(keys), [key("ARC-2"), key("ARC-1")]);
    }

    #[test]
    fn dependencies_refuse_self_unknown_and_circles_with_the_path() {
        let edges = HashMap::from([
            (key("ARC-1"), vec![]),
            (key("ARC-2"), vec![key("ARC-1")]),
            (key("ARC-3"), vec![key("ARC-2")]),
            (key("ARC-4"), vec![]),
        ]);
        assert_eq!(
            validate_dependencies(&key("ARC-1"), &[key("ARC-1")], &edges),
            Err(RuleError::SelfDependency(key("ARC-1")))
        );
        assert_eq!(
            validate_dependencies(&key("ARC-1"), &[key("ARC-9")], &edges),
            Err(RuleError::UnknownDependency(key("ARC-9")))
        );
        let circle = validate_dependencies(&key("ARC-1"), &[key("ARC-4"), key("ARC-3")], &edges);
        assert_eq!(
            circle,
            Err(RuleError::DependencyCycle(vec![
                key("ARC-1"),
                key("ARC-3"),
                key("ARC-2"),
                key("ARC-1")
            ]))
        );
        assert_eq!(
            circle.unwrap_err().to_string(),
            "dependencies would run in a circle: ARC-0001 -> ARC-0003 -> ARC-0002 -> ARC-0001"
        );
        assert_eq!(
            validate_dependencies(&key("ARC-4"), &[key("ARC-3")], &edges),
            Ok(())
        );
    }
}
