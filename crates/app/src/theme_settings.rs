//! Values of theme settings: the stored form and the refusal of bad input.

use std::collections::BTreeMap;

use nullradix_domain::{FieldError, SettingValue, ThemeSettingValues};
use serde_json::{Map, Value};

use crate::{AppError, FieldProblem};

#[must_use]
pub fn settings_invalid(errors: Vec<FieldError>) -> AppError {
    let fields = errors
        .into_iter()
        .map(|error| FieldProblem {
            field: error.field,
            message: error.message,
        })
        .collect();
    AppError::invalid(
        "theme-settings-invalid",
        "the values do not match the settings the theme declares",
    )
    .with_fields(fields)
}

/// The values as the JSON object that is stored.
#[must_use]
pub fn settings_json(values: &ThemeSettingValues) -> Value {
    let object: Map<String, Value> = values
        .values()
        .iter()
        .map(|(key, value)| {
            let json = match value {
                SettingValue::Switch(on) => Value::Bool(*on),
                SettingValue::Choice(text) => Value::String(text.clone()),
            };
            (key.clone(), json)
        })
        .collect();
    Value::Object(object)
}

/// Reads a stored object; `None` unless every value is a switch or a text.
#[must_use]
pub fn parse_settings(json: &str) -> Option<ThemeSettingValues> {
    let Value::Object(object) = serde_json::from_str(json).ok()? else {
        return None;
    };
    let values: Option<BTreeMap<String, SettingValue>> = object
        .into_iter()
        .map(|(key, value)| match value {
            Value::Bool(on) => Some((key, SettingValue::Switch(on))),
            Value::String(text) => Some((key, SettingValue::Choice(text))),
            _ => None,
        })
        .collect();
    values.map(ThemeSettingValues::restore)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_through_their_stored_form() {
        let values = ThemeSettingValues::restore(BTreeMap::from([
            (String::from("dim"), SettingValue::Switch(false)),
            (
                String::from("mode"),
                SettingValue::Choice(String::from("b")),
            ),
        ]));
        let json = settings_json(&values).to_string();
        assert_eq!(json, r#"{"dim":false,"mode":"b"}"#);
        assert_eq!(parse_settings(&json), Some(values));
    }

    #[test]
    fn anything_but_switches_and_texts_is_unreadable() {
        for json in ["[]", r#"{"a":1}"#, r#"{"a":null}"#, r#"{"a":["x"]}"#, "{"] {
            assert_eq!(parse_settings(json), None, "{json}");
        }
    }

    #[test]
    fn refused_fields_travel_with_the_error() {
        let error = settings_invalid(vec![FieldError::new("dim", "no")]);
        assert_eq!(error.code(), "theme-settings-invalid");
        assert_eq!(error.fields()[0].field, "dim");
    }
}
