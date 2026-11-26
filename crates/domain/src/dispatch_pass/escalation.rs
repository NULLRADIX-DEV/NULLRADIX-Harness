use super::profile_choice::ProfileChoice;
use crate::{DispatcherConfig, effort_problem, model_problem};

const MODEL_LABEL: &str = "model:";
const EFFORT_LABEL: &str = "effort:";

/// Model and effort a ticket asks for with its labels; they override the profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelOverrides {
    pub model: Option<String>,
    pub effort: Option<String>,
}

fn first_value<'a>(labels: &'a [String], prefix: &str) -> Option<&'a str> {
    labels
        .iter()
        .find_map(|label| label.strip_prefix(prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// The valid `model:` and `effort:` labels; readiness refuses an invalid one.
#[must_use]
pub fn label_overrides(config: &DispatcherConfig, labels: &[String]) -> LabelOverrides {
    LabelOverrides {
        model: first_value(labels, MODEL_LABEL)
            .filter(|model| model_problem(model, config.allow_fable).is_none())
            .map(str::to_owned),
        effort: first_value(labels, EFFORT_LABEL)
            .filter(|effort| effort_problem(effort).is_none())
            .map(str::to_owned),
    }
}

/// A `model:` or `effort:` label the dispatcher cannot start with.
#[must_use]
pub fn label_problem(config: &DispatcherConfig, labels: &[String]) -> Option<String> {
    if let Some(model) = first_value(labels, MODEL_LABEL)
        && let Some(problem) = model_problem(model, config.allow_fable)
    {
        return Some(format!("label {MODEL_LABEL}{model} - {problem}"));
    }
    if let Some(effort) = first_value(labels, EFFORT_LABEL)
        && let Some(problem) = effort_problem(effort)
    {
        return Some(format!("label {EFFORT_LABEL}{effort} - {problem}"));
    }
    None
}

impl LabelOverrides {
    /// What the labels add to the reason of the profile choice.
    #[must_use]
    pub fn note(&self) -> String {
        let model = self
            .model
            .as_ref()
            .map(|model| format!(", label {MODEL_LABEL}{model}"));
        let effort = self
            .effort
            .as_ref()
            .map(|effort| format!(", label {EFFORT_LABEL}{effort}"));
        model.into_iter().chain(effort).collect()
    }
}

/// Lifts the profile one ladder step per earlier abort; the top step repeats.
#[must_use]
pub fn escalate(
    config: &DispatcherConfig,
    mut choice: ProfileChoice,
    attempt: usize,
) -> ProfileChoice {
    if !config.escalate || attempt <= 1 {
        return choice;
    }
    let Some(at) = config
        .ladder
        .iter()
        .position(|name| *name == choice.profile)
    else {
        return choice;
    };
    let top = config.ladder.len() - 1;
    let target = &config.ladder[at.saturating_add(attempt - 1).min(top)];
    if *target == choice.profile || config.profile(target).is_none() {
        return choice;
    }
    choice.why = format!("{}, attempt {attempt}: escalated to {target}", choice.why);
    choice.escalated_from = Some(std::mem::replace(&mut choice.profile, target.clone()));
    choice
}

#[cfg(test)]
mod tests {
    use super::super::profile_choice::choose_profile;
    use super::*;
    use crate::Priority;

    fn choice(labels: &[&str]) -> (DispatcherConfig, ProfileChoice) {
        let config = DispatcherConfig::default();
        let labels: Vec<String> = labels.iter().map(|label| (*label).to_owned()).collect();
        let choice = choose_profile(&config, &labels, Priority::Normal, "");
        (config, choice)
    }

    fn strings(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| (*label).to_owned()).collect()
    }

    #[test]
    fn each_earlier_abort_climbs_one_step_and_the_top_repeats() {
        let (config, base) = choice(&["docs"]);
        assert_eq!(base.profile, "small");
        let first = escalate(&config, base.clone(), 1);
        assert_eq!(
            (first.profile.as_str(), first.escalated_from),
            ("small", None)
        );
        let second = escalate(&config, base.clone(), 2);
        assert_eq!(second.profile, "medium");
        assert_eq!(second.escalated_from.as_deref(), Some("small"));
        assert_eq!(
            second.why,
            "rule 1 -> small, attempt 2: escalated to medium"
        );
        assert_eq!(escalate(&config, base.clone(), 3).profile, "large");
        assert_eq!(escalate(&config, base, 9).profile, "large");
    }

    #[test]
    fn the_top_step_is_not_an_escalation_and_a_switch_turns_it_off() {
        let (mut config, base) = choice(&["architecture"]);
        assert_eq!(base.profile, "large");
        let top = escalate(&config, base.clone(), 3);
        assert_eq!((top.profile.as_str(), top.escalated_from), ("large", None));
        assert_eq!(top.why, base.why);
        let (_, small) = choice(&["docs"]);
        config.escalate = false;
        assert_eq!(escalate(&config, small, 3).profile, "small");
    }

    #[test]
    fn a_profile_off_the_ladder_stays() {
        let (mut config, base) = choice(&["docs"]);
        config.ladder = vec![String::from("medium"), String::from("large")];
        assert_eq!(escalate(&config, base, 2).profile, "small");
    }

    #[test]
    fn valid_labels_override_and_say_so() {
        let config = DispatcherConfig::default();
        let labels = strings(&["auto", "model:haiku", "effort: low", "model:opus"]);
        let overrides = label_overrides(&config, &labels);
        assert_eq!(overrides.model.as_deref(), Some("haiku"));
        assert_eq!(overrides.effort.as_deref(), Some("low"));
        assert_eq!(overrides.note(), ", label model:haiku, label effort:low");
        assert_eq!(label_problem(&config, &labels), None);
    }

    #[test]
    fn invalid_labels_are_ignored_but_stop_readiness() {
        let config = DispatcherConfig::default();
        let labels = strings(&["model:gpt", "effort:extreme"]);
        assert_eq!(label_overrides(&config, &labels), LabelOverrides::default());
        let model = label_problem(&config, &labels).unwrap();
        assert!(model.starts_with("label model:gpt - unknown model"));
        let effort = label_problem(&config, &strings(&["effort:extreme"])).unwrap();
        assert!(effort.starts_with("label effort:extreme - unknown effort"));
        assert_eq!(label_problem(&config, &strings(&["model:"])), None);
        assert!(label_problem(&config, &strings(&["model:fable"])).is_some());
    }
}
