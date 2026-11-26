//! Values a person chose for the settings an installed theme declares.

use std::collections::BTreeMap;

use crate::theme::{FieldError, SETTING_KEY_MAX, SETTINGS_MAX, SettingSpec};

/// Longest text a choice may carry.
pub const SETTING_VALUE_MAX: usize = 1000;

/// A switch is on or off; a choice is one option of a select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    Switch(bool),
    Choice(String),
}

/// Checked values of one theme, by setting key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeSettingValues(BTreeMap<String, SettingValue>);

impl ThemeSettingValues {
    /// Accepts declared keys with a value of their type; a select takes one of its options.
    pub fn check(
        specs: Option<&BTreeMap<String, SettingSpec>>,
        input: BTreeMap<String, SettingValue>,
    ) -> Result<Self, Vec<FieldError>> {
        if input.len() > SETTINGS_MAX {
            return Err(vec![FieldError::new(
                "values",
                format!("a theme has at most {SETTINGS_MAX} settings"),
            )]);
        }
        let mut errors = Vec::new();
        for (key, value) in &input {
            if key.chars().count() > SETTING_KEY_MAX {
                errors.push(FieldError::new(
                    "values",
                    format!("a key has at most {SETTING_KEY_MAX} characters"),
                ));
                continue;
            }
            let spec = specs.and_then(|specs| specs.get(key));
            if let Some(message) = problem(spec, value) {
                errors.push(FieldError::new(key.as_str(), message));
            }
        }
        if errors.is_empty() {
            Ok(Self(input))
        } else {
            Err(errors)
        }
    }

    /// Keeps the stored entries that still fit the specs; the rest is dropped.
    #[must_use]
    pub fn fitting(self, specs: Option<&BTreeMap<String, SettingSpec>>) -> Self {
        let mut values = self.0;
        values
            .retain(|key, value| problem(specs.and_then(|specs| specs.get(key)), value).is_none());
        Self(values)
    }

    /// Trusts values read back from storage.
    #[must_use]
    pub const fn restore(values: BTreeMap<String, SettingValue>) -> Self {
        Self(values)
    }

    #[must_use]
    pub const fn values(&self) -> &BTreeMap<String, SettingValue> {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn problem(spec: Option<&SettingSpec>, value: &SettingValue) -> Option<String> {
    match (spec, value) {
        (None, _) => Some(String::from("the theme declares no such setting")),
        (Some(SettingSpec::Boolean { .. }), SettingValue::Switch(_)) => None,
        (Some(SettingSpec::Boolean { .. }), SettingValue::Choice(_)) => {
            Some(String::from("the value must be true or false"))
        }
        (Some(SettingSpec::Select { .. }), SettingValue::Switch(_)) => {
            Some(String::from("the value must be one of the options"))
        }
        (Some(SettingSpec::Select { options, .. }), SettingValue::Choice(text)) => {
            if text.chars().count() > SETTING_VALUE_MAX {
                return Some(format!(
                    "the value has at most {SETTING_VALUE_MAX} characters"
                ));
            }
            (!options.iter().any(|option| &option.value == text))
                .then(|| String::from("the value is none of the options"))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::SelectOption;

    use super::*;

    fn specs() -> BTreeMap<String, SettingSpec> {
        let option = |value: &str| SelectOption {
            value: value.to_owned(),
            label: value.to_uppercase(),
        };
        BTreeMap::from([
            (
                String::from("dim"),
                SettingSpec::Boolean {
                    label: String::from("Dim"),
                    hint: None,
                    default: true,
                },
            ),
            (
                String::from("mode"),
                SettingSpec::Select {
                    label: String::from("Mode"),
                    hint: None,
                    options: vec![option("a"), option("b")],
                    default: String::from("a"),
                },
            ),
        ])
    }

    fn input(items: &[(&str, SettingValue)]) -> BTreeMap<String, SettingValue> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    fn refused(
        specs: Option<&BTreeMap<String, SettingSpec>>,
        items: &[(&str, SettingValue)],
    ) -> Vec<FieldError> {
        ThemeSettingValues::check(specs, input(items)).unwrap_err()
    }

    fn fields(
        specs: Option<&BTreeMap<String, SettingSpec>>,
        items: &[(&str, SettingValue)],
    ) -> Vec<String> {
        refused(specs, items)
            .into_iter()
            .map(|error| error.field)
            .collect()
    }

    fn choice(text: &str) -> SettingValue {
        SettingValue::Choice(text.to_owned())
    }

    #[test]
    fn declared_keys_with_matching_values_are_accepted() {
        let specs = specs();
        let values = ThemeSettingValues::check(
            Some(&specs),
            input(&[("dim", SettingValue::Switch(false)), ("mode", choice("b"))]),
        )
        .unwrap();
        assert_eq!(values.values().len(), 2);
        let none = ThemeSettingValues::check(Some(&specs), BTreeMap::new()).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn each_refused_key_is_reported_by_its_own_field() {
        let specs = specs();
        let fields_of = fields(
            Some(&specs),
            &[
                ("dim", choice("yes")),
                ("mode", SettingValue::Switch(true)),
                ("other", choice("a")),
            ],
        );
        assert_eq!(fields_of, ["dim", "mode", "other"]);
        assert_eq!(fields(Some(&specs), &[("mode", choice("c"))]), ["mode"]);
    }

    #[test]
    fn stored_entries_that_no_longer_fit_are_dropped() {
        let specs = specs();
        let stored = ThemeSettingValues::restore(input(&[
            ("dim", choice("yes")),
            ("mode", choice("b")),
            ("gone", SettingValue::Switch(true)),
        ]));
        let kept = stored.clone().fitting(Some(&specs));
        assert_eq!(kept.values(), &input(&[("mode", choice("b"))]));
        assert!(stored.fitting(None).is_empty());
    }

    #[test]
    fn a_theme_without_settings_accepts_no_key() {
        let fields_of = fields(None, &[("dim", SettingValue::Switch(true))]);
        assert_eq!(fields_of, ["dim"]);
    }

    #[test]
    fn counts_and_lengths_are_bounded() {
        let specs = specs();
        let many: BTreeMap<String, SettingValue> = (0..=SETTINGS_MAX)
            .map(|index| (format!("k{index}"), SettingValue::Switch(true)))
            .collect();
        let errors = ThemeSettingValues::check(Some(&specs), many).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, "values");

        let long_key = "k".repeat(SETTING_KEY_MAX + 1);
        let fields_of = fields(Some(&specs), &[(long_key.as_str(), choice("a"))]);
        assert_eq!(fields_of, ["values"]);
        let wide = "\u{e9}".repeat(SETTING_KEY_MAX);
        assert!(
            ThemeSettingValues::check(None, input(&[(wide.as_str(), choice("a"))]))
                .is_err_and(|errors| errors[0].field == wide)
        );

        let long = "a".repeat(SETTING_VALUE_MAX + 1);
        let errors = refused(Some(&specs), &[("mode", choice(&long))]);
        assert!(errors[0].message.contains("at most"));
        let edge = "a".repeat(SETTING_VALUE_MAX);
        let errors = refused(Some(&specs), &[("mode", choice(&edge))]);
        assert!(errors[0].message.contains("none of the options"));
    }
}
