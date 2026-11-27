//! Rules for ticket labels, dependencies and text limits, ported 1:1 from the legacy board.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use crate::{Column, Priority, TicketKey, TicketStatus, utf16_len, utf16_prefix};

pub const LABEL_MAX: usize = 100;
/// How much of a too long label an error shows, as in legacy.
const LABEL_SHOWN: usize = 30;

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

/// Dependencies after a change: a full list wins, else `added` join `current`; `removed` go last.
#[must_use]
pub fn changed_dependencies(
    current: &[TicketKey],
    list: Option<Vec<TicketKey>>,
    added: Vec<TicketKey>,
    removed: &[TicketKey],
) -> Vec<TicketKey> {
    let base = list
        .unwrap_or_else(|| normalize_dependencies(current.iter().cloned().chain(added).collect()));
    base.into_iter()
        .filter(|key| !removed.contains(key))
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

/// Status, dependencies and dependents of every ticket, to tell what waits on what.
#[derive(Debug, Clone, Default)]
pub struct DependencyIndex {
    statuses: HashMap<TicketKey, TicketStatus>,
    edges: HashMap<TicketKey, Vec<TicketKey>>,
    dependents: HashMap<TicketKey, Vec<TicketKey>>,
}

impl DependencyIndex {
    pub fn new(
        tickets: impl IntoIterator<Item = (TicketKey, TicketStatus, Vec<TicketKey>)>,
    ) -> Self {
        let mut index = Self::default();
        for (key, status, depends_on) in tickets {
            for dependency in &depends_on {
                index
                    .dependents
                    .entry(dependency.clone())
                    .or_default()
                    .push(key.clone());
            }
            index.statuses.insert(key.clone(), status);
            index.edges.insert(key, depends_on);
        }
        // legacy sorted the key text, so ARC-10000 comes before ARC-9999
        for dependents in index.dependents.values_mut() {
            dependents.sort_by_cached_key(ToString::to_string);
        }
        index
    }

    /// Each ticket with its own dependencies, as [`validate_dependencies`] takes them.
    #[must_use]
    pub const fn edges(&self) -> &HashMap<TicketKey, Vec<TicketKey>> {
        &self.edges
    }

    /// Dependencies neither done nor not needed; deleted ones count as met.
    #[must_use]
    pub fn open_dependencies(&self, depends_on: &[TicketKey]) -> Vec<TicketKey> {
        depends_on
            .iter()
            .filter(|key| {
                self.statuses
                    .get(*key)
                    .is_some_and(|status| status.is_open())
            })
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn dependents(&self, key: &TicketKey) -> &[TicketKey] {
        self.dependents.get(key).map_or(&[], Vec::as_slice)
    }

    /// Dependents still ahead of the work whose only open dependency is `key`.
    #[must_use]
    pub fn unlocks(&self, key: &TicketKey) -> Vec<TicketKey> {
        self.dependents(key)
            .iter()
            .filter(|dependent| {
                matches!(
                    self.statuses.get(*dependent),
                    Some(TicketStatus::Backlog | TicketStatus::InPlanning | TicketStatus::Planned)
                ) && self
                    .edges
                    .get(*dependent)
                    .is_some_and(|edges| self.open_dependencies(edges) == [key.clone()])
            })
            .cloned()
            .collect()
    }
}

/// Sort key of the display order: column, priority, number, then the key.
#[must_use]
pub fn display_order(
    column: Column,
    priority: Priority,
    key: &TicketKey,
) -> (Column, u8, u32, String) {
    (column, priority.rank(), key.number(), key.to_string())
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
    fn a_full_list_wins_over_added_keys_and_removals_go_last() {
        let current = keys(&["ARC-1", "ARC-2"]);
        let changed = |list: Option<&[&str]>, added: &[&str], removed: &[&str]| {
            changed_dependencies(&current, list.map(keys), keys(added), &keys(removed))
        };
        assert_eq!(
            changed(None, &["ARC-3", "ARC-1"], &[]),
            keys(&["ARC-1", "ARC-2", "ARC-3"])
        );
        assert_eq!(changed(Some(&["ARC-9"]), &["ARC-3"], &[]), keys(&["ARC-9"]));
        assert_eq!(
            changed(None, &["ARC-3"], &["ARC-2", "ARC-3"]),
            keys(&["ARC-1"])
        );
        assert_eq!(
            changed(Some(&["ARC-9", "ARC-8"]), &[], &["ARC-9"]),
            keys(&["ARC-8"])
        );
        assert_eq!(changed(Some(&[]), &["ARC-3"], &[]), keys(&[]));
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

    #[test]
    fn the_index_tells_open_dependencies_dependents_and_unlocks() {
        use TicketStatus::{Backlog, Blocked, Done, InProgress, Planned};
        let index = DependencyIndex::new([
            (key("ARC-1"), InProgress, Vec::new()),
            (key("ARC-2"), Done, Vec::new()),
            (
                key("ARC-3"),
                Planned,
                vec![key("ARC-1"), key("ARC-2"), key("ARC-9")],
            ),
            (key("ARC-4"), Backlog, vec![key("ARC-1"), key("ARC-3")]),
            (key("ARC-5"), Blocked, vec![key("ARC-1")]),
        ]);
        assert_eq!(
            index.open_dependencies(&[key("ARC-1"), key("ARC-2"), key("ARC-9")]),
            [key("ARC-1")]
        );
        assert_eq!(
            index.dependents(&key("ARC-1")),
            [key("ARC-3"), key("ARC-4"), key("ARC-5")]
        );
        assert_eq!(index.unlocks(&key("ARC-1")), [key("ARC-3")]);
        assert_eq!(index.unlocks(&key("ARC-2")), Vec::<TicketKey>::new());
        assert_eq!(index.dependents(&key("ARC-5")), []);
        assert!(validate_dependencies(&key("ARC-1"), &[key("ARC-4")], index.edges()).is_err());
    }

    #[test]
    fn display_order_sorts_by_column_priority_and_number() {
        let mut keys = [
            display_order(Column::Planned, Priority::Low, &key("ARC-1")),
            display_order(Column::Waiting, Priority::Low, &key("ARC-5")),
            display_order(Column::Planned, Priority::Urgent, &key("ARC-9")),
            display_order(Column::Planned, Priority::Urgent, &key("ARC-3")),
        ];
        keys.sort();
        let numbers: Vec<u32> = keys.iter().map(|order| order.2).collect();
        assert_eq!(numbers, [5, 3, 9, 1]);
    }
}
