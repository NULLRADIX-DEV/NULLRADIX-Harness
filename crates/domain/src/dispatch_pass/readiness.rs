use std::fmt;

use crate::{DispatcherConfig, TicketKey, TicketStatus};

use super::profile_choice::PROFILE_LABEL_PREFIX;

/// Where the board's repository stands on the machine the run would start on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryFact {
    /// The board points to no repository.
    Missing,
    /// The repository has no checkout on this machine.
    NoCheckout,
    /// The checkout folder is gone.
    FolderGone(String),
    Present,
}

/// What readiness looks at of a ticket and its surroundings.
#[derive(Debug, Clone)]
pub struct ReadinessFacts<'a> {
    pub status: TicketStatus,
    pub assignee: Option<&'a str>,
    pub needs_planning: bool,
    pub labels: &'a [String],
    pub plan: &'a str,
    pub handbook: &'a str,
    pub acceptance: &'a str,
    pub depends_open: &'a [TicketKey],
    /// Done dependencies that still carry a branch.
    pub depends_unmerged: &'a [TicketKey],
    pub open_questions: &'a [String],
    pub board_dispatch: bool,
    pub repository: RepositoryFact,
}

/// The first thing that keeps the dispatcher from taking a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReady {
    NotPlanned,
    Assigned(String),
    NeedsPlanning,
    OptedOut(String),
    OptInMissing(String),
    NoPlan,
    NoHandbook,
    NoAcceptance,
    WaitsFor(Vec<TicketKey>),
    WaitsForMerge(Vec<TicketKey>),
    WaitsForQuestions(Vec<String>),
    UnknownProfile { name: String, known: Vec<String> },
    BoardOff,
    NoRepository,
    NoCheckout,
    FolderGone(String),
}

fn keys(keys: &[TicketKey]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for NotReady {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPlanned => f.write_str("status is not planned"),
            Self::Assigned(name) => write!(f, "already assigned to {name}"),
            Self::NeedsPlanning => f.write_str("the planning flag is still set"),
            Self::OptedOut(label) => write!(f, "label \"{label}\""),
            Self::OptInMissing(label) => write!(f, "label \"{label}\" is missing"),
            Self::NoPlan => f.write_str("no plan"),
            Self::NoHandbook => f.write_str("no handbook"),
            Self::NoAcceptance => f.write_str("no acceptance criteria"),
            Self::WaitsFor(open) => write!(f, "waits for {}", keys(open)),
            Self::WaitsForMerge(open) => write!(f, "waits for the merge of {}", keys(open)),
            Self::WaitsForQuestions(open) => write!(f, "waits for question {}", open.join(", ")),
            Self::UnknownProfile { name, known } => write!(
                f,
                "label {PROFILE_LABEL_PREFIX}{name} - there is no such profile ({})",
                known.join(", ")
            ),
            Self::BoardOff => f.write_str("the dispatcher is off for the board"),
            Self::NoRepository => f.write_str("the board has no repository"),
            Self::NoCheckout => f.write_str("the repository has no checkout on this machine"),
            Self::FolderGone(path) => write!(f, "the repository folder {path} does not exist"),
        }
    }
}

fn has(labels: &[String], wanted: &str) -> bool {
    labels.iter().any(|label| label == wanted)
}

/// The first missing point in a fixed order, or `None` when the dispatcher may take the ticket.
#[must_use]
pub fn not_ready(config: &DispatcherConfig, facts: &ReadinessFacts<'_>) -> Option<NotReady> {
    if facts.status != TicketStatus::Planned {
        return Some(NotReady::NotPlanned);
    }
    if let Some(assignee) = facts.assignee {
        return Some(NotReady::Assigned(assignee.to_owned()));
    }
    if facts.needs_planning {
        return Some(NotReady::NeedsPlanning);
    }
    if let Some(label) = &config.opt_out_label
        && has(facts.labels, label)
    {
        return Some(NotReady::OptedOut(label.clone()));
    }
    if let Some(label) = &config.opt_in_label
        && !has(facts.labels, label)
    {
        return Some(NotReady::OptInMissing(label.clone()));
    }
    if facts.plan.trim().is_empty() {
        return Some(NotReady::NoPlan);
    }
    if facts.handbook.trim().is_empty() {
        return Some(NotReady::NoHandbook);
    }
    if facts.acceptance.trim().is_empty() {
        return Some(NotReady::NoAcceptance);
    }
    if !facts.depends_open.is_empty() {
        return Some(NotReady::WaitsFor(facts.depends_open.to_vec()));
    }
    // done is not enough: only a merged dependency has its code in the new branch
    if !facts.depends_unmerged.is_empty() {
        return Some(NotReady::WaitsForMerge(facts.depends_unmerged.to_vec()));
    }
    if !facts.open_questions.is_empty() {
        return Some(NotReady::WaitsForQuestions(facts.open_questions.to_vec()));
    }
    if let Some(name) = profile_label(facts.labels)
        && config.profile(name).is_none()
    {
        return Some(NotReady::UnknownProfile {
            name: name.to_owned(),
            known: config.profiles.iter().map(|p| p.name.clone()).collect(),
        });
    }
    if !facts.board_dispatch {
        return Some(NotReady::BoardOff);
    }
    match &facts.repository {
        RepositoryFact::Missing => Some(NotReady::NoRepository),
        RepositoryFact::NoCheckout => Some(NotReady::NoCheckout),
        RepositoryFact::FolderGone(path) => Some(NotReady::FolderGone(path.clone())),
        RepositoryFact::Present => None,
    }
}

/// The profile named by the first `profile:` label.
pub(super) fn profile_label(labels: &[String]) -> Option<&str> {
    labels
        .iter()
        .find_map(|label| label.strip_prefix(PROFILE_LABEL_PREFIX))
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> TicketKey {
        TicketKey::parse(text).unwrap()
    }

    struct Parts {
        labels: Vec<String>,
        open: Vec<TicketKey>,
        unmerged: Vec<TicketKey>,
        questions: Vec<String>,
    }

    fn parts() -> Parts {
        Parts {
            labels: vec![String::from("auto")],
            open: Vec::new(),
            unmerged: Vec::new(),
            questions: Vec::new(),
        }
    }

    fn facts(parts: &Parts) -> ReadinessFacts<'_> {
        ReadinessFacts {
            status: TicketStatus::Planned,
            assignee: None,
            needs_planning: false,
            labels: &parts.labels,
            plan: "Plan",
            handbook: "Handbook",
            acceptance: "Works",
            depends_open: &parts.open,
            depends_unmerged: &parts.unmerged,
            open_questions: &parts.questions,
            board_dispatch: true,
            repository: RepositoryFact::Present,
        }
    }

    fn config() -> DispatcherConfig {
        DispatcherConfig::default()
    }

    #[test]
    fn a_complete_opted_in_ticket_is_taken() {
        let parts = parts();
        assert_eq!(not_ready(&config(), &facts(&parts)), None);
    }

    #[test]
    fn the_first_missing_point_is_named_in_order() {
        let mut parts = parts();
        let config = config();
        let mut ticket = facts(&parts);
        ticket.status = TicketStatus::Backlog;
        ticket.assignee = Some("worker-ARC-1");
        assert_eq!(not_ready(&config, &ticket), Some(NotReady::NotPlanned));
        ticket.status = TicketStatus::Planned;
        assert_eq!(
            not_ready(&config, &ticket).unwrap().to_string(),
            "already assigned to worker-ARC-1"
        );
        ticket.assignee = None;
        ticket.needs_planning = true;
        assert_eq!(not_ready(&config, &ticket), Some(NotReady::NeedsPlanning));
        ticket.needs_planning = false;
        ticket.plan = " ";
        assert_eq!(not_ready(&config, &ticket), Some(NotReady::NoPlan));
        ticket.plan = "p";
        ticket.handbook = "";
        assert_eq!(not_ready(&config, &ticket), Some(NotReady::NoHandbook));
        ticket.handbook = "h";
        ticket.acceptance = "";
        assert_eq!(not_ready(&config, &ticket), Some(NotReady::NoAcceptance));
        parts.open = vec![key("ARC-2")];
        parts.unmerged = vec![key("ARC-3")];
        assert_eq!(
            not_ready(&config, &facts(&parts)).unwrap().to_string(),
            "waits for ARC-0002"
        );
        parts.open = Vec::new();
        assert_eq!(
            not_ready(&config, &facts(&parts)).unwrap().to_string(),
            "waits for the merge of ARC-0003"
        );
        parts.unmerged = Vec::new();
        parts.questions = vec![String::from("ARC-Q0001")];
        assert_eq!(
            not_ready(&config, &facts(&parts)).unwrap().to_string(),
            "waits for question ARC-Q0001"
        );
    }

    #[test]
    fn the_opt_labels_decide_before_the_content() {
        let mut parts = parts();
        let config = config();
        parts.labels = vec![String::from("docs")];
        assert_eq!(
            not_ready(&config, &facts(&parts)),
            Some(NotReady::OptInMissing(String::from("auto")))
        );
        parts.labels = vec![String::from("auto"), String::from("manual")];
        assert_eq!(
            not_ready(&config, &facts(&parts)),
            Some(NotReady::OptedOut(String::from("manual")))
        );
        let open = DispatcherConfig {
            opt_in_label: None,
            ..config
        };
        parts.labels = vec![String::from("docs")];
        assert_eq!(not_ready(&open, &facts(&parts)), None);
    }

    #[test]
    fn a_profile_label_must_name_a_profile() {
        let mut parts = parts();
        parts.labels = vec![String::from("auto"), String::from("profile:huge")];
        let reason = not_ready(&config(), &facts(&parts)).unwrap();
        assert_eq!(
            reason.to_string(),
            "label profile:huge - there is no such profile (small, medium, large)"
        );
        parts.labels = vec![String::from("auto"), String::from("profile:large")];
        assert_eq!(not_ready(&config(), &facts(&parts)), None);
    }

    #[test]
    fn the_repository_is_checked_last() {
        let parts = parts();
        let config = config();
        let mut ticket = facts(&parts);
        ticket.repository = RepositoryFact::Missing;
        assert_eq!(not_ready(&config, &ticket), Some(NotReady::NoRepository));
        ticket.repository = RepositoryFact::NoCheckout;
        assert_eq!(not_ready(&config, &ticket), Some(NotReady::NoCheckout));
        ticket.repository = RepositoryFact::FolderGone(String::from("/gone"));
        assert_eq!(
            not_ready(&config, &ticket).unwrap().to_string(),
            "the repository folder /gone does not exist"
        );
        ticket.board_dispatch = false;
        assert_eq!(not_ready(&config, &ticket), Some(NotReady::BoardOff));
    }
}
