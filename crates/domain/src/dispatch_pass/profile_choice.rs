use strum::{EnumString, IntoStaticStr};

use crate::{DispatcherConfig, Priority, Rule};

use super::readiness::profile_label;

/// Label prefix that forces a profile, such as `profile:large`.
pub const PROFILE_LABEL_PREFIX: &str = "profile:";

/// Why a profile was picked for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum ProfileReason {
    Default,
    Label,
    Plan,
    Rule,
}

impl ProfileReason {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// The base profile of a ticket before any escalation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChoice {
    pub profile: String,
    pub reason: ProfileReason,
    /// One-based place of the rule that matched.
    pub rule: Option<usize>,
    pub why: String,
}

fn word(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(text.len());
    &text[..end]
}

/// The planner's estimate: a line `Effort: <profile>` or the first word under `## Effort`.
#[must_use]
pub fn plan_estimate(plan: &str) -> Option<String> {
    for line in plan.lines() {
        let rest = line.trim_start().trim_start_matches('*');
        let Some(head) = rest.get(..6) else { continue };
        if !head.eq_ignore_ascii_case("effort") {
            continue;
        }
        let after = rest[6..].trim_start_matches('*').trim_start();
        if let Some(value) = after.strip_prefix(':') {
            let found = word(value.trim_start().trim_start_matches('*'));
            if !found.is_empty() {
                return Some(found.to_lowercase());
            }
        }
    }
    let mut lines = plan.lines();
    while let Some(line) = lines.next() {
        let heading = line.trim_end();
        let Some(rest) = heading.strip_prefix("##") else {
            continue;
        };
        if !rest.starts_with([' ', '\t']) || !rest.trim().eq_ignore_ascii_case("effort") {
            continue;
        }
        let below: String = lines.collect::<Vec<_>>().join("\n");
        let found = word(below.trim_start_matches(|c: char| c.is_whitespace() || c == '*'));
        return (!found.is_empty()).then(|| found.to_lowercase());
    }
    None
}

/// Rows of the table under `## Integration`, without header and separator.
#[must_use]
pub fn plan_file_count(plan: &str) -> usize {
    let mut lines = plan.lines().map(str::trim);
    let in_section = lines.by_ref().any(|line| {
        line.strip_prefix("##").is_some_and(|rest| {
            rest.starts_with([' ', '\t']) && rest.trim().to_lowercase().starts_with("integration")
        })
    });
    if !in_section {
        return 0;
    }
    let mut rows = 0;
    let mut separator = false;
    for line in lines {
        if line.starts_with("##") {
            break;
        }
        if !line.starts_with('|') {
            continue;
        }
        if line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
            separator = true;
        } else if separator {
            rows += 1;
        }
    }
    rows
}

fn matches(rule: &Rule, labels: &[String], priority: Priority, plan: &str) -> bool {
    let when = &rule.when;
    when.labels
        .as_ref()
        .is_none_or(|wanted| wanted.iter().any(|label| labels.contains(label)))
        && when
            .priorities
            .as_ref()
            .is_none_or(|wanted| wanted.iter().any(|found| found == priority.as_str()))
        && when
            .files_min
            .is_none_or(|min| i64::try_from(plan_file_count(plan)).is_ok_and(|count| count >= min))
        && when
            .plan_min_chars
            .is_none_or(|min| i64::try_from(plan.chars().count()).is_ok_and(|count| count >= min))
}

/// The profile of a first attempt: `profile:` label, the plan's estimate, a rule, the default.
#[must_use]
pub fn choose_profile(
    config: &DispatcherConfig,
    labels: &[String],
    priority: Priority,
    plan: &str,
) -> ProfileChoice {
    if let Some(name) = profile_label(labels).filter(|name| config.profile(name).is_some()) {
        return ProfileChoice {
            profile: name.to_owned(),
            reason: ProfileReason::Label,
            rule: None,
            why: format!("label {PROFILE_LABEL_PREFIX}{name}"),
        };
    }
    let estimate = plan_estimate(plan);
    if let Some(name) = estimate
        .as_deref()
        .filter(|name| config.profile(name).is_some())
    {
        return ProfileChoice {
            profile: name.to_owned(),
            reason: ProfileReason::Plan,
            rule: None,
            why: format!("effort from the plan: {name}"),
        };
    }
    let note = estimate.map_or_else(String::new, |text| {
        format!(" (effort \"{text}\" in the plan is no profile)")
    });
    if let Some((index, rule)) = config
        .rules
        .iter()
        .enumerate()
        .find(|(_, rule)| matches(rule, labels, priority, plan))
    {
        return ProfileChoice {
            profile: rule.profile.clone(),
            reason: ProfileReason::Rule,
            rule: Some(index + 1),
            why: format!("rule {} -> {}{note}", index + 1, rule.profile),
        };
    }
    ProfileChoice {
        profile: config.default_profile.clone(),
        reason: ProfileReason::Default,
        rule: None,
        why: format!("default profile {}{note}", config.default_profile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn choose(names: &[&str], priority: Priority, plan: &str) -> ProfileChoice {
        choose_profile(&DispatcherConfig::default(), &labels(names), priority, plan)
    }

    #[test]
    fn the_estimate_is_a_line_or_the_section_below_the_heading() {
        assert_eq!(
            plan_estimate("Goal\nEffort: Large - big"),
            Some("large".into())
        );
        assert_eq!(plan_estimate("**Effort**: **small**"), Some("small".into()));
        assert_eq!(
            plan_estimate("Steps\n\n## Effort\n\n**Medium**, some reason"),
            Some("medium".into())
        );
        assert_eq!(plan_estimate("## Effort   \nsmall"), Some("small".into()));
        assert_eq!(plan_estimate("effort is high"), None);
        assert_eq!(plan_estimate("## Effort\n"), None);
        assert_eq!(plan_estimate(""), None);
    }

    #[test]
    fn files_are_the_rows_of_the_integration_table() {
        let plan = "## Integration\n| File | Change |\n|---|---|\n| a.rs | x |\n| b.rs | y |\n\n## Tests\n| c | d |\n|-|-|\n| e | f |";
        assert_eq!(plan_file_count(plan), 2);
        assert_eq!(plan_file_count("## Tests\n| a |\n|-|\n| b |"), 0);
        assert_eq!(plan_file_count("## Integration\nnothing"), 0);
    }

    #[test]
    fn a_label_beats_the_plan_the_plan_beats_the_rules() {
        let choice = choose(
            &["auto", "profile:large"],
            Priority::Normal,
            "Effort: small",
        );
        assert_eq!(
            (choice.profile.as_str(), choice.reason),
            ("large", ProfileReason::Label)
        );
        let choice = choose(&["docs"], Priority::Normal, "Effort: large");
        assert_eq!(
            (choice.profile.as_str(), choice.reason),
            ("large", ProfileReason::Plan)
        );
        assert_eq!(choice.why, "effort from the plan: large");
    }

    #[test]
    fn rules_match_in_order_and_the_default_is_the_last_resort() {
        let choice = choose(&["docs"], Priority::Normal, "");
        assert_eq!(
            (choice.profile.as_str(), choice.rule, choice.reason),
            ("small", Some(1), ProfileReason::Rule)
        );
        assert_eq!(choice.why, "rule 1 -> small");
        let urgent = choose(&["auto"], Priority::Urgent, "");
        assert_eq!((urgent.profile.as_str(), urgent.rule), ("large", Some(3)));
        let many = "## Integration\n| f |\n|-|\n| 1 |\n| 2 |\n| 3 |\n| 4 |\n| 5 |\n| 6 |\n";
        assert_eq!(choose(&["auto"], Priority::Low, many).rule, Some(4));
        let plain = choose(&["auto"], Priority::Normal, "Effort: huge");
        assert_eq!(
            (plain.profile.as_str(), plain.reason, plain.rule),
            ("medium", ProfileReason::Default, None)
        );
        assert_eq!(
            plain.why,
            "default profile medium (effort \"huge\" in the plan is no profile)"
        );
    }

    #[test]
    fn an_unknown_profile_label_falls_through_to_the_rest() {
        let choice = choose(&["profile:huge", "docs"], Priority::Normal, "");
        assert_eq!(choice.profile, "small");
    }
}
