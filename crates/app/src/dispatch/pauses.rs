//! Pauses on a usage limit and on missing Claude: no attempt is counted, the ticket goes back.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{
    Account, DispatchOutcome, DispatcherConfig, Id, Run, RunKind, SystemMarker, TicketRecord,
    TicketStatus, Timestamp, local_day_of,
};

use super::accounts::{AccountLimits, AccountPauses, MAX_PAUSE_DAYS, PauseReason};
use super::evaluate::agent_of;
use super::local_time::{LocalTime, day_text, minute_with_offset};
use super::log::DispatchLog;
use super::pause::{PauseHook, PauseKind};
use super::start::TicketStarter;
use super::state::DispatcherStateService;
use crate::accounts::AccountService;
use crate::comments::CommentService;
use crate::notifier::{Notice, NoticeKind, Notifier};
use crate::runs::{RunFilter, RunService};
use crate::tickets::TicketService;
use crate::{AppError, Clock, ErrorKind};

/// Limit pauses in a row that count no attempt; the next one counts as one.
pub const LIMIT_PAUSES_FREE: usize = 2;

/// How many earlier runs of a ticket the streak reads at most.
const STREAK_RUNS: u32 = 50;

/// The comment of a pause and the text of its notice.
struct Posted {
    body: String,
    toast: String,
}

/// What a pause ends: a ticket that goes back, a research or a plan that waits for a new start.
#[derive(Clone, Copy)]
enum Paused {
    Ticket,
    Research,
    Plan,
}

impl Paused {
    fn back(self) -> &'static str {
        match self {
            Self::Ticket => "ticket back to planned",
            Self::Research => "research not stored",
            Self::Plan => "plan not saved",
        }
    }

    fn next(self) -> &'static str {
        match self {
            Self::Ticket => "The ticket starts on the next pass with another free account.",
            Self::Research => "Start the research again to use another free account.",
            Self::Plan => "Start the planning again to use another free account.",
        }
    }

    fn counting(self) -> &'static str {
        match self {
            Self::Ticket => "Does not count as an attempt.",
            Self::Research => "Start the research again afterwards.",
            Self::Plan => "Start the planning again afterwards.",
        }
    }
}

pub struct DispatchPauses {
    tickets: Arc<TicketService>,
    comments: Arc<CommentService>,
    runs: Arc<RunService>,
    accounts: Arc<AccountService>,
    pauses: Arc<AccountPauses>,
    limits: Arc<dyn AccountLimits>,
    state: Arc<DispatcherStateService>,
    starter: Arc<TicketStarter>,
    local: Arc<dyn LocalTime>,
    clock: Arc<dyn Clock>,
    log: Arc<DispatchLog>,
    notifier: Arc<dyn Notifier>,
}

impl std::fmt::Debug for DispatchPauses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchPauses").finish_non_exhaustive()
    }
}

/// Everything the pauses work with.
pub struct PauseParts {
    pub tickets: Arc<TicketService>,
    pub comments: Arc<CommentService>,
    pub runs: Arc<RunService>,
    pub accounts: Arc<AccountService>,
    pub pauses: Arc<AccountPauses>,
    pub limits: Arc<dyn AccountLimits>,
    pub state: Arc<DispatcherStateService>,
    pub starter: Arc<TicketStarter>,
    pub local: Arc<dyn LocalTime>,
    pub clock: Arc<dyn Clock>,
    pub log: Arc<DispatchLog>,
    pub notifier: Arc<dyn Notifier>,
}

/// What became of the pause of the whole dispatcher.
#[derive(Clone, Copy)]
enum Held {
    /// Set until this moment.
    Until(Timestamp),
    /// A person changed it meanwhile.
    Changed,
    /// Not touched: other accounts can still start work.
    Untouched,
}

/// What the toast of a limit pause is worked out from.
struct ToastParts<'a> {
    now: Timestamp,
    held: Held,
    until: Timestamp,
    own_pause: Option<&'a (String, Timestamp)>,
    own_text: Option<&'a str>,
    multi: bool,
}

/// One full window of an account: its use, its ceiling and when it resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullWindow {
    pub percent: i64,
    pub limit: i64,
    pub resets_at: Option<Timestamp>,
}

/// The last reset of the full windows; unknown when none is full or a reset is missing.
#[must_use]
pub fn reset_after_limit(windows: &[Option<FullWindow>]) -> Option<Timestamp> {
    let full: Vec<Option<Timestamp>> = windows
        .iter()
        .flatten()
        .filter(|window| window.percent >= window.limit)
        .map(|window| window.resets_at)
        .collect();
    if full.is_empty() || full.iter().any(Option::is_none) {
        return None;
    }
    full.into_iter().flatten().max()
}

fn clock_text(minute_of_day: u32) -> String {
    format!("{:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

fn pause_posted(detail: &crate::tickets::TicketDetail, run: &Run) -> bool {
    detail.comments.iter().any(|line| {
        let record = line.comment.record();
        record.system_marker == Some(SystemMarker::Pause) && record.run == Some(run.id())
    })
}

impl DispatchPauses {
    #[must_use]
    pub fn new(parts: PauseParts) -> Self {
        Self {
            tickets: parts.tickets,
            comments: parts.comments,
            runs: parts.runs,
            accounts: parts.accounts,
            pauses: parts.pauses,
            limits: parts.limits,
            state: parts.state,
            starter: parts.starter,
            local: parts.local,
            clock: parts.clock,
            log: parts.log,
            notifier: parts.notifier,
        }
    }

    /// The local time of a moment, with the date when it is not today.
    async fn when(&self, at: Timestamp) -> String {
        let now = self.clock.now();
        let offset = self.local.offset_minutes(at).await;
        let day = local_day_of(at, offset);
        let today = local_day_of(now, self.local.offset_minutes(now).await);
        let minute = minute_with_offset(at, offset);
        let clock = clock_text(minute);
        if day == today {
            clock
        } else {
            format!("{} {clock}", day_text(day))
        }
    }

    fn default_until(&self, config: &DispatcherConfig) -> Timestamp {
        let minutes = u64::try_from(config.pause_minutes_on_limit)
            .unwrap_or(1)
            .max(1);
        let now = self.clock.now();
        now.checked_add(Duration::from_secs(minutes * 60))
            .unwrap_or(now)
    }

    /// Limit pauses of the ticket's earlier runs in a row; pauses of other accounts are skipped.
    async fn streak(
        &self,
        run: &Run,
        key: &str,
        account: Option<uuid::Uuid>,
    ) -> Result<usize, AppError> {
        let filter = RunFilter {
            subject_key: Some(key.to_owned()),
            kind: Some(RunKind::Work),
            link: None,
        };
        let mut count = 0;
        for earlier in self.runs.find(&filter, STREAK_RUNS).await? {
            if earlier.id() == run.id() {
                continue;
            }
            // a run nobody settled yet is no break in the row
            let Some(outcome) = earlier.outcome() else {
                continue;
            };
            if outcome != DispatchOutcome::PauseLimit {
                break;
            }
            let of = earlier.dispatch().and_then(|dispatch| dispatch.account);
            if matches!((account, of), (Some(mine), Some(theirs)) if mine != theirs) {
                continue;
            }
            count += 1;
        }
        Ok(count)
    }

    /// The reset of the account's full windows, read fresh.
    async fn reset_of(&self, account: &Account, config: &DispatcherConfig) -> Option<Timestamp> {
        let status = self
            .limits
            .status(std::slice::from_ref(account), true)
            .await
            .pop()?;
        let window = |window: Option<crate::usage::UsageWindow>, own: Option<u8>| {
            window.map(|window| FullWindow {
                percent: i64::from(window.percent),
                limit: own.map_or(config.account_limit_percent, i64::from),
                resets_at: window.resets_at,
            })
        };
        reset_after_limit(&[
            window(
                status.usage.five_hour,
                account
                    .limit_five_hour()
                    .map(nullradix_domain::Limit::percent),
            ),
            window(
                status.usage.seven_day,
                account.limit_week().map(nullradix_domain::Limit::percent),
            ),
        ])
    }

    /// The end of the account's pause: one that runs already, else a new one until `until`.
    async fn account_pause(
        &self,
        run: &Run,
        account: &Account,
        until: Timestamp,
    ) -> Result<Timestamp, AppError> {
        let held = self
            .pauses
            .active()
            .await?
            .into_iter()
            .find(|pause| pause.account == account.id());
        if let Some(held) = held {
            return Ok(held.until);
        }
        // a person may have paused it while the reset was read
        let latest = self.pauses.active().await?;
        if let Some(pause) = latest.iter().find(|pause| pause.account == account.id()) {
            return Ok(pause.until);
        }
        let limit = self
            .clock
            .now()
            .checked_add(Duration::from_hours(MAX_PAUSE_DAYS * 24))
            .unwrap_or(until);
        let pause = self
            .pauses
            .pause_account(
                account.id(),
                until.min(limit),
                PauseReason::UsageLimit,
                Some(run.id()),
            )
            .await?;
        Ok(pause.until)
    }

    /// The toast of a limit pause; it names only a pause that is set and still ahead.
    async fn limit_toast(&self, parts: ToastParts<'_>) -> String {
        let ToastParts {
            now,
            held,
            until,
            own_pause,
            own_text,
            multi,
        } = parts;
        // the account's own end is the one that matters only next to other accounts
        let shown = if let (Some(text), true) = (own_text, multi) {
            Some(text.to_owned())
        } else {
            let holding = match held {
                Held::Until(at) => Some(at),
                // a person changed it meanwhile: theirs holds, if one is still set
                Held::Changed => self
                    .state
                    .get()
                    .await
                    .ok()
                    .and_then(|state| state.record().paused_until)
                    .filter(|at| *at > now),
                Held::Untouched => Some(until),
            };
            match holding {
                Some(at) => Some(self.when(at).await),
                None => None,
            }
        };
        let name = own_pause.map_or_else(String::new, |(slug, _)| format!(" {slug}"));
        match shown {
            Some(text) => format!("usage limit{name} - pause until {text}"),
            None => format!("usage limit{name} - pause stays as set by hand"),
        }
    }

    async fn pause_for_limit(
        &self,
        run: &Run,
        config: &DispatcherConfig,
        what: Paused,
    ) -> Result<Posted, AppError> {
        let now = self.clock.now();
        let all = self.accounts.list().await?;
        let enabled: Vec<&Account> = all.iter().filter(|account| account.enabled()).collect();
        let multi = enabled.len() > 1;
        let chosen = run
            .dispatch()
            .and_then(|dispatch| dispatch.account)
            .map(Id::<Account>::from_uuid)
            .and_then(|id| all.iter().find(|account| account.id() == id));
        let seen = self.state.get().await?.record().paused_until;
        let mut until = self.default_until(config);
        let mut own_pause = None;
        let mut whole = true;
        if let Some(account) = chosen {
            if let Some(reset) = self.reset_of(account, config).await.filter(|at| *at > now) {
                until = reset;
            }
            let own = self.account_pause(run, account, until).await?;
            until = own;
            own_pause = Some((account.slug().as_str().to_owned(), own));
            if multi {
                let active = self.pauses.active().await?;
                let paused: Vec<Timestamp> = enabled
                    .iter()
                    .filter_map(|account| {
                        active
                            .iter()
                            .find(|pause| pause.account == account.id())
                            .map(|pause| pause.until)
                    })
                    .collect();
                whole = paused.len() == enabled.len();
                if whole {
                    until = paused.into_iter().min().unwrap_or(until);
                }
            }
        }
        let held = if whole {
            Some(self.state.pause_unless_changed(until, seen).await?)
        } else {
            None
        };
        let own_text = match &own_pause {
            Some((_, at)) => Some(self.when(*at).await),
            None => None,
        };
        let rest = match held {
            Some(None) => String::from(
                "The pause of the dispatcher was changed by hand meanwhile and stays as set.",
            ),
            Some(Some(at)) => {
                let text = self.when(at).await;
                if own_pause.is_some() && multi {
                    format!(
                        "All accounts are paused - the dispatcher starts nothing new until {text}."
                    )
                } else {
                    format!("The dispatcher starts nothing new until {text}.")
                }
            }
            None => String::from(what.next()),
        };
        let body = match (&own_pause, &own_text, multi) {
            (Some((slug, _)), Some(text), true) => format!(
                "[dispatch] pause: usage limit of account {slug} reached, {}. Account {slug} paused until {text}. {rest} {}",
                what.back(),
                what.counting()
            ),
            _ => format!(
                "[dispatch] pause: usage limit reached, {}. {rest} {}",
                what.back(),
                what.counting()
            ),
        };
        let toast = self
            .limit_toast(ToastParts {
                now,
                held: match held {
                    Some(Some(at)) => Held::Until(at),
                    Some(None) => Held::Changed,
                    None => Held::Untouched,
                },
                until,
                own_pause: own_pause.as_ref(),
                own_text: own_text.as_deref(),
                multi,
            })
            .await;
        Ok(Posted { body, toast })
    }

    async fn pause_for_missing(
        &self,
        config: &DispatcherConfig,
        what: Paused,
    ) -> Result<Posted, AppError> {
        let seen = self.state.get().await?.record().paused_until;
        let until = self.default_until(config);
        let applied = self.state.pause_unless_changed(until, seen).await?;
        let minutes = config.pause_minutes_on_limit;
        let tail = match applied {
            Some(at) if at > until => format!(
                "The dispatcher starts nothing new until {}, as paused by hand.",
                self.when(at).await
            ),
            Some(_) => format!("The dispatcher starts nothing new for {minutes} min."),
            None => String::from(
                "The pause of the dispatcher was changed by hand meanwhile and stays as set.",
            ),
        };
        let body = format!(
            "[dispatch] pause: Claude Code was not found (command \"{}\"), {}. {tail} Please install Claude Code or check the command in the settings. {}",
            config.claude.command,
            what.back(),
            what.counting()
        );
        let toast = match applied {
            Some(at) if at > until => {
                format!(
                    "Claude Code not found - paused until {}",
                    self.when(at).await
                )
            }
            None => String::from("Claude Code not found - pause stays as set by hand"),
            Some(_) => format!("Claude Code not found - pause {minutes} min"),
        };
        Ok(Posted { body, toast })
    }
}

#[async_trait]
impl PauseHook for DispatchPauses {
    async fn limit_exhausted(&self, run: &Run, ticket: &TicketRecord) -> Result<bool, AppError> {
        let account = run.dispatch().and_then(|dispatch| dispatch.account);
        let streak = self.streak(run, &ticket.key.to_string(), account).await?;
        Ok(streak >= LIMIT_PAUSES_FREE)
    }

    async fn paused(
        &self,
        run: &Run,
        ticket: &TicketRecord,
        kind: PauseKind,
        config: &DispatcherConfig,
    ) -> Result<Option<DispatchOutcome>, AppError> {
        let actor = self.starter.actor().await?;
        let text = ticket.key.to_string();
        let detail = self.tickets.get(&text).await?;
        // a comment of this run means an earlier pass paused and stopped before it was done
        if !pause_posted(&detail, run) {
            let posted = match kind {
                PauseKind::UsageLimit => self.pause_for_limit(run, config, Paused::Ticket).await?,
                PauseKind::ClaudeMissing => self.pause_for_missing(config, Paused::Ticket).await?,
            };
            self.comments
                .add_system(
                    &ticket.key,
                    &posted.body,
                    Some(SystemMarker::Pause),
                    Some(run.id()),
                    actor,
                )
                .await?;
            self.log.info(&posted.body, Some(&text)).await;
            self.toast(ticket, posted.toast).await;
        }
        let fresh = self.tickets.get(&text).await?;
        let current = fresh.view.ticket.record();
        let ours = current.status == TicketStatus::InProgress
            && current.assignee.as_deref() == Some(agent_of(run, &text, config).as_str());
        if ours {
            match self
                .tickets
                .release(&text, Some("planned"), actor, Some(current.version), false)
                .await
            {
                Ok(_) => {}
                // a person changed the ticket first; it is theirs now
                Err(error) if error.kind() == ErrorKind::Conflict => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Some(match kind {
            PauseKind::UsageLimit => DispatchOutcome::PauseLimit,
            PauseKind::ClaudeMissing => DispatchOutcome::PauseClaudeMissing,
        }))
    }

    async fn paused_research(
        &self,
        run: &Run,
        title: &str,
        kind: PauseKind,
        config: &DispatcherConfig,
    ) -> Result<Option<String>, AppError> {
        let posted = match kind {
            PauseKind::UsageLimit => self.pause_for_limit(run, config, Paused::Research).await?,
            PauseKind::ClaudeMissing => self.pause_for_missing(config, Paused::Research).await?,
        };
        self.log.info(&posted.body, None).await;
        self.notify(None, title, posted.toast).await;
        // the comment of the question is the marker of an empty research
        Ok(Some(posted.body.replacen(
            "[dispatch] pause:",
            "[dispatch] research-empty:",
            1,
        )))
    }

    async fn paused_plan(
        &self,
        run: &Run,
        ticket: &TicketRecord,
        kind: PauseKind,
        config: &DispatcherConfig,
    ) -> Result<Option<String>, AppError> {
        let posted = match kind {
            PauseKind::UsageLimit => self.pause_for_limit(run, config, Paused::Plan).await?,
            PauseKind::ClaudeMissing => self.pause_for_missing(config, Paused::Plan).await?,
        };
        self.log
            .info(&posted.body, Some(&ticket.key.to_string()))
            .await;
        self.toast(ticket, posted.toast).await;
        // the comment of the ticket is the marker of an empty plan
        Ok(Some(posted.body.replacen(
            "[dispatch] pause:",
            "[dispatch] plan-empty:",
            1,
        )))
    }
}

impl DispatchPauses {
    async fn toast(&self, ticket: &TicketRecord, text: String) {
        self.notify(Some(ticket.key.to_string()), ticket.title.as_str(), text)
            .await;
    }

    async fn notify(&self, ticket: Option<String>, title: &str, text: String) {
        let notice = Notice {
            kind: NoticeKind::Paused,
            ticket,
            title: title.to_owned(),
            text,
        };
        if let Err(error) = self.notifier.notify(&notice).await {
            self.log
                .error(
                    &format!("cannot show a notice: {}", error.message()),
                    notice.ticket.as_deref(),
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> Timestamp {
        Timestamp::from_millis(minutes * 60_000)
    }

    fn window(percent: i64, resets: Option<i64>) -> FullWindow {
        FullWindow {
            percent,
            limit: 90,
            resets_at: resets.map(at),
        }
    }

    #[test]
    fn the_account_is_free_when_its_last_full_window_resets() {
        let reset = reset_after_limit(&[Some(window(95, Some(10))), Some(window(91, Some(40)))]);
        assert_eq!(reset, Some(at(40)));
        let only = reset_after_limit(&[Some(window(95, Some(10))), Some(window(20, Some(99)))]);
        assert_eq!(only, Some(at(10)));
    }

    #[test]
    fn no_full_window_or_a_missing_reset_is_unknown() {
        assert_eq!(reset_after_limit(&[Some(window(10, Some(5))), None]), None);
        assert_eq!(
            reset_after_limit(&[Some(window(95, None)), Some(window(95, Some(5)))]),
            None
        );
    }

    #[test]
    fn the_clock_text_has_two_digits() {
        assert_eq!(clock_text(9 * 60 + 5), "09:05");
    }
}
