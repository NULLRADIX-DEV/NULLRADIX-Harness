//! The tests an agent ran, read from the stream-json output of its run, as legacy did.

use std::sync::LazyLock;

use nullradix_domain::{REVIEW_TESTS_MAX, TestRun};
use regex::Regex;
use serde_json::Value;

/// Characters of the output kept per test run; the end stays.
pub const MAX_TEST_OUTPUT: usize = 4000;

/// Tools whose calls run commands.
const SHELL_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

/// The start of a command: line start or after a separator, behind an optional runner.
const START: &str = r"(?:^|[;&|(]\s*)(?:npx\s+|python3?\s+-m\s+|\./)?";

/// Commands that count as a test run: tests, build, lint and type check.
static TEST_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(vitest|jest|pytest)\b",
        r"npm\s+(test|t)\b",
        r"npm\s+run\s+(test|build|lint|typecheck)\b",
        r"tsc\b",
        r"dotnet\s+test\b",
        r"go\s+test\b",
        r"cargo\s+test\b",
        r"(mvn|mvnw|gradle|gradlew)\b.*\btest\b",
    ]
    .into_iter()
    .filter_map(|pattern| Regex::new(&format!("{START}{pattern}")).ok())
    .collect()
});

/// Claude Code's answers when its permission check refuses a call.
const DENIAL_MARKERS: [&str; 3] = [
    "requires approval",
    "requested permissions to use",
    "haven't granted it yet",
];

/// Whether a command is a test run.
#[must_use]
pub fn is_test(command: &str) -> bool {
    TEST_PATTERNS
        .iter()
        .any(|pattern| pattern.is_match(command))
}

/// The last `max` characters of `text`.
fn last_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(max)).collect()
}

/// The test runs of one run, in the order of the calls; lines that are no JSON are skipped.
#[derive(Debug, Default)]
pub struct TestRuns {
    runs: Vec<(String, TestRun)>,
}

impl TestRuns {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one whole output line.
    pub fn feed(&mut self, line: &str) {
        let text = line.trim();
        if !text.starts_with('{')
            || !(text.contains("tool_use")
                || text.contains("tool_result")
                || text.contains("permission_denials"))
        {
            return;
        }
        let Ok(event) = serde_json::from_str::<Value>(text) else {
            return;
        };
        for id in denied_ids(&event) {
            self.runs.retain(|(known, _)| *known != id);
        }
        let blocks = event
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array);
        for block in blocks
            .into_iter()
            .flatten()
            .filter(|block| block.is_object())
        {
            self.block(block);
        }
    }

    fn block(&mut self, block: &Value) {
        let kind = block.get("type").and_then(Value::as_str);
        if kind == Some("tool_use") {
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let Some(id) = block.get("id").and_then(Value::as_str) else {
                return;
            };
            let command = command(block.get("input"));
            if SHELL_TOOLS.contains(&name) && !command.is_empty() && is_test(&command) {
                self.runs.retain(|(known, _)| known != id);
                if self.runs.len() == REVIEW_TESTS_MAX {
                    self.runs.remove(0);
                }
                self.runs.push((
                    id.to_owned(),
                    TestRun {
                        command,
                        ok: None,
                        output: String::new(),
                    },
                ));
            }
        } else if kind == Some("tool_result") {
            let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                return;
            };
            let Some(at) = self.runs.iter().position(|(known, _)| known == id) else {
                return;
            };
            let failed = block.get("is_error") == Some(&Value::Bool(true));
            let content = text(block.get("content"));
            // refused by the permission check: the command never ran, so it is no test run
            if failed && is_denial(&content) {
                self.runs.remove(at);
                return;
            }
            let run = &mut self.runs[at].1;
            run.ok = Some(!failed);
            run.output = last_chars(&content, MAX_TEST_OUTPUT);
        }
    }

    #[must_use]
    pub fn finish(self) -> Vec<TestRun> {
        self.runs.into_iter().map(|(_, run)| run).collect()
    }
}

fn is_denial(content: &str) -> bool {
    DENIAL_MARKERS.iter().any(|marker| content.contains(marker))
}

/// The calls the result event of a run lists under `permission_denials`.
fn denied_ids(event: &Value) -> Vec<String> {
    event
        .get("permission_denials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|denial| denial.get("tool_use_id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn command(input: Option<&Value>) -> String {
    input
        .and_then(|input| input.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// Text of a tool result, which is a string or a list of text blocks.
fn text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .collect::<Vec<&str>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(id: &str, command: &str, name: &str) -> String {
        json!({ "type": "assistant", "message": { "content": [
            { "type": "tool_use", "id": id, "name": name, "input": { "command": command } },
        ] } })
        .to_string()
    }

    fn result(id: &str, content: &Value, is_error: bool) -> String {
        json!({ "type": "user", "message": { "content": [
            { "type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error },
        ] } })
        .to_string()
    }

    fn run(command: &str, ok: Option<bool>, output: &str) -> TestRun {
        TestRun {
            command: command.to_owned(),
            ok,
            output: output.to_owned(),
        }
    }

    fn extract(lines: &[String]) -> Vec<TestRun> {
        let mut runs = TestRuns::new();
        for line in lines {
            runs.feed(line);
        }
        runs.finish()
    }

    #[test]
    fn pairs_test_calls_with_their_result() {
        let lines = [
            call("a", "npx vitest run packages/core", "Bash"),
            result("a", &json!("Tests 12 passed"), false),
            call("b", "git status --short", "Bash"),
            result("b", &json!(" M x.ts"), false),
            call("c", "npm test", "PowerShell"),
            result("c", &json!("FAIL x.test.ts"), true),
            String::from("not a json line"),
        ];
        assert_eq!(
            extract(&lines),
            vec![
                run(
                    "npx vitest run packages/core",
                    Some(true),
                    "Tests 12 passed"
                ),
                run("npm test", Some(false), "FAIL x.test.ts"),
            ]
        );
    }

    #[test]
    fn knows_common_test_commands_and_nothing_else() {
        for command in [
            "npm run test:unit",
            "npm run build",
            "npm run lint",
            "npm run typecheck",
            "npx tsc --noEmit -p web",
            "tsc --noEmit",
            "npx jest",
            "dotnet test Foo.sln",
            "pytest -q",
            "python -m pytest",
            "go test ./...",
            "cargo test",
            "mvn test",
            "./gradlew test",
            "cd web && npm test",
        ] {
            assert!(is_test(command), "{command}");
        }
        for command in [
            "git status",
            "npm ci",
            "npm install",
            "ls tests",
            "cat vitest.config.ts",
            "nullradix ticket show ARC-0001",
        ] {
            assert!(!is_test(command), "{command}");
        }
    }

    #[test]
    fn refused_calls_are_no_test_runs() {
        let lines = [
            call("a", "cd x && python -m pytest --version", "Bash"),
            result(
                "a",
                &json!(
                    "This Bash command contains multiple operations. The following part requires approval: cd x"
                ),
                true,
            ),
            call("b", "npm test", "Bash"),
            call("c", "cargo test", "Bash"),
            result("c", &json!("ok"), false),
            json!({ "type": "result", "subtype": "success", "is_error": false,
                "permission_denials": [{ "tool_name": "Bash", "tool_use_id": "b" }] })
            .to_string(),
        ];
        assert_eq!(extract(&lines), vec![run("cargo test", Some(true), "ok")]);
    }

    #[test]
    fn a_call_without_result_has_none() {
        assert_eq!(
            extract(&[call("a", "npm test", "Bash")]),
            vec![run("npm test", None, "")]
        );
    }

    #[test]
    fn keeps_the_last_4000_characters_of_the_output() {
        let long = format!("{}END", "x".repeat(5000));
        let runs = extract(&[
            call("a", "npm test", "Bash"),
            result("a", &json!(long), false),
        ]);
        assert_eq!(runs[0].output.chars().count(), MAX_TEST_OUTPUT);
        assert!(runs[0].output.ends_with("END"));
    }

    #[test]
    fn reads_results_given_as_text_blocks() {
        let content = json!([{ "type": "text", "text": "one" }, { "type": "text", "text": "two" }]);
        let runs = extract(&[call("a", "npm test", "Bash"), result("a", &content, false)]);
        assert_eq!(runs[0].output, "one\ntwo");
    }

    #[test]
    fn keeps_the_newest_runs_when_there_are_more_than_the_limit() {
        let mut lines = Vec::new();
        for number in 0..=REVIEW_TESTS_MAX {
            lines.push(call(
                &format!("t{number}"),
                &format!("cargo test {number}"),
                "Bash",
            ));
        }
        let runs = extract(&lines);
        assert_eq!(runs.len(), REVIEW_TESTS_MAX);
        assert_eq!(runs[0].command, "cargo test 1");
        assert_eq!(
            runs[REVIEW_TESTS_MAX - 1].command,
            format!("cargo test {REVIEW_TESTS_MAX}")
        );
    }
}
