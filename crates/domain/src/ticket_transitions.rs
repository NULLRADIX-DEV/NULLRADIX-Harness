//! State rules of a ticket: each takes the current state and a request and returns the next state.

use crate::{
    BLOCKED_REASON_MAX, Board, Id, StatusFacts, StatusLock, Ticket, TicketError, TicketKey,
    TicketRecord, TicketStatus, Timestamp, optional_text, strip_fence,
};

/// Why a claim for an agent is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimError {
    #[error("{key} is already assigned to {assignee}; take the next free ticket")]
    Assigned { key: TicketKey, assignee: String },
    #[error(transparent)]
    Locked(#[from] StatusLock),
}

/// The ticket is not blocked on a person.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{key} waits for no acceptance; only blocked with \"Human Interaction: ...\"")]
pub struct AcceptError {
    pub key: TicketKey,
}

/// Another agent plans the ticket right now.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{owner} is planning {key} right now")]
pub struct PlanningClaimed {
    pub owner: String,
    pub key: TicketKey,
}

/// Why a ticket cannot go to another board.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BoardMoveError {
    #[error("{key} is on board {board} already")]
    AlreadyThere { key: TicketKey, board: String },
    #[error("board {0} belongs to another project; tickets move only within one")]
    OtherProject(Id<Board>),
}

/// What a plan writes; a text left out stays as it is.
#[derive(Debug, Clone, Copy)]
pub struct PlanChange<'a> {
    pub plan: Option<&'a str>,
    pub handbook: Option<&'a str>,
    pub diagram: Option<&'a str>,
    pub acceptance: Option<&'a str>,
    pub status: TicketStatus,
    pub by: &'a str,
    pub at: Timestamp,
}

impl TicketRecord {
    /// Whether the next state sits in another column and so goes to the end of it.
    #[must_use]
    pub fn changes_column(&self, after: &Self) -> bool {
        after.status != self.status || after.board != self.board
    }

    /// The block reason after a change; leaving blocked drops it unless a new one is given.
    pub fn blocked_reason_after(
        &self,
        status: TicketStatus,
        given: Option<Option<&str>>,
    ) -> Result<Option<String>, TicketError> {
        match given {
            Some(Some(text)) => optional_text(text, BLOCKED_REASON_MAX)
                .map(|text| text.map(str::to_owned))
                .map_err(TicketError::BlockedReason),
            Some(None) => Ok(None),
            None if self.status == TicketStatus::Blocked && status != TicketStatus::Blocked => {
                Ok(None)
            }
            None => Ok(self.blocked_reason.clone()),
        }
    }

    /// The state on `target`, which must be another board of the same project.
    pub fn on_board(&self, target: &Board) -> Result<Self, BoardMoveError> {
        if self.board == target.id() {
            return Err(BoardMoveError::AlreadyThere {
                key: self.key.clone(),
                board: target.name().as_str().to_owned(),
            });
        }
        if self.project != target.project() {
            return Err(BoardMoveError::OtherProject(target.id()));
        }
        Ok(Self {
            board: target.id(),
            ..self.clone()
        })
    }
}

impl Ticket {
    /// Moves to a column; the reason of blocked stays only in blocked.
    #[must_use]
    pub fn moved(&self, status: TicketStatus, acceptance: Option<&str>) -> TicketRecord {
        let mut after = self.record().clone();
        after.status = status;
        if let Some(acceptance) = acceptance {
            acceptance.clone_into(&mut after.acceptance);
        }
        if status != TicketStatus::Blocked {
            after.blocked_reason = None;
        }
        after
    }

    /// Hands the ticket to an agent; `facts` come into play unless the claim is forced.
    pub fn claimed(
        &self,
        assignee: String,
        status: TicketStatus,
        force: bool,
        facts: &StatusFacts,
    ) -> Result<TicketRecord, ClaimError> {
        let record = self.record();
        // even the same name: several sessions often share one, only this keeps a claim exclusive
        if let Some(current) = &record.assignee
            && !force
        {
            return Err(ClaimError::Assigned {
                key: record.key.clone(),
                assignee: current.clone(),
            });
        }
        if !force {
            if status == TicketStatus::NotNeeded {
                return Err(StatusLock::NotNeededByPeople.into());
            }
            if !facts.open_dependencies.is_empty() {
                return Err(StatusLock::DependencyOpen(facts.open_dependencies.clone()).into());
            }
            if !facts.open_questions.is_empty() {
                return Err(StatusLock::QuestionOpen(facts.open_questions.clone()).into());
            }
        }
        let mut after = record.clone();
        after.assignee = Some(assignee);
        after.status = status;
        Ok(after)
    }

    /// Clears the assignee; a given status applies without any lock.
    #[must_use]
    pub fn released(&self, status: Option<TicketStatus>) -> TicketRecord {
        let mut after = self.record().clone();
        after.assignee = None;
        after.status = status.unwrap_or(after.status);
        after
    }

    /// Finishes a ticket that is blocked on a person.
    pub fn accepted(&self) -> Result<TicketRecord, AcceptError> {
        let record = self.record();
        if record.status != TicketStatus::Blocked
            || !crate::waits_for_a_person(record.blocked_reason.as_deref())
        {
            return Err(AcceptError {
                key: record.key.clone(),
            });
        }
        let mut after = record.clone();
        after.status = TicketStatus::Done;
        after.blocked_reason = None;
        Ok(after)
    }

    /// Writes the plan; the planning claim ends and the ticket no longer needs planning.
    #[must_use]
    pub fn planned(&self, change: &PlanChange<'_>) -> TicketRecord {
        let mut after = self.record().clone();
        if let Some(plan) = change.plan {
            plan.clone_into(&mut after.plan);
        }
        if let Some(handbook) = change.handbook {
            handbook.clone_into(&mut after.handbook);
        }
        if let Some(diagram) = change.diagram {
            after.diagram = strip_fence(diagram);
        }
        if let Some(acceptance) = change.acceptance.filter(|text| !text.trim().is_empty()) {
            acceptance.clone_into(&mut after.acceptance);
        }
        after.status = change.status;
        after.needs_planning = false;
        after.planning_claim = None;
        after.planned = Some((change.by.to_owned(), change.at));
        after
    }

    /// Starts a planning claim for `agent`; another agent's live claim refuses it.
    pub fn planning_claimed(
        &self,
        agent: String,
        now: Timestamp,
    ) -> Result<TicketRecord, PlanningClaimed> {
        let record = self.record();
        if let Some((owner, _)) = &record.planning_claim
            && *owner != agent
            && crate::planning_claim_alive(record, now)
        {
            return Err(PlanningClaimed {
                owner: owner.clone(),
                key: record.key.clone(),
            });
        }
        let mut after = record.clone();
        after.planning_claim = Some((agent, now));
        Ok(after)
    }

    /// Ends the planning claim of `agent`; none when someone else holds it or nobody does.
    #[must_use]
    pub fn planning_released(&self, agent: &str) -> Option<TicketRecord> {
        let record = self.record();
        let owner = record
            .planning_claim
            .as_ref()
            .map(|(owner, _)| owner.as_str());
        (owner == Some(agent)).then(|| TicketRecord {
            planning_claim: None,
            ..record.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        BoardName, HUMAN_INTERACTION, Priority, TicketDraft, TicketTitle, Version,
        planning_claim_alive,
    };

    fn id<T>(value: u128) -> Id<T> {
        Id::from_uuid(uuid::Uuid::from_u128(value))
    }

    fn board(board: u128, project: u128, name: &str) -> Board {
        Board::new(
            id(board),
            id(project),
            BoardName::parse(name).unwrap(),
            id(3),
            Timestamp::from_millis(5),
        )
    }

    fn ticket(status: TicketStatus) -> Ticket {
        let draft = TicketDraft {
            title: TicketTitle::parse("Fix it").unwrap(),
            description: String::new(),
            acceptance: String::new(),
            status,
            priority: Priority::Normal,
            labels: Vec::new(),
            depends_on: Vec::new(),
            needs_planning: true,
            target_branch: None,
        };
        Ticket::new(
            id(4),
            TicketKey::parse("ARC-7").unwrap(),
            &board(1, 2, "Main"),
            draft,
            0,
            id(3),
            Timestamp::from_millis(9),
        )
    }

    fn blocked(reason: &str) -> Ticket {
        let mut record = ticket(TicketStatus::Blocked).record().clone();
        record.blocked_reason = Some(reason.to_owned());
        Ticket::restore(record)
    }

    fn key(text: &str) -> TicketKey {
        TicketKey::parse(text).unwrap()
    }

    #[test]
    fn moving_out_of_blocked_drops_the_reason_and_staying_keeps_it() {
        let ticket = blocked("Merge: conflict");
        let planned = ticket.moved(TicketStatus::Planned, None);
        assert_eq!(planned.status, TicketStatus::Planned);
        assert_eq!(planned.blocked_reason, None);
        let still = ticket.moved(TicketStatus::Blocked, None);
        assert_eq!(still.blocked_reason.as_deref(), Some("Merge: conflict"));
    }

    #[test]
    fn a_move_writes_the_acceptance_only_when_given() {
        let ticket = ticket(TicketStatus::Backlog);
        assert_eq!(
            ticket
                .moved(TicketStatus::Planned, Some("it works"))
                .acceptance,
            "it works"
        );
        assert_eq!(ticket.moved(TicketStatus::Planned, None).acceptance, "");
        assert_eq!(ticket.moved(TicketStatus::Planned, Some("")).acceptance, "");
    }

    #[test]
    fn a_patch_without_a_reason_drops_it_when_leaving_blocked() {
        let record = blocked("Merge: conflict").record().clone();
        let leave = |status, given| record.blocked_reason_after(status, given);
        assert_eq!(leave(TicketStatus::Planned, None), Ok(None));
        assert_eq!(
            leave(TicketStatus::Blocked, None),
            Ok(Some(String::from("Merge: conflict")))
        );
        assert_eq!(
            leave(TicketStatus::Planned, Some(Some("  later "))),
            Ok(Some(String::from("later")))
        );
        assert_eq!(leave(TicketStatus::Blocked, Some(None)), Ok(None));
        assert_eq!(leave(TicketStatus::Blocked, Some(Some("  "))), Ok(None));
    }

    #[test]
    fn a_reason_stays_on_a_ticket_that_was_not_blocked() {
        let mut record = ticket(TicketStatus::Planned).record().clone();
        record.blocked_reason = Some(String::from("odd"));
        assert_eq!(
            record.blocked_reason_after(TicketStatus::Done, None),
            Ok(Some(String::from("odd")))
        );
    }

    #[test]
    fn a_too_long_reason_is_refused() {
        let record = ticket(TicketStatus::Blocked).record().clone();
        let long = "x".repeat(BLOCKED_REASON_MAX + 1);
        assert!(matches!(
            record.blocked_reason_after(TicketStatus::Blocked, Some(Some(&long))),
            Err(TicketError::BlockedReason(_))
        ));
    }

    #[test]
    fn only_a_new_status_or_board_goes_to_the_end_of_a_column() {
        let before = ticket(TicketStatus::Backlog).record().clone();
        assert!(!before.changes_column(&before));
        let mut next = before.clone();
        next.status = TicketStatus::Planned;
        assert!(before.changes_column(&next));
        let mut next = before.clone();
        next.board = id(9);
        assert!(before.changes_column(&next));
        let mut next = before.clone();
        next.priority = Priority::High;
        assert!(!before.changes_column(&next));
    }

    #[test]
    fn tickets_move_only_to_another_board_of_their_project() {
        let record = ticket(TicketStatus::Backlog).record().clone();
        let next = record.on_board(&board(10, 2, "Other")).unwrap();
        assert_eq!(next.board, id(10));
        assert_eq!(next.status, record.status);
        assert_eq!(
            record.on_board(&board(1, 2, "Main")),
            Err(BoardMoveError::AlreadyThere {
                key: key("ARC-7"),
                board: String::from("Main"),
            })
        );
        assert_eq!(
            record.on_board(&board(11, 5, "Foreign")),
            Err(BoardMoveError::OtherProject(id(11)))
        );
    }

    #[test]
    fn a_claim_sets_the_assignee_and_status() {
        let ticket = ticket(TicketStatus::Planned);
        let after = ticket
            .claimed(
                String::from("claude"),
                TicketStatus::InProgress,
                false,
                &StatusFacts::default(),
            )
            .unwrap();
        assert_eq!(after.assignee.as_deref(), Some("claude"));
        assert_eq!(after.status, TicketStatus::InProgress);
    }

    #[test]
    fn a_claim_is_exclusive_unless_forced() {
        let mut record = ticket(TicketStatus::Planned).record().clone();
        record.assignee = Some(String::from("claude"));
        let ticket = Ticket::restore(record);
        let facts = StatusFacts::default();
        for force in [false, true] {
            let result = ticket.claimed(
                String::from("claude"),
                TicketStatus::InProgress,
                force,
                &facts,
            );
            if force {
                assert!(result.is_ok());
            } else {
                assert_eq!(
                    result,
                    Err(ClaimError::Assigned {
                        key: key("ARC-7"),
                        assignee: String::from("claude"),
                    })
                );
            }
        }
    }

    #[test]
    fn a_claim_checks_not_needed_dependencies_and_questions_in_order() {
        let ticket = ticket(TicketStatus::Planned);
        let facts = StatusFacts {
            acceptance_blank: true,
            open_dependencies: vec![key("ARC-1")],
            open_questions: vec![String::from("Q-1")],
        };
        let claim = |status, force, facts: &StatusFacts| {
            ticket.claimed(String::from("claude"), status, force, facts)
        };
        assert_eq!(
            claim(TicketStatus::NotNeeded, false, &facts),
            Err(StatusLock::NotNeededByPeople.into())
        );
        assert_eq!(
            claim(TicketStatus::InProgress, false, &facts),
            Err(StatusLock::DependencyOpen(vec![key("ARC-1")]).into())
        );
        let questions = StatusFacts {
            open_dependencies: Vec::new(),
            ..facts.clone()
        };
        assert_eq!(
            claim(TicketStatus::Planned, false, &questions),
            Err(StatusLock::QuestionOpen(vec![String::from("Q-1")]).into())
        );
        // a forced claim skips every lock, and the acceptance never counts
        assert!(claim(TicketStatus::NotNeeded, true, &facts).is_ok());
        assert!(claim(TicketStatus::Done, false, &StatusFacts::default()).is_ok());
    }

    #[test]
    fn a_claim_keeps_the_block_reason() {
        let after = blocked("Merge: conflict")
            .claimed(
                String::from("claude"),
                TicketStatus::InProgress,
                false,
                &StatusFacts::default(),
            )
            .unwrap();
        assert_eq!(after.blocked_reason.as_deref(), Some("Merge: conflict"));
    }

    #[test]
    fn a_release_clears_the_assignee_and_keeps_the_status_unless_given() {
        let mut record = ticket(TicketStatus::InProgress).record().clone();
        record.assignee = Some(String::from("claude"));
        let ticket = Ticket::restore(record);
        let kept = ticket.released(None);
        assert_eq!(
            (kept.assignee, kept.status),
            (None, TicketStatus::InProgress)
        );
        let moved = ticket.released(Some(TicketStatus::Planned));
        assert_eq!(
            (moved.assignee, moved.status),
            (None, TicketStatus::Planned)
        );
    }

    #[test]
    fn accepting_needs_a_block_that_waits_for_a_person() {
        let after = blocked("Human Interaction: Review before merge")
            .accepted()
            .unwrap();
        assert_eq!(after.status, TicketStatus::Done);
        assert_eq!(after.blocked_reason, None);
        for ticket in [blocked("Merge: conflict"), ticket(TicketStatus::Planned), {
            let mut record = ticket(TicketStatus::Planned).record().clone();
            record.blocked_reason = Some(String::from(HUMAN_INTERACTION));
            Ticket::restore(record)
        }] {
            assert_eq!(ticket.accepted(), Err(AcceptError { key: key("ARC-7") }));
        }
        assert!(
            AcceptError { key: key("ARC-7") }
                .to_string()
                .contains(HUMAN_INTERACTION)
        );
    }

    #[test]
    fn a_plan_writes_the_given_texts_and_ends_the_planning() {
        let mut record = ticket(TicketStatus::InPlanning).record().clone();
        record.plan = String::from("old plan");
        record.handbook = String::from("old handbook");
        record.acceptance = String::from("old acceptance");
        record.planning_claim = Some((String::from("planner"), Timestamp::from_millis(1)));
        let ticket = Ticket::restore(record);
        let at = Timestamp::from_millis(50);
        let change = PlanChange {
            plan: Some("new plan"),
            handbook: None,
            diagram: Some("```mermaid\ngraph TD\n```"),
            acceptance: Some("  "),
            status: TicketStatus::Planned,
            by: "tristan",
            at,
        };
        let after = ticket.planned(&change);
        assert_eq!(after.plan, "new plan");
        assert_eq!(after.handbook, "old handbook");
        assert_eq!(after.diagram, "graph TD");
        assert_eq!(after.acceptance, "old acceptance");
        assert_eq!(after.status, TicketStatus::Planned);
        assert!(!after.needs_planning);
        assert_eq!(after.planning_claim, None);
        assert_eq!(after.planned, Some((String::from("tristan"), at)));
        let with_acceptance = ticket.planned(&PlanChange {
            acceptance: Some("done when green"),
            ..change
        });
        assert_eq!(with_acceptance.acceptance, "done when green");
    }

    #[test]
    fn another_agents_live_planning_claim_refuses_a_claim() {
        let start = Timestamp::from_millis(1_000);
        let mut record = ticket(TicketStatus::Backlog).record().clone();
        record.planning_claim = Some((String::from("planner"), start));
        let ticket = Ticket::restore(record);
        let later = |minutes: u64| start.checked_add(Duration::from_mins(minutes)).unwrap();
        assert_eq!(
            ticket.planning_claimed(String::from("other"), later(14)),
            Err(PlanningClaimed {
                owner: String::from("planner"),
                key: key("ARC-7"),
            })
        );
        // the owner renews, and an expired claim goes to anyone
        let renewed = ticket
            .planning_claimed(String::from("planner"), later(14))
            .unwrap();
        assert_eq!(
            renewed.planning_claim,
            Some((String::from("planner"), later(14)))
        );
        let taken = ticket
            .planning_claimed(String::from("other"), later(15))
            .unwrap();
        assert!(planning_claim_alive(&taken, later(15)));
        assert_eq!(
            taken.planning_claim,
            Some((String::from("other"), later(15)))
        );
        assert_eq!(taken.status, TicketStatus::Backlog);
    }

    #[test]
    fn only_the_owner_releases_a_planning_claim() {
        let mut record = ticket(TicketStatus::Backlog).record().clone();
        assert_eq!(
            Ticket::restore(record.clone()).planning_released("planner"),
            None
        );
        record.planning_claim = Some((String::from("planner"), Timestamp::from_millis(1)));
        let ticket = Ticket::restore(record);
        assert_eq!(ticket.planning_released("other"), None);
        let released = ticket.planning_released("planner").unwrap();
        assert_eq!(released.planning_claim, None);
        assert_eq!(released.version, Version::INITIAL);
    }
}
