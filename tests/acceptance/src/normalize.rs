use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

static UUID: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new("(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").ok()
});
static SECRET: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\b[0-9a-f]{64}\b").ok());
static PID: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\bpid \d+").ok());
/// The rest of a path behind a placeholder, so Windows and Unix record the same separators.
static PATH_TAIL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(<[A-Z]+>)((?:[\\/][^\s"'<>]*)+)"#).ok());

/// Replaces what differs between runs with stable placeholders; ids keep their identity.
#[derive(Debug, Default)]
pub(crate) struct Normalizer {
    paths: Vec<(String, String)>,
    literals: Vec<(Regex, String)>,
    ids: HashMap<String, usize>,
}

impl Normalizer {
    #[must_use]
    /// `paths` are matched in all their spellings, `literals` as written.
    pub(crate) fn new(paths: &[(&str, &Path)], literals: &[(&str, String)]) -> Self {
        let mut rules = Vec::new();
        for (name, path) in paths {
            for spelling in spellings(path) {
                rules.push((spelling, format!("<{name}>")));
            }
        }
        // the longest spelling first, so a parent never eats part of its child
        rules.sort_by_key(|(spelling, _)| std::cmp::Reverse(spelling.len()));
        // whole words only, so a version never matches inside a longer number
        let literals = literals
            .iter()
            .filter_map(|(name, literal)| {
                let pattern = format!(r"\b{}\b", regex::escape(literal));
                Regex::new(&pattern)
                    .ok()
                    .map(|found| (found, format!("<{name}>")))
            })
            .collect();
        Self {
            paths: rules,
            literals,
            ids: HashMap::new(),
        }
    }

    pub(crate) fn text(&mut self, text: &str) -> String {
        let mut text = text.replace("\r\n", "\n");
        for (spelling, placeholder) in &self.paths {
            text = text.replace(spelling.as_str(), placeholder);
        }
        for (literal, placeholder) in &self.literals {
            text = literal
                .replace_all(&text, placeholder.as_str())
                .into_owned();
        }
        if let Some(tail) = PATH_TAIL.as_ref() {
            text = tail
                .replace_all(&text, |found: &regex::Captures<'_>| {
                    let tail = found[2].replace("\\\\", "/").replace('\\', "/");
                    format!("{}{tail}", &found[1])
                })
                .into_owned();
        }
        if let Some(uuid) = UUID.as_ref() {
            let mut result = String::new();
            let mut last = 0;
            for found in uuid.find_iter(&text) {
                result.push_str(&text[last..found.start()]);
                let next = self.ids.len() + 1;
                let index = *self
                    .ids
                    .entry(found.as_str().to_lowercase())
                    .or_insert(next);
                result.push_str("<ID-");
                result.push_str(&index.to_string());
                result.push('>');
                last = found.end();
            }
            result.push_str(&text[last..]);
            text = result;
        }
        if let Some(secret) = SECRET.as_ref() {
            text = secret.replace_all(&text, "<SECRET>").into_owned();
        }
        if let Some(pid) = PID.as_ref() {
            text = pid.replace_all(&text, "pid <PID>").into_owned();
        }
        text
    }

    pub(crate) fn value(&mut self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(items) => {
                for item in items {
                    self.value(item);
                }
            }
            Value::Object(fields) => self.object(fields),
            _ => {}
        }
    }

    fn object(&mut self, fields: &mut Map<String, Value>) {
        for (key, field) in fields.iter_mut() {
            let moment = key == "at" || key.ends_with("At");
            if moment && field.is_number() {
                *field = Value::from("<TIME>");
            } else if matches!(key.as_str(), "pid" | "port") && field.is_number() {
                *field = Value::from(0);
            } else if key == "version" && field.is_string() {
                *field = Value::from("<VERSION>");
            } else {
                self.value(field);
            }
        }
    }
}

/// The ways a path may show up: as given, with forward slashes and fully resolved.
fn spellings(path: &Path) -> Vec<String> {
    let mut found = vec![path.to_string_lossy().into_owned()];
    if let Ok(resolved) = std::fs::canonicalize(path) {
        let text = resolved.to_string_lossy().into_owned();
        found.push(
            text.strip_prefix(r"\\?\")
                .map_or(text.clone(), str::to_owned),
        );
    }
    let slashed: Vec<String> = found.iter().map(|text| text.replace('\\', "/")).collect();
    // JSON printed as text doubles each backslash
    let escaped: Vec<String> = found
        .iter()
        .filter(|text| text.contains('\\'))
        .map(|text| text.replace('\\', "\\\\"))
        .collect();
    found.extend(slashed);
    found.extend(escaped);
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn ids_keep_their_identity_and_volatile_fields_become_stable() {
        let home = std::env::temp_dir();
        let mut normalizer = Normalizer::new(
            &[("HOME", &home)],
            &[("ADDRESS", String::from("127.0.0.1:9"))],
        );
        let first = "0190a0b0-0000-7000-8000-000000000001";
        let mut value = json!({
            "id": first,
            "parent": "0190A0B0-0000-7000-8000-000000000002",
            "again": first,
            "createdAt": 17,
            "pid": 4242,
            "version": "0.1.0",
            "rows": 2,
            "cwd": format!("{}/work", home.to_string_lossy().replace('\\', "/")),
            "code": "a".repeat(64),
        });
        normalizer.value(&mut value);
        assert_eq!(
            value,
            json!({
                "id": "<ID-1>",
                "parent": "<ID-2>",
                "again": "<ID-1>",
                "createdAt": "<TIME>",
                "pid": 0,
                "version": "<VERSION>",
                "rows": 2,
                "cwd": "<HOME>/work",
                "code": "<SECRET>",
            })
        );
        assert_eq!(normalizer.text("a\r\nb"), "a\nb");
        assert_eq!(
            normalizer.text("running at 127.0.0.1:9 (pid 4242)"),
            "running at <ADDRESS> (pid <PID>)"
        );
        // literals match whole words only, secrets whole words of exactly 64 digits
        assert_eq!(normalizer.text("at 127.0.0.19"), "at 127.0.0.19");
        assert_eq!(normalizer.text(&"b".repeat(128)), "b".repeat(128));
        if home.to_string_lossy().contains('\\') {
            let escaped = format!("{}\\\\data", home.to_string_lossy().replace('\\', "\\\\"));
            assert_eq!(normalizer.text(&escaped), "<HOME>/data");
        }
        let windows = format!("{}\\data\\runs x", home.to_string_lossy());
        assert_eq!(normalizer.text(&windows), "<HOME>/data/runs x");
    }
}
