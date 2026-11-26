//! Merge sessions: the Claude run that resolves a merge conflict, its profile and its comments.

use crate::{DispatcherConfig, ProfileChoice, hold_files};

/// Most merge sessions autopilot starts for one conflict, whatever the setting says.
pub const MERGE_SESSIONS_CAP: i64 = 5;
/// Longest agent report the prompt of the next merge session quotes.
pub const PREVIOUS_REPORT_MAX: usize = 2000;
/// How the comments about a merge conflict begin; the next prompt reads them again.
pub const MERGE_CONFLICT_PREFIX: &str = "[dispatch] merge-conflict";
/// End of the comment that announces the next session after a failed one.
pub const NEXT_SESSION: &str = ", next attempt.";
/// Words that mark the comment of a session that started.
const STARTED: &str = " started (profile ";

/// Most merge sessions one conflict gets under autopilot (`autopilot.maxMergeSessions`, 1 to 5).
#[must_use]
pub fn merge_session_limit(config: &DispatcherConfig) -> u32 {
    u32::try_from(
        config
            .autopilot
            .max_merge_sessions
            .clamp(1, MERGE_SESSIONS_CAP),
    )
    .unwrap_or(1)
}

fn ladder_index(config: &DispatcherConfig, name: &str) -> Option<usize> {
    config.ladder.iter().position(|entry| entry == name)
}

/// A merge session runs at least with the default profile.
fn at_least_default(config: &DispatcherConfig, mut choice: ProfileChoice) -> ProfileChoice {
    if ladder_index(config, &choice.profile) >= ladder_index(config, &config.default_profile) {
        return choice;
    }
    if config.profile(&config.default_profile).is_none() {
        return choice;
    }
    choice.why = format!(
        "{}, merge session at least {}",
        choice.why, config.default_profile
    );
    choice.profile.clone_from(&config.default_profile);
    choice.escalated_from = None;
    choice
}

/// Session `session` (from 1) goes one ladder step above the first; the top step repeats.
fn step_up(config: &DispatcherConfig, mut choice: ProfileChoice, session: u32) -> ProfileChoice {
    if session <= 1 {
        return choice;
    }
    let Some(at) = ladder_index(config, &choice.profile) else {
        return choice;
    };
    let steps = usize::try_from(session - 1).unwrap_or(usize::MAX);
    let index = at.saturating_add(steps).min(config.ladder.len() - 1);
    let name = &config.ladder[index];
    if *name == choice.profile || config.profile(name).is_none() {
        return choice;
    }
    choice.why = format!("{}, escalated to {name}", choice.why);
    let before = std::mem::replace(&mut choice.profile, name.clone());
    choice.escalated_from = choice.escalated_from.or(Some(before));
    choice
}

/// The profile of merge session `session` from the profile the ticket would work with.
#[must_use]
pub fn merge_session_choice(
    config: &DispatcherConfig,
    base: ProfileChoice,
    session: u32,
    autopilot: bool,
) -> ProfileChoice {
    let mut choice = step_up(config, at_least_default(config, base), session);
    if autopilot {
        choice.why = format!(
            "{}, merge session {session}/{}",
            choice.why,
            merge_session_limit(config)
        );
    }
    choice
}

/// The comment that says a merge session started for a conflict.
#[must_use]
pub fn session_started_note(
    files: &[String],
    into: &str,
    profile: &str,
    numbered: Option<(u32, u32)>,
) -> String {
    let started = match numbered {
        Some((session, max)) => format!("merge session {session}/{max} started"),
        None => String::from("merge session started"),
    };
    format!(
        "{MERGE_CONFLICT_PREFIX} in {} on merge into {into} - {started} (profile {profile}).",
        hold_files(files)
    )
}

/// The comment that says a session failed and the next one follows.
#[must_use]
pub fn session_failed_note(used: u32, max: u32, detail: &str) -> String {
    format!(
        "{MERGE_CONFLICT_PREFIX} - merge session {used}/{max} without solution ({detail}){NEXT_SESSION}"
    )
}

/// What the prompt of a later merge session needs to know of a comment.
#[derive(Debug, Clone, Copy)]
pub struct ReportComment<'a> {
    pub by_dispatcher: bool,
    pub author: &'a str,
    pub body: &'a str,
}

fn is_conflict(comment: &ReportComment<'_>) -> bool {
    comment.by_dispatcher && comment.body.starts_with(MERGE_CONFLICT_PREFIX)
}

fn clipped(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(PREVIOUS_REPORT_MAX) {
        Some((end, _)) => format!("{}...", &trimmed[..end]),
        None => trimmed.to_owned(),
    }
}

/// The report of the failed earlier sessions: the last outcome and the last word of the agent.
#[must_use]
pub fn previous_attempts(comments: &[ReportComment<'_>], agent: &str) -> String {
    let outcome = comments
        .iter()
        .rev()
        .filter(|comment| is_conflict(comment))
        .find_map(|comment| {
            comment
                .body
                .strip_prefix(MERGE_CONFLICT_PREFIX)?
                .strip_prefix(" - ")?
                .strip_suffix(NEXT_SESSION)
        });
    let started = comments
        .iter()
        .rposition(|comment| is_conflict(comment) && comment.body.contains(STARTED))
        .map_or(0, |index| index + 1);
    let report = comments[started.min(comments.len())..]
        .iter()
        .rev()
        .find(|comment| comment.author == agent && !comment.by_dispatcher)
        .map(|comment| clipped(comment.body));
    let mut text = String::from("Earlier merge session(s) failed");
    if let Some(outcome) = outcome {
        text = format!("{text} - {outcome}");
    }
    match report {
        Some(report) => format!("{text}. Last report of the merge agent: \"{report}\""),
        None => format!("{text}. The merge agent left no report."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProfileReason;

    fn base(profile: &str) -> ProfileChoice {
        ProfileChoice {
            profile: profile.to_owned(),
            reason: ProfileReason::Rule,
            rule: Some(1),
            why: String::from("rule 1"),
            escalated_from: None,
        }
    }

    fn config() -> DispatcherConfig {
        DispatcherConfig::default()
    }

    #[test]
    fn the_limit_stays_between_one_and_five() {
        let mut config = config();
        config.autopilot.max_merge_sessions = 0;
        assert_eq!(merge_session_limit(&config), 1);
        config.autopilot.max_merge_sessions = 9;
        assert_eq!(merge_session_limit(&config), 5);
        config.autopilot.max_merge_sessions = 3;
        assert_eq!(merge_session_limit(&config), 3);
    }

    #[test]
    fn a_session_runs_at_least_with_the_default_profile() {
        let mut config = config();
        config.default_profile = String::from("medium");
        let small = merge_session_choice(&config, base("small"), 1, false);
        assert_eq!(small.profile, "medium");
        assert_eq!(small.why, "rule 1, merge session at least medium");
        assert_eq!(small.escalated_from, None);
        let large = merge_session_choice(&config, base("large"), 1, false);
        assert_eq!(
            (large.profile.as_str(), large.why.as_str()),
            ("large", "rule 1")
        );
    }

    #[test]
    fn every_session_climbs_one_ladder_step_and_the_top_repeats() {
        let mut config = config();
        config.default_profile = String::from("small");
        let first = merge_session_choice(&config, base("small"), 1, true);
        assert_eq!(first.profile, "small");
        assert_eq!(first.why, "rule 1, merge session 1/3");
        let second = merge_session_choice(&config, base("small"), 2, true);
        assert_eq!(second.profile, "medium");
        assert_eq!(second.escalated_from.as_deref(), Some("small"));
        assert_eq!(second.why, "rule 1, escalated to medium, merge session 2/3");
        let third = merge_session_choice(&config, base("small"), 3, true);
        assert_eq!(third.profile, "large");
        let top = merge_session_choice(&config, base("large"), 3, false);
        assert_eq!((top.profile.as_str(), top.escalated_from), ("large", None));
    }

    #[test]
    fn a_profile_off_the_ladder_stays_for_every_session() {
        let mut config = config();
        config.ladder = vec![String::from("medium"), String::from("large")];
        config.default_profile = String::from("small");
        let kept = merge_session_choice(&config, base("small"), 3, false);
        assert_eq!(kept.profile, "small");
        config.default_profile = String::from("medium");
        let lifted = merge_session_choice(&config, base("small"), 1, false);
        assert_eq!(lifted.profile, "medium");
    }

    #[test]
    fn the_comments_name_files_session_and_profile() {
        let files = vec![String::from("a.rs"), String::from("b.rs")];
        assert_eq!(
            session_started_note(&files, "main", "large", Some((2, 3))),
            "[dispatch] merge-conflict in a.rs, b.rs on merge into main - merge session 2/3 started (profile large)."
        );
        assert_eq!(
            session_started_note(&[], "main", "large", None),
            "[dispatch] merge-conflict in unknown files on merge into main - merge session started (profile large)."
        );
        assert_eq!(
            session_failed_note(1, 3, "exit code 1, run abc"),
            "[dispatch] merge-conflict - merge session 1/3 without solution (exit code 1, run abc), next attempt."
        );
    }

    fn comment<'a>(by_dispatcher: bool, author: &'a str, body: &'a str) -> ReportComment<'a> {
        ReportComment {
            by_dispatcher,
            author,
            body,
        }
    }

    #[test]
    fn the_report_quotes_the_last_outcome_and_the_last_word_of_the_agent_since_the_start() {
        let started = session_started_note(&[String::from("a.rs")], "main", "small", Some((2, 3)));
        let failed = session_failed_note(1, 3, "exit code 1, run abc");
        let comments = [
            comment(
                true,
                "dispatcher",
                "[dispatch] merge-conflict in a.rs on merge into main - merge session 1/3 started (profile small).",
            ),
            comment(false, "worker-ARC-1", "too hard"),
            comment(true, "dispatcher", &failed),
            comment(true, "dispatcher", &started),
            comment(false, "worker-ARC-1", "  tests fail  "),
            comment(false, "someone", "other"),
        ];
        assert_eq!(
            previous_attempts(&comments, "worker-ARC-1"),
            "Earlier merge session(s) failed - merge session 1/3 without solution (exit code 1, run abc). Last report of the merge agent: \"tests fail\""
        );
        let silent = [
            comment(true, "dispatcher", &failed),
            comment(true, "dispatcher", &started),
        ];
        assert_eq!(
            previous_attempts(&silent, "worker-ARC-1"),
            "Earlier merge session(s) failed - merge session 1/3 without solution (exit code 1, run abc). The merge agent left no report."
        );
        assert_eq!(
            previous_attempts(&[], "worker-ARC-1"),
            "Earlier merge session(s) failed. The merge agent left no report."
        );
    }

    #[test]
    fn a_long_report_is_cut() {
        let long = "x".repeat(PREVIOUS_REPORT_MAX + 10);
        let comments = [comment(false, "worker", &long)];
        let text = previous_attempts(&comments, "worker");
        assert!(text.ends_with("...\""));
        assert!(text.chars().count() < PREVIOUS_REPORT_MAX + 100);
    }

    #[test]
    fn the_files_of_a_conflict_are_listed_like_a_hold() {
        let many: Vec<String> = (1..=12).map(|n| format!("f{n}")).collect();
        let note = session_started_note(&many, "main", "small", None);
        assert!(note.contains("f10 and 2 more"));
    }
}
