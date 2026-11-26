//! Accounts of dispatcher runs: choice, pauses until a reset, usage samples.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{
    Account, AccountChoice, AccountChoiceInput, AccountReserve, AccountState, AllowedAccounts,
    DispatchRun, DispatcherConfig, EstimatedReserve, Id, ProfileChoice, ReserveFor, Run, RunKind,
    RunningWork, Timestamp, UsageSample, UsageSnapshot, allowed_accounts, choose_account,
    estimate_reserve, pending_reserve,
};
use strum::{EnumString, IntoStaticStr};
use uuid::Uuid;

use super::state::DispatcherStateService;
use crate::accounts::AccountService;
use crate::runs::RunService;
use crate::usage::{AccountStatus, AccountUsage, UsageService};
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// Longest a pause may run ahead; a week's limit resets within a week.
pub const MAX_PAUSE_DAYS: u64 = 14;

/// How many past samples the reserve estimate reads at most.
pub const SAMPLE_LIMIT: u32 = 500;

/// How often a choice is made again after a pause turned up while it was made.
const CHOICE_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum PauseReason {
    UsageLimit,
    AuthMissing,
    Manual,
}

impl PauseReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// An account that the choice skips until `until`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPause {
    pub account: Id<Account>,
    pub until: Timestamp,
    pub reason: PauseReason,
    /// The run whose outcome caused the pause.
    pub run: Option<Id<Run>>,
    pub created_at: Timestamp,
}

#[async_trait]
pub trait AccountPauseRepository: Send + Sync {
    /// Every stored pause, also the ended ones.
    async fn all(&self) -> Result<Vec<AccountPause>, AppError>;
    /// Stores the pause of its account, replacing an earlier one.
    async fn put(&self, pause: &AccountPause) -> Result<Change, AppError>;
    /// Removes the pause of an account; none when it had none.
    async fn remove(&self, account: Id<Account>, at: Timestamp)
    -> Result<Option<Change>, AppError>;
    /// Removes every pause and returns the accounts that had one.
    async fn clear(&self, at: Timestamp) -> Result<Vec<Change>, AppError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum SamplePhase {
    Start,
    End,
    Poll,
}

impl SamplePhase {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One reading of the limits of an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSample {
    pub id: Uuid,
    pub account: Id<Account>,
    pub run: Option<Id<Run>>,
    pub phase: SamplePhase,
    pub usage: UsageSnapshot,
    pub at: Timestamp,
}

#[async_trait]
pub trait UsageSampleRepository: Send + Sync {
    /// Stores a start or end row; a second one of the same run and phase is dropped.
    async fn record(&self, sample: &NewSample) -> Result<(), AppError>;
    /// Stores a reading only when it differs from the latest one of the account.
    async fn record_poll(&self, sample: &NewSample) -> Result<bool, AppError>;
    /// Runs with a start and an end row, oldest end first, the latest `limit` of them.
    async fn history(&self, limit: u32) -> Result<Vec<UsageSample>, AppError>;
    /// The start rows of these runs.
    async fn starts(&self, runs: &[Id<Run>]) -> Result<HashMap<Id<Run>, UsageSnapshot>, AppError>;
}

/// The limits and login of accounts as Anthropic and Claude's files say.
#[async_trait]
pub trait AccountLimits: Send + Sync {
    async fn status(&self, accounts: &[Account], fresh: bool) -> Vec<AccountStatus>;
}

#[async_trait]
impl AccountLimits for UsageService {
    async fn status(&self, accounts: &[Account], fresh: bool) -> Vec<AccountStatus> {
        UsageService::status(self, accounts, fresh).await
    }
}

#[must_use]
pub fn pause_invalid(message: &str) -> AppError {
    AppError::invalid("account-pause-invalid", message.to_owned())
}

/// Pauses and resumes accounts for the choice.
pub struct AccountPauses {
    repository: Arc<dyn AccountPauseRepository>,
    accounts: Arc<AccountService>,
    state: Arc<DispatcherStateService>,
    clock: Arc<dyn Clock>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for AccountPauses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountPauses").finish_non_exhaustive()
    }
}

impl AccountPauses {
    #[must_use]
    pub fn new(
        repository: Arc<dyn AccountPauseRepository>,
        accounts: Arc<AccountService>,
        state: Arc<DispatcherStateService>,
        clock: Arc<dyn Clock>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            accounts,
            state,
            clock,
            signal,
        }
    }

    /// The pauses that have not ended yet.
    pub async fn active(&self) -> Result<Vec<AccountPause>, AppError> {
        let now = self.clock.now();
        let mut pauses = self.repository.all().await?;
        pauses.retain(|pause| pause.until > now);
        Ok(pauses)
    }

    /// Keeps runs off the account until `until`; the pause of the account is replaced.
    pub async fn pause_account(
        &self,
        account: Id<Account>,
        until: Timestamp,
        reason: PauseReason,
        run: Option<Id<Run>>,
    ) -> Result<AccountPause, AppError> {
        let now = self.clock.now();
        if until <= now {
            return Err(pause_invalid("the pause must end in the future"));
        }
        let latest = now
            .checked_add(Duration::from_hours(MAX_PAUSE_DAYS * 24))
            .ok_or_else(|| pause_invalid("the pause ends too far ahead"))?;
        if until > latest {
            return Err(pause_invalid(&format!(
                "the pause must end within {MAX_PAUSE_DAYS} days"
            )));
        }
        self.accounts.get(account).await?;
        let pause = AccountPause {
            account,
            until,
            reason,
            run,
            created_at: now,
        };
        let change = self.repository.put(&pause).await?;
        self.signal.notify(&change);
        Ok(pause)
    }

    /// Lets runs use the account again; a dispatcher paused as a whole only for it resumes too.
    pub async fn resume_account(&self, account: Id<Account>) -> Result<bool, AppError> {
        self.accounts.get(account).await?;
        let Some(change) = self.repository.remove(account, self.clock.now()).await? else {
            return Ok(false);
        };
        self.signal.notify(&change);
        self.state.resume().await?;
        Ok(true)
    }

    /// Ends every account pause, as resuming the dispatcher does.
    pub async fn resume_all(&self) -> Result<(), AppError> {
        for change in self.repository.clear(self.clock.now()).await? {
            self.signal.notify(&change);
        }
        Ok(())
    }
}

/// What a run asks of the account choice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountRequest {
    pub allowed: AllowedAccounts,
    /// The profile the run starts with; side runs have none and reserve nothing.
    pub profile: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl AccountRequest {
    /// The request of a ticket run with its profile choice and labels.
    #[must_use]
    pub fn for_ticket(
        config: &DispatcherConfig,
        choice: &ProfileChoice,
        labels: &[String],
    ) -> Self {
        let profile = config.profile(&choice.profile);
        Self {
            allowed: allowed_accounts(config, choice, labels),
            profile: Some(choice.profile.clone()),
            model: profile
                .and_then(|profile| super::invocation::effective_model(&config.claude, profile)),
            effort: profile
                .and_then(|profile| super::invocation::effective_effort(&config.claude, profile)),
        }
    }
}

/// The account a run starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub account: Id<Account>,
    pub slug: String,
    /// Why this account, for the start comment.
    pub why: String,
    pub snapshot: UsageSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountPick {
    Picked(Chosen),
    Waiting {
        reason: String,
        resume_at: Option<Timestamp>,
    },
}

/// A start asks Anthropic afresh and keeps the readings; a preview reads without writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickMode {
    Start,
    Preview,
}

pub struct AccountPicker {
    accounts: Arc<AccountService>,
    limits: Arc<dyn AccountLimits>,
    pauses: Arc<AccountPauses>,
    samples: Arc<dyn UsageSampleRepository>,
    runs: Arc<RunService>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl std::fmt::Debug for AccountPicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountPicker").finish_non_exhaustive()
    }
}

/// Everything a picker works with.
pub struct PickerParts {
    pub accounts: Arc<AccountService>,
    pub limits: Arc<dyn AccountLimits>,
    pub pauses: Arc<AccountPauses>,
    pub samples: Arc<dyn UsageSampleRepository>,
    pub runs: Arc<RunService>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
}

/// The numbers of an available reading, as the choice sees them.
fn snapshot_of(usage: &AccountUsage) -> UsageSnapshot {
    if !usage.available() {
        return UsageSnapshot::default();
    }
    UsageSnapshot {
        five_hour: usage.five_hour.map(|window| i64::from(window.percent)),
        seven_day: usage.seven_day.map(|window| i64::from(window.percent)),
        five_hour_resets_at: usage.five_hour.and_then(|window| window.resets_at),
        seven_day_resets_at: usage.seven_day.and_then(|window| window.resets_at),
    }
}

impl AccountPicker {
    #[must_use]
    pub fn new(parts: PickerParts) -> Self {
        Self {
            accounts: parts.accounts,
            limits: parts.limits,
            pauses: parts.pauses,
            samples: parts.samples,
            runs: parts.runs,
            clock: parts.clock,
            ids: parts.ids,
        }
    }

    /// The pauses, for callers that show or set them.
    #[must_use]
    pub const fn pauses(&self) -> &Arc<AccountPauses> {
        &self.pauses
    }

    /// Picks the account of a run, or says why none is free.
    pub async fn pick(
        &self,
        request: &AccountRequest,
        config: &DispatcherConfig,
        mode: PickMode,
    ) -> Result<AccountPick, AppError> {
        let accounts = self.accounts.list().await?;
        let asked: Vec<Account> = accounts
            .iter()
            .filter(|account| {
                request.allowed.slugs.is_empty()
                    || request
                        .allowed
                        .slugs
                        .iter()
                        .any(|slug| slug == account.slug().as_str())
            })
            .cloned()
            .collect();
        let mut states = self.read_states(&accounts, &asked, mode).await;
        let (reserve, estimate, pending) = self.reserves(request, config, &states).await?;
        for _ in 0..CHOICE_ATTEMPTS {
            let pauses = self.pauses.active().await?;
            for pause in &pauses {
                states.entry(pause.account).or_default().paused_until = Some(pause.until);
            }
            let choice = choose_account(&AccountChoiceInput {
                allowed: &request.allowed,
                accounts: &accounts,
                states: &states,
                strategy: config.account_strategy,
                limit_percent: config.account_limit_percent,
                now: self.clock.now(),
                reserve,
                pending: &pending,
                reset_grace_minutes: config.account_reset_grace_minutes,
            });
            match choice {
                AccountChoice::Waiting { reason, resume_at } => {
                    return Ok(AccountPick::Waiting { reason, resume_at });
                }
                AccountChoice::Picked {
                    account,
                    slug,
                    why,
                    snapshot,
                } => {
                    // a pause that came in while the choice was made keeps the run off that account
                    if mode == PickMode::Start && self.paused_since(account, &pauses).await? {
                        continue;
                    }
                    return Ok(AccountPick::Picked(Chosen {
                        account,
                        slug,
                        why: with_reserve(&why, reserve, estimate.as_ref()),
                        snapshot,
                    }));
                }
            }
        }
        Ok(AccountPick::Waiting {
            reason: String::from("the accounts keep changing, try again"),
            resume_at: None,
        })
    }

    /// Whether the account has a pause now that the pauses read before did not show.
    async fn paused_since(
        &self,
        account: Id<Account>,
        seen: &[AccountPause],
    ) -> Result<bool, AppError> {
        let known = seen.iter().find(|pause| pause.account == account);
        Ok(self
            .pauses
            .active()
            .await?
            .iter()
            .any(|pause| pause.account == account && Some(pause) != known))
    }

    /// Checks that a chosen account is still free just before its run starts, else chooses again.
    pub async fn confirm(
        &self,
        chosen: &Chosen,
        request: &AccountRequest,
        config: &DispatcherConfig,
    ) -> Result<AccountPick, AppError> {
        let paused = self
            .pauses
            .active()
            .await?
            .iter()
            .any(|pause| pause.account == chosen.account);
        if paused {
            return self.pick(request, config, PickMode::Start).await;
        }
        Ok(AccountPick::Picked(chosen.clone()))
    }

    /// Logins and limits of the asked accounts, kept as samples when a run starts.
    async fn read_states(
        &self,
        all: &[Account],
        asked: &[Account],
        mode: PickMode,
    ) -> HashMap<Id<Account>, AccountState> {
        // limits are only asked once there is something to share out or an account has a ceiling
        let enabled = all.iter().filter(|account| account.enabled());
        let multi = enabled.clone().count() > 1
            || enabled.into_iter().any(|account| {
                account.limit_five_hour().is_some() || account.limit_week().is_some()
            });
        let mut states = HashMap::new();
        if !multi {
            return states;
        }
        let fresh = mode == PickMode::Start;
        for status in self.limits.status(asked, fresh).await {
            let usage = snapshot_of(&status.usage);
            if fresh && status.usage.available() {
                self.record(None, status.account, SamplePhase::Poll, usage)
                    .await;
            }
            states.insert(
                status.account,
                AccountState {
                    logged_in: Some(status.logged_in),
                    usage,
                    paused_until: None,
                },
            );
        }
        states
    }

    /// The reserve of the run, how it was estimated, and what runs under way still need.
    async fn reserves(
        &self,
        request: &AccountRequest,
        config: &DispatcherConfig,
        states: &HashMap<Id<Account>, AccountState>,
    ) -> Result<
        (
            AccountReserve,
            Option<EstimatedReserve>,
            HashMap<Id<Account>, AccountReserve>,
        ),
        AppError,
    > {
        let Some(profile) = &request.profile else {
            return Ok((AccountReserve::default(), None, HashMap::new()));
        };
        let samples = self.samples.history(SAMPLE_LIMIT).await?;
        let fallback = AccountReserve {
            five_hour: config.account_reserve_five_hour,
            week: config.account_reserve_week,
        };
        let own = estimate_reserve(
            &samples,
            &ReserveFor {
                profile,
                model: request.model.as_deref(),
                effort: request.effort.as_deref(),
            },
            fallback,
        );
        let running = self.running_work().await?;
        let pending = pending_reserve(&running, &samples, fallback, states);
        Ok((own.reserve, Some(own), pending))
    }

    /// Work runs under way with the use of their account at their start.
    async fn running_work(&self) -> Result<Vec<RunningWork>, AppError> {
        let active = self.runs.active().await?;
        let work: Vec<(Id<Run>, Id<Account>, &DispatchRun)> = active
            .iter()
            .filter_map(|run| {
                let dispatch = run.dispatch()?;
                let account = dispatch.account.map(Id::from_uuid)?;
                (dispatch.kind == RunKind::Work).then_some((run.id(), account, dispatch))
            })
            .collect();
        let ids: Vec<Id<Run>> = work.iter().map(|(id, ..)| *id).collect();
        let starts = self.samples.starts(&ids).await?;
        Ok(work
            .into_iter()
            .filter_map(|(id, account, dispatch)| {
                Some(RunningWork {
                    account,
                    profile: dispatch.profile.name.clone(),
                    model: dispatch.profile.model.clone(),
                    effort: dispatch.profile.effort.clone(),
                    start: *starts.get(&id)?,
                })
            })
            .collect())
    }

    /// Keeps how full the account was when the run started.
    pub async fn record_start(&self, run: Id<Run>, chosen: &Chosen) {
        self.record(
            Some(run),
            chosen.account,
            SamplePhase::Start,
            chosen.snapshot,
        )
        .await;
    }

    /// Keeps how full the account is when the run ended; the evaluation of runs calls it.
    pub async fn record_end(&self, run: Id<Run>, account: Id<Account>) {
        let Ok(account) = self.accounts.get(account).await else {
            return;
        };
        let Some(status) = self.limits.status(&[account], true).await.pop() else {
            return;
        };
        if !status.usage.available() {
            return;
        }
        self.record(
            Some(run),
            status.account,
            SamplePhase::End,
            snapshot_of(&status.usage),
        )
        .await;
    }

    async fn record(
        &self,
        run: Option<Id<Run>>,
        account: Id<Account>,
        phase: SamplePhase,
        usage: UsageSnapshot,
    ) {
        let sample = NewSample {
            id: self.ids.next_uuid(),
            account,
            run,
            phase,
            usage,
            at: self.clock.now(),
        };
        let stored = match phase {
            SamplePhase::Poll => self.samples.record_poll(&sample).await.map(|_| ()),
            SamplePhase::Start | SamplePhase::End => self.samples.record(&sample).await,
        };
        if let Err(error) = stored {
            tracing::warn!(%error, "cannot keep a usage sample");
        }
    }
}

/// The reason text with the reserve and expected duration the ticket view reads.
fn with_reserve(why: &str, reserve: AccountReserve, estimate: Option<&EstimatedReserve>) -> String {
    let Some(estimate) = estimate else {
        return why.to_owned();
    };
    let minutes = estimate.minutes.map_or_else(String::new, |minutes| {
        format!("; duration about {minutes} min")
    });
    format!(
        "{why}; reserve 5 h {} %, week {} % ({}){minutes}",
        reserve.five_hour, reserve.week, estimate.source
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reason_carries_the_reserve_and_the_duration() {
        let estimate = EstimatedReserve {
            reserve: AccountReserve {
                five_hour: 4,
                week: 1,
            },
            source: String::from("buffer"),
            minutes: Some(12),
        };
        let reserve = AccountReserve {
            five_hour: 4,
            week: 1,
        };
        assert_eq!(
            with_reserve("all accounts -> max", reserve, Some(&estimate)),
            "all accounts -> max; reserve 5 h 4 %, week 1 % (buffer); duration about 12 min"
        );
        assert_eq!(with_reserve("why", reserve, None), "why");
    }

    #[test]
    fn pause_reasons_use_the_stored_spelling() {
        assert_eq!(PauseReason::UsageLimit.as_str(), "usage-limit");
        assert_eq!(PauseReason::AuthMissing.as_str(), "auth-missing");
        assert_eq!("manual".parse::<PauseReason>(), Ok(PauseReason::Manual));
        assert_eq!(SamplePhase::Poll.as_str(), "poll");
    }

    #[test]
    fn an_unavailable_reading_knows_nothing() {
        let usage = AccountUsage::unavailable(crate::usage::UsageReason::NoLogin, None);
        assert_eq!(snapshot_of(&usage), UsageSnapshot::default());
    }
}
