use strum::{EnumString, IntoStaticStr};

use crate::{TimeWindow, Timestamp, Version};

/// Why no new ticket starts right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdleReason {
    Disabled,
    Paused(Timestamp),
    OutsideWindow { from: String, to: String },
}

/// The first reason in the order disabled, paused, outside the window.
#[must_use]
pub fn idle_reason(
    enabled: bool,
    paused_until: Option<Timestamp>,
    window: Option<&TimeWindow>,
    now: Timestamp,
    minute_of_day: u32,
) -> Option<IdleReason> {
    if !enabled {
        return Some(IdleReason::Disabled);
    }
    if let Some(until) = paused_until.filter(|until| *until > now) {
        return Some(IdleReason::Paused(until));
    }
    window
        .filter(|window| !window.contains(minute_of_day))
        .map(|window| IdleReason::OutsideWindow {
            from: window.from.clone(),
            to: window.to.clone(),
        })
}

/// The dispatcher's state in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum DispatcherPhase {
    Off,
    Idle,
    Running,
    Paused,
    OutsideWindow,
}

impl DispatcherPhase {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Phase from the idle reason and the runs that count for the limit.
#[must_use]
pub fn phase(idle: Option<&IdleReason>, running: usize) -> DispatcherPhase {
    match idle {
        Some(IdleReason::Disabled) => DispatcherPhase::Off,
        Some(IdleReason::Paused(_)) => DispatcherPhase::Paused,
        Some(IdleReason::OutsideWindow { .. }) => DispatcherPhase::OutsideWindow,
        None if running > 0 => DispatcherPhase::Running,
        None => DispatcherPhase::Idle,
    }
}

/// Every stored field of the dispatcher's runtime state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatcherStateRecord {
    /// Set by enable and disable; none follows the configuration.
    pub enabled_override: Option<bool>,
    pub paused_until: Option<Timestamp>,
    pub last_pass_at: Option<Timestamp>,
    /// What the last pass did, as the app wrote it.
    pub last_pass: Option<String>,
    pub version: Version,
}

/// The singleton runtime state: the switch, the pause and the last pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatcherState(DispatcherStateRecord);

impl Default for DispatcherState {
    fn default() -> Self {
        Self(DispatcherStateRecord {
            enabled_override: None,
            paused_until: None,
            last_pass_at: None,
            last_pass: None,
            version: Version::INITIAL,
        })
    }
}

impl DispatcherState {
    #[must_use]
    pub const fn restore(record: DispatcherStateRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &DispatcherStateRecord {
        &self.0
    }

    /// Whether new tickets may start: the switch when set, else the configuration.
    #[must_use]
    pub fn enabled(&self, configured: bool) -> bool {
        self.0.enabled_override.unwrap_or(configured)
    }

    /// The pause, while it lies in the future.
    #[must_use]
    pub fn paused_until(&self, now: Timestamp) -> Option<Timestamp> {
        self.0.paused_until.filter(|until| *until > now)
    }

    fn next(&self, change: impl FnOnce(&mut DispatcherStateRecord)) -> Self {
        let mut record = self.0.clone();
        change(&mut record);
        record.version = self.0.version.next();
        Self(record)
    }

    #[must_use]
    pub fn with_enabled(&self, enabled: Option<bool>) -> Self {
        self.next(|record| record.enabled_override = enabled)
    }

    #[must_use]
    pub fn paused(&self, until: Timestamp) -> Self {
        self.next(|record| record.paused_until = Some(until))
    }

    #[must_use]
    pub fn resumed(&self) -> Self {
        self.next(|record| record.paused_until = None)
    }

    #[must_use]
    pub fn passed(&self, at: Timestamp, report: String) -> Self {
        self.next(|record| {
            record.last_pass_at = Some(at);
            record.last_pass = Some(report);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    #[test]
    fn the_idle_reason_follows_the_order() {
        let window = TimeWindow {
            from: String::from("08:00"),
            to: String::from("18:00"),
        };
        let now = at(1_000);
        assert_eq!(
            idle_reason(false, Some(at(2_000)), Some(&window), now, 60),
            Some(IdleReason::Disabled)
        );
        assert_eq!(
            idle_reason(true, Some(at(2_000)), Some(&window), now, 60),
            Some(IdleReason::Paused(at(2_000)))
        );
        assert_eq!(
            idle_reason(true, Some(at(500)), Some(&window), now, 60),
            Some(IdleReason::OutsideWindow {
                from: String::from("08:00"),
                to: String::from("18:00")
            })
        );
        assert_eq!(idle_reason(true, None, Some(&window), now, 9 * 60), None);
        assert_eq!(idle_reason(true, None, None, now, 0), None);
    }

    #[test]
    fn the_phase_names_what_the_dispatcher_does() {
        assert_eq!(phase(Some(&IdleReason::Disabled), 3), DispatcherPhase::Off);
        assert_eq!(
            phase(Some(&IdleReason::Paused(at(1))), 0),
            DispatcherPhase::Paused
        );
        assert_eq!(phase(None, 2), DispatcherPhase::Running);
        assert_eq!(phase(None, 0), DispatcherPhase::Idle);
        assert_eq!(DispatcherPhase::OutsideWindow.as_str(), "outside-window");
    }

    #[test]
    fn every_change_bumps_the_version() {
        let state = DispatcherState::default();
        assert!(state.enabled(true));
        assert!(!state.enabled(false));
        let off = state.with_enabled(Some(false));
        assert!(!off.enabled(true));
        assert_eq!(off.record().version, Version::from_stored(2));
        let paused = off.paused(at(9_000));
        assert_eq!(paused.paused_until(at(1_000)), Some(at(9_000)));
        assert_eq!(paused.paused_until(at(9_000)), None);
        let resumed = paused.resumed();
        assert_eq!(resumed.record().paused_until, None);
        let recorded = resumed.passed(at(5), String::from("{}"));
        assert_eq!(recorded.record().last_pass_at, Some(at(5)));
        assert_eq!(recorded.record().version, Version::from_stored(5));
    }
}
