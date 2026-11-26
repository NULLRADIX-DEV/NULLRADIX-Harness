//! What a finished work run left on its ticket, decided as legacy `ResultHandler` did.

use nullradix_domain::{
    Abort, AbortCategory, DispatchOutcome, DispatcherConfig, QuestionKey, Run, RunStatus,
    TicketRecord, TicketStatus, waits_for_a_person,
};

/// The decision for one finished work run; the first match in legacy order wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The ticket was finished; a gate decides what follows (review, merge).
    Done,
    /// The ticket waits for a person and autopilot accepts it itself.
    AutoAccept { reason: String },
    /// Blocked with "Human Interaction": a person has to look at it.
    Human(String),
    /// Blocked for any other reason.
    Blocked(String),
    /// The agent asked and the ticket waits for the answers.
    Question(Vec<QuestionKey>),
    /// Someone else took the ticket.
    Taken,
    /// The ticket is neither done, blocked nor in progress.
    Other,
    /// The run ended without achieving anything; the attempt is counted.
    Abort(Ended),
    /// Starting the run failed; the start counted the attempt already.
    StartFailed,
}

/// How a run that achieved nothing ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub category: Option<AbortCategory>,
    pub reason: String,
}

impl Decision {
    /// The outcome to record, when the decision settles the run on its own.
    #[must_use]
    pub const fn outcome(&self) -> Option<DispatchOutcome> {
        match self {
            Self::Done | Self::AutoAccept { .. } => Some(DispatchOutcome::Done),
            Self::Human(_) => Some(DispatchOutcome::Human),
            Self::Blocked(_) => Some(DispatchOutcome::Blocked),
            Self::Question(_) => Some(DispatchOutcome::Question),
            Self::Taken => Some(DispatchOutcome::Taken),
            Self::Other => Some(DispatchOutcome::Other),
            Self::Abort(_) | Self::StartFailed => None,
        }
    }
}

/// Decides from the ticket as it is now; `agent` is the name the run worked under.
#[must_use]
pub fn decide(
    config: &DispatcherConfig,
    agent: &str,
    run: &Run,
    ticket: &TicketRecord,
    open_questions: &[QuestionKey],
) -> Decision {
    let abort = run.result().and_then(|result| result.abort.as_ref());
    // Claude that was not found ran and exited; a run that never spawned only waits
    if abort.is_some_and(|abort| abort.category == AbortCategory::StartFailed)
        && run.exit_code().is_none()
    {
        return Decision::StartFailed;
    }
    let assignee = ticket.assignee.as_deref();
    if ticket.status == TicketStatus::Done {
        // a person who took the ticket and finished it keeps it out of the dispatcher's hands
        return if assignee.is_some_and(|name| name != agent) {
            Decision::Taken
        } else {
            Decision::Done
        };
    }
    if assignee != Some(agent) {
        return Decision::Taken;
    }
    if ticket.status == TicketStatus::Blocked {
        let reason = ticket.blocked_reason.clone().unwrap_or_default();
        let human = waits_for_a_person(Some(&reason));
        return match (human, config.autopilot_applies(&ticket.labels)) {
            (true, true) => Decision::AutoAccept {
                reason: reason.trim().to_owned(),
            },
            (true, false) => Decision::Human(reason),
            (false, _) => Decision::Blocked(reason),
        };
    }
    if ticket.status != TicketStatus::InProgress {
        return Decision::Other;
    }
    if !open_questions.is_empty() {
        return Decision::Question(open_questions.to_vec());
    }
    Decision::Abort(ended(run, abort))
}

const NO_RESULT: &str =
    "run ended without a result - the ticket is neither done nor blocked and has no open question";

fn ended(run: &Run, abort: Option<&Abort>) -> Ended {
    match abort {
        Some(abort) => Ended {
            category: Some(abort.category),
            reason: abort.reason.clone(),
        },
        None => Ended {
            category: None,
            reason: String::from(if run.status() == RunStatus::Cancelled {
                "the run was cancelled"
            } else {
                NO_RESULT
            }),
        },
    }
}

/// The comment of a run that ended with questions open.
#[must_use]
pub fn question_comment(questions: &[QuestionKey], branch: Option<&str>) -> String {
    let keys = questions
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let stays = branch.map_or_else(
        || String::from("the worktree stays"),
        |branch| format!("branch {branch} and the worktree stay"),
    );
    format!(
        "[dispatch] question: waiting on {keys} - ticket back to planned, {stays}. It continues there once decided."
    )
}

#[cfg(test)]
mod tests {
    use nullradix_domain::{
        DispatchRun, Id, Priority, RunEnd, RunKind, RunLabel, RunOutcome, RunProfile, RunResult,
        RunTrigger, TicketKey, TicketTitle, Timestamp, Version,
    };

    use super::*;

    const AGENT: &str = "worker-ARC-0001";

    fn dispatch() -> DispatchRun {
        DispatchRun {
            kind: RunKind::Work,
            trigger: RunTrigger::Dispatcher,
            subject_key: Some(String::from("ARC-0001")),
            profile: RunProfile {
                id: None,
                name: String::from("medium"),
                model: None,
                effort: None,
                max_turns: 10,
                timeout_minutes: 10,
            },
            account: None,
            config_hash: String::new(),
            agent: None,
            cwd: String::new(),
            args: Vec::new(),
            prompt: String::new(),
        }
    }

    fn run() -> Run {
        Run::new(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            RunLabel::parse("work").unwrap(),
            Timestamp::from_millis(1),
        )
        .with_dispatch(dispatch())
    }

    fn ticket(status: TicketStatus, assignee: Option<&str>, reason: Option<&str>) -> TicketRecord {
        let id = |value| uuid::Uuid::from_u128(value);
        TicketRecord {
            id: Id::from_uuid(id(3)),
            key: TicketKey::parse("ARC-1").unwrap(),
            project: Id::from_uuid(id(4)),
            board: Id::from_uuid(id(5)),
            title: TicketTitle::parse("Title").unwrap(),
            description: String::new(),
            acceptance: String::new(),
            plan: String::new(),
            handbook: String::new(),
            diagram: String::new(),
            status,
            priority: Priority::Normal,
            labels: Vec::new(),
            depends_on: Vec::new(),
            sort_order: 0,
            assignee: assignee.map(str::to_owned),
            needs_planning: false,
            blocked_reason: reason.map(str::to_owned),
            target_branch: None,
            branch: None,
            worktree: None,
            planned: None,
            planning_claim: None,
            created_by: Id::from_uuid(id(2)),
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(1),
            version: Version::INITIAL,
        }
    }

    fn decide_for(record: &TicketRecord, questions: &[QuestionKey], autopilot: bool) -> Decision {
        let mut config = DispatcherConfig::default();
        config.autopilot.enabled = autopilot;
        decide(&config, AGENT, &run(), record, questions)
    }

    #[test]
    fn a_finished_ticket_is_done_unless_a_person_took_it() {
        let ours = ticket(TicketStatus::Done, Some(AGENT), None);
        assert_eq!(decide_for(&ours, &[], false), Decision::Done);
        let unassigned = ticket(TicketStatus::Done, None, None);
        assert_eq!(decide_for(&unassigned, &[], false), Decision::Done);
        let taken = ticket(TicketStatus::Done, Some("anna"), None);
        assert_eq!(decide_for(&taken, &[], false), Decision::Taken);
    }

    #[test]
    fn blocked_splits_into_human_blocked_and_autopilot_accept() {
        let human = ticket(
            TicketStatus::Blocked,
            Some(AGENT),
            Some("Human Interaction: look at it"),
        );
        assert_eq!(
            decide_for(&human, &[], false),
            Decision::Human(String::from("Human Interaction: look at it"))
        );
        assert_eq!(
            decide_for(&human, &[], true),
            Decision::AutoAccept {
                reason: String::from("Human Interaction: look at it")
            }
        );
        let bare = ticket(
            TicketStatus::Blocked,
            Some(AGENT),
            Some("Human Interaction"),
        );
        assert!(matches!(decide_for(&bare, &[], false), Decision::Human(_)));
        let missing = ticket(TicketStatus::Blocked, Some(AGENT), Some("no access"));
        assert_eq!(
            decide_for(&missing, &[], true),
            Decision::Blocked(String::from("no access"))
        );
        let strangers = ticket(TicketStatus::Blocked, Some("anna"), Some("no access"));
        assert_eq!(decide_for(&strangers, &[], false), Decision::Taken);
    }

    #[test]
    fn open_questions_win_over_an_open_run_and_other_states_are_other() {
        let working = ticket(TicketStatus::InProgress, Some(AGENT), None);
        let question = QuestionKey::parse("ARC-Q0001").unwrap();
        assert_eq!(
            decide_for(&working, std::slice::from_ref(&question), false),
            Decision::Question(vec![question])
        );
        assert_eq!(
            decide_for(&working, &[], false),
            Decision::Abort(Ended {
                category: None,
                reason: String::from(NO_RESULT)
            })
        );
        let moved = ticket(TicketStatus::Planned, Some(AGENT), None);
        assert_eq!(decide_for(&moved, &[], false), Decision::Other);
        let released = ticket(TicketStatus::Planned, None, None);
        assert_eq!(decide_for(&released, &[], false), Decision::Taken);
    }

    #[test]
    fn a_run_that_never_started_is_left_to_its_start() {
        let record = ticket(TicketStatus::Planned, None, None);
        let failed = run()
            .failed_to_start(String::from("no claude"), Timestamp::from_millis(2))
            .unwrap();
        let decision = decide(&DispatcherConfig::default(), AGENT, &failed, &record, &[]);
        assert_eq!(decision, Decision::StartFailed);
        assert_eq!(decision.outcome(), None);
    }

    fn finished_with(abort: Option<Abort>, status: RunStatus, exit: Option<i32>) -> Run {
        let (outcome, code) = match status {
            RunStatus::Cancelled => (RunOutcome::Cancelled, None),
            RunStatus::TimedOut => (RunOutcome::TimedOut, None),
            RunStatus::Succeeded => (RunOutcome::Succeeded, Some(0)),
            _ => (RunOutcome::Failed, exit),
        };
        let started = run().started(1, Timestamp::from_millis(2)).unwrap();
        let end = RunEnd {
            exit_code: code,
            error: None,
        };
        let result = RunResult {
            abort,
            ..RunResult::default()
        };
        started
            .finished_with(outcome, end, Some(result), Timestamp::from_millis(3))
            .unwrap()
    }

    #[test]
    fn a_run_that_achieved_nothing_is_an_abort_with_the_reason_of_its_end() {
        let working = ticket(TicketStatus::InProgress, Some(AGENT), None);
        let config = DispatcherConfig::default();
        let timeout = Abort {
            category: AbortCategory::Timeout,
            reason: String::from("time limit of 90 min reached, run stopped"),
        };
        let run = finished_with(Some(timeout), RunStatus::TimedOut, None);
        assert_eq!(
            decide(&config, AGENT, &run, &working, &[]),
            Decision::Abort(Ended {
                category: Some(AbortCategory::Timeout),
                reason: String::from("time limit of 90 min reached, run stopped")
            })
        );
        let clean = finished_with(None, RunStatus::Succeeded, Some(0));
        assert_eq!(
            decide(&config, AGENT, &clean, &working, &[]),
            Decision::Abort(Ended {
                category: None,
                reason: String::from(NO_RESULT)
            })
        );
        let cancelled = finished_with(None, RunStatus::Cancelled, None);
        let Decision::Abort(ended) = decide(&config, AGENT, &cancelled, &working, &[]) else {
            panic!("not an abort");
        };
        assert_eq!(ended.reason, "the run was cancelled");
    }

    #[test]
    fn claude_that_was_not_found_is_an_abort_of_its_own_category() {
        let working = ticket(TicketStatus::InProgress, Some(AGENT), None);
        let missing = Abort {
            category: AbortCategory::StartFailed,
            reason: String::from("claude could not be started: not found"),
        };
        let run = finished_with(Some(missing), RunStatus::Failed, Some(127));
        let decision = decide(&DispatcherConfig::default(), AGENT, &run, &working, &[]);
        assert!(matches!(
            decision,
            Decision::Abort(Ended {
                category: Some(AbortCategory::StartFailed),
                ..
            })
        ));
    }

    #[test]
    fn decisions_map_to_the_recorded_outcomes() {
        assert_eq!(Decision::Done.outcome(), Some(DispatchOutcome::Done));
        assert_eq!(
            Decision::AutoAccept {
                reason: String::new()
            }
            .outcome(),
            Some(DispatchOutcome::Done)
        );
        assert_eq!(
            Decision::Human(String::new()).outcome(),
            Some(DispatchOutcome::Human)
        );
        assert_eq!(
            Decision::Blocked(String::new()).outcome(),
            Some(DispatchOutcome::Blocked)
        );
        assert_eq!(
            Decision::Question(Vec::new()).outcome(),
            Some(DispatchOutcome::Question)
        );
        assert_eq!(Decision::Taken.outcome(), Some(DispatchOutcome::Taken));
        assert_eq!(Decision::Other.outcome(), Some(DispatchOutcome::Other));
    }

    #[test]
    fn the_question_comment_names_questions_and_branch() {
        let keys = [
            QuestionKey::parse("ARC-Q0001").unwrap(),
            QuestionKey::parse("ARC-Q0002").unwrap(),
        ];
        assert_eq!(
            question_comment(&keys, Some("ticket/ARC-0001")),
            "[dispatch] question: waiting on ARC-Q0001, ARC-Q0002 - ticket back to planned, branch ticket/ARC-0001 and the worktree stay. It continues there once decided."
        );
        assert!(question_comment(&keys[..1], None).contains("the worktree stays"));
    }
}
