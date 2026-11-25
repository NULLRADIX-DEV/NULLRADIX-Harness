use std::collections::HashMap;

use thiserror::Error;

/// Values of the placeholders, by name.
pub type PromptValues = HashMap<String, String>;

/// A template names placeholders that have no value.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown placeholders in the template: {}", .0.iter().map(|name| format!("{{{{{name}}}}}")).collect::<Vec<_>>().join(", "))]
pub struct UnknownPlaceholders(pub Vec<String>);

/// One `{{name}}` in a text: its byte range and name.
struct Span<'a> {
    start: usize,
    end: usize,
    name: &'a str,
}

fn scan(text: &str) -> Vec<Span<'_>> {
    let mut spans = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find("{{") {
        let start = at + found;
        let name_start = start + 2;
        let name_len = text[name_start..]
            .char_indices()
            .find(|(index, character)| {
                if *index == 0 {
                    !character.is_ascii_alphabetic()
                } else {
                    !(character.is_ascii_alphanumeric() || *character == '_')
                }
            })
            .map_or(text.len() - name_start, |(index, _)| index);
        let name_end = name_start + name_len;
        if name_len > 0 && text[name_end..].starts_with("}}") {
            spans.push(Span {
                start,
                end: name_end + 2,
                name: &text[name_start..name_end],
            });
            at = name_end + 2;
        } else {
            at = start + 1;
        }
    }
    spans
}

/// The distinct placeholder names of a template in order of first use.
#[must_use]
pub fn placeholder_names(template: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for span in scan(template) {
        if !names.iter().any(|known| known == span.name) {
            names.push(span.name.to_owned());
        }
    }
    names
}

/// Line endings stay as they are; the last line may have none.
fn lines_with_endings(text: &str) -> Vec<(&str, &str)> {
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find(['\r', '\n']) {
            None => {
                parts.push((rest, ""));
                rest = "";
            }
            Some(at) => {
                let ending = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
                parts.push((&rest[..at], &rest[at..at + ending]));
                rest = &rest[at + ending..];
            }
        }
    }
    parts
}

fn value_of<'a>(values: &'a PromptValues, name: &str) -> &'a str {
    values.get(name).map_or("", String::as_str)
}

/// The name when the line holds nothing but one placeholder.
fn block_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let spans = scan(trimmed);
    match spans.as_slice() {
        [only] if only.start == 0 && only.end == trimmed.len() => Some(only.name),
        _ => None,
    }
}

/// Replaces `{{name}}`; a line that holds only an empty placeholder disappears.
pub fn render_template(
    template: &str,
    values: &PromptValues,
) -> Result<String, UnknownPlaceholders> {
    let unknown: Vec<String> = placeholder_names(template)
        .into_iter()
        .filter(|name| !values.contains_key(name))
        .collect();
    if !unknown.is_empty() {
        return Err(UnknownPlaceholders(unknown));
    }
    let mut out = String::new();
    for (line, ending) in lines_with_endings(template) {
        if block_name(line).is_some_and(|name| value_of(values, name).is_empty()) {
            continue;
        }
        let mut at = 0;
        for span in scan(line) {
            out.push_str(&line[at..span.start]);
            out.push_str(value_of(values, span.name));
            at = span.end;
        }
        out.push_str(&line[at..]);
        out.push_str(ending);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> PromptValues {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn names_are_distinct_and_in_order() {
        assert_eq!(
            placeholder_names("{{b}} {{a}} {{b}} {{ c }} {{1x}} {{x-y}} {{ok_1}}"),
            ["b", "a", "ok_1"]
        );
    }

    #[test]
    fn replaces_every_occurrence() {
        let out = render_template(
            "Ticket {{id}} for {{who}} ({{id}})",
            &values(&[("id", "ARC-9"), ("who", "worker")]),
        );
        assert_eq!(out.as_deref(), Ok("Ticket ARC-9 for worker (ARC-9)"));
    }

    #[test]
    fn unknown_placeholders_are_named_once_each() {
        let error = render_template("{{a}} {{b}} {{b}}", &values(&[("a", "1")])).unwrap_err();
        assert_eq!(error.0, ["b"]);
        assert_eq!(
            error.to_string(),
            "unknown placeholders in the template: {{b}}"
        );
    }

    #[test]
    fn an_empty_block_line_disappears_with_its_ending() {
        let template = "before\r\n  {{block}}  \r\nafter {{inline}}.\nend";
        let empty = render_template(template, &values(&[("block", ""), ("inline", "")]));
        assert_eq!(empty.as_deref(), Ok("before\r\nafter .\nend"));
        let full = render_template(template, &values(&[("block", "x\ny"), ("inline", "!")]));
        assert_eq!(full.as_deref(), Ok("before\r\n  x\ny  \r\nafter !.\nend"));
    }

    #[test]
    fn an_empty_inline_placeholder_keeps_its_line() {
        let out = render_template("a {{x}}\nb", &values(&[("x", "")]));
        assert_eq!(out.as_deref(), Ok("a \nb"));
    }

    #[test]
    fn a_third_brace_does_not_hide_the_placeholder() {
        assert_eq!(placeholder_names("{{{a}}"), ["a"]);
    }

    #[test]
    fn text_without_placeholders_and_odd_braces_stays() {
        let out = render_template("{ } {{ {{a}} }}", &values(&[("a", "1")]));
        assert_eq!(out.as_deref(), Ok("{ } {{ 1 }}"));
    }
}
