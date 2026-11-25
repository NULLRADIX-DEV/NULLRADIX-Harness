//! Figures of a session from its transcript, fed line by line so a growing file reads only anew.

use std::collections::HashMap;

use nullradix_app::session_detail::{
    ContextUsage, TodoItem, TodoStatus, TokenTotals, TranscriptFacts,
};
use nullradix_domain::SessionClient;
use serde_json::Value;

use super::jsonl::{flag, number, text};
use super::summary::{id_text, task_id};

/// Context limit of the models.
const CONTEXT_LIMIT: i64 = 200_000;
/// Context limit of the models with a large context (`[1m]`).
const LARGE_CONTEXT_LIMIT: i64 = 1_000_000;

fn todo_status(name: &str) -> Option<TodoStatus> {
    match name {
        "pending" => Some(TodoStatus::Pending),
        "in_progress" => Some(TodoStatus::InProgress),
        "completed" => Some(TodoStatus::Completed),
        _ => None,
    }
}

/// Collects the figures line by line.
#[derive(Debug, Clone, Default)]
pub(crate) struct FactsCollector {
    client: Option<SessionClient>,
    branch: Option<String>,
    version: Option<String>,
    title: Option<String>,
    model: Option<String>,
    /// Tokens per answer (`message.id`); lines of one answer repeat them.
    usage: HashMap<String, TokenTotals>,
    usage_sum: TokenTotals,
    /// Last state from `cost-state`.
    cost_tokens: Option<TokenTotals>,
    cost_usd: Option<f64>,
    duration_ms: Option<i64>,
    context: Option<ContextUsage>,
    /// Tasks with their number; only `TaskCreate` gives one.
    todos: Vec<(Option<String>, TodoItem)>,
    /// Todo calls without result: id to name and input.
    pending: HashMap<String, (String, Value)>,
}

impl FactsCollector {
    pub(crate) fn feed(&mut self, value: &Value) {
        let side = flag(value, "isSidechain");
        if !side {
            if self.client.is_none()
                && let Some(entrypoint) = text(value, "entrypoint")
            {
                self.client = Some(SessionClient::from_entrypoint(entrypoint));
            }
            if let Some(branch) = text(value, "gitBranch") {
                self.branch = Some(branch.to_owned());
            }
            if let Some(version) = text(value, "version") {
                self.version = Some(version.to_owned());
            }
        }
        match text(value, "type") {
            Some("custom-title") => {
                if let Some(title) = text(value, "customTitle") {
                    self.title = Some(title.to_owned());
                }
            }
            Some("cost-state") => self.costs(value),
            Some("assistant") => self.assistant(value, side),
            Some("user") if !side => self.results(value),
            _ => {}
        }
    }

    pub(crate) fn facts(&self) -> TranscriptFacts {
        TranscriptFacts {
            client: self.client,
            branch: self.branch.clone(),
            version: self.version.clone(),
            title: self.title.clone(),
            model: self.model.clone(),
            tokens: self.cost_tokens.unwrap_or(self.usage_sum),
            cost_usd: self.cost_usd,
            duration_ms: self.duration_ms,
            context: self.context,
            todos: self.todos.iter().map(|(_, todo)| todo.clone()).collect(),
        }
    }

    /// Cost, duration and tokens of all models; the last state counts.
    fn costs(&mut self, value: &Value) {
        if let Some(cost) = value.get("totalCostUSD").and_then(Value::as_f64) {
            self.cost_usd = Some(cost);
        }
        if let Some(duration) = number(value, "totalDuration") {
            self.duration_ms = Some(duration);
        }
        if let Some(models) = value.get("modelUsage").and_then(Value::as_object) {
            let mut sum = TokenTotals::default();
            for model in models.values() {
                sum.input += number(model, "inputTokens").unwrap_or(0);
                sum.output += number(model, "outputTokens").unwrap_or(0);
                sum.cache_read += number(model, "cacheReadInputTokens").unwrap_or(0);
                sum.cache_write += number(model, "cacheCreationInputTokens").unwrap_or(0);
            }
            self.cost_tokens = Some(sum);
        }
    }

    /// Tokens of the answer, context use of the main thread and open todo calls.
    fn assistant(&mut self, value: &Value, side: bool) {
        let message = value.get("message").unwrap_or(&Value::Null);
        if let Some(usage) = message.get("usage") {
            let tokens = TokenTotals {
                input: number(usage, "input_tokens").unwrap_or(0),
                output: number(usage, "output_tokens").unwrap_or(0),
                cache_read: number(usage, "cache_read_input_tokens").unwrap_or(0),
                cache_write: number(usage, "cache_creation_input_tokens").unwrap_or(0),
            };
            let key = text(message, "id")
                .or_else(|| text(value, "uuid"))
                .unwrap_or("")
                .to_owned();
            let old = self.usage.insert(key, tokens).unwrap_or_default();
            self.usage_sum.input += tokens.input - old.input;
            self.usage_sum.output += tokens.output - old.output;
            self.usage_sum.cache_read += tokens.cache_read - old.cache_read;
            self.usage_sum.cache_write += tokens.cache_write - old.cache_write;
            let used = tokens.input + tokens.cache_read + tokens.cache_write;
            if !side && used > 0 {
                let large = text(message, "model").is_some_and(|model| model.contains("[1m]"))
                    || used > CONTEXT_LIMIT;
                self.context = Some(ContextUsage {
                    used_tokens: used,
                    limit_tokens: Some(if large {
                        LARGE_CONTEXT_LIMIT
                    } else {
                        CONTEXT_LIMIT
                    }),
                });
            }
        }
        if side {
            return;
        }
        // placeholders such as `<synthetic>` of aborted answers are no model
        if let Some(model) = text(message, "model").filter(|model| !model.starts_with('<')) {
            self.model = Some(model.to_owned());
        }
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            let name = text(block, "name").unwrap_or("");
            if text(block, "type") == Some("tool_use")
                && matches!(name, "TodoWrite" | "TaskCreate" | "TaskUpdate")
                && let Some(id) = text(block, "id")
            {
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                self.pending.insert(id.to_owned(), (name.to_owned(), input));
            }
        }
    }

    /// Applies successful results of open todo calls.
    fn results(&mut self, value: &Value) {
        let Some(parts) = value.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        let details = value.get("toolUseResult").unwrap_or(&Value::Null);
        for part in parts {
            let Some(id) = text(part, "tool_use_id") else {
                continue;
            };
            let Some((name, input)) = self.pending.remove(id) else {
                continue;
            };
            if flag(part, "is_error") {
                continue;
            }
            match name.as_str() {
                "TodoWrite" => self.write_todos(&input),
                "TaskCreate" => self.create_task(&input, details),
                _ => self.update_task(&input),
            }
        }
    }

    /// `TodoWrite` replaces the whole list.
    fn write_todos(&mut self, input: &Value) {
        let Some(todos) = input.get("todos").and_then(Value::as_array) else {
            return;
        };
        self.todos = todos
            .iter()
            .filter_map(|todo| {
                Some((
                    None,
                    TodoItem {
                        content: text(todo, "content")?.to_owned(),
                        status: text(todo, "status")
                            .and_then(todo_status)
                            .unwrap_or(TodoStatus::Pending),
                    },
                ))
            })
            .collect();
    }

    /// `TaskCreate` appends a pending task; its number is in the result.
    fn create_task(&mut self, input: &Value, details: &Value) {
        let subject = text(input, "subject")
            .or_else(|| details.pointer("/task/subject").and_then(Value::as_str));
        let Some(subject) = subject else {
            return;
        };
        let id = details.pointer("/task/id").and_then(id_text);
        self.todos.push((
            id,
            TodoItem {
                content: subject.to_owned(),
                status: TodoStatus::Pending,
            },
        ));
    }

    /// `TaskUpdate` changes state or subject; `deleted` removes the task.
    fn update_task(&mut self, input: &Value) {
        let Some(id) = task_id(input) else {
            return;
        };
        let Some(index) = self
            .todos
            .iter()
            .position(|(task, _)| task.as_deref() == Some(id.as_str()))
        else {
            return;
        };
        if text(input, "status") == Some("deleted") {
            self.todos.remove(index);
            return;
        }
        let todo = &mut self.todos[index].1;
        if let Some(status) = text(input, "status").and_then(todo_status) {
            todo.status = status;
        }
        if let Some(subject) = text(input, "subject") {
            subject.clone_into(&mut todo.content);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::fixtures;
    use super::*;

    fn collect(content: &str) -> TranscriptFacts {
        let mut collector = FactsCollector::default();
        for line in content.lines() {
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                collector.feed(&value);
            }
        }
        collector.facts()
    }

    #[test]
    fn reads_client_branch_version_and_title() {
        let facts = collect(&fixtures::transcript());
        assert_eq!(facts.client, Some(SessionClient::ClaudeDesktop));
        assert_eq!(facts.branch.as_deref(), Some("feature/demo"));
        assert_eq!(facts.version.as_deref(), Some("2.1.0"));
        assert_eq!(facts.title.as_deref(), Some("Demo title"));
        assert_eq!(facts.model.as_deref(), Some("claude-demo"));
    }

    #[test]
    fn takes_tokens_cost_and_duration_from_the_last_cost_state() {
        let facts = collect(&fixtures::transcript());
        assert_eq!(
            facts.tokens,
            TokenTotals {
                input: 120,
                output: 203,
                cache_read: 2300,
                cache_write: 260
            }
        );
        assert_eq!(facts.cost_usd, Some(0.42));
        assert_eq!(facts.duration_ms, Some(65_000));
    }

    #[test]
    fn without_cost_state_adds_each_answer_once() {
        let facts = collect(&fixtures::transcript_without_costs());
        assert_eq!(
            facts.tokens,
            TokenTotals {
                input: 125,
                output: 203,
                cache_read: 2360,
                cache_write: 260
            }
        );
        assert_eq!(facts.cost_usd, None);
        assert_eq!(facts.duration_ms, None);
    }

    #[test]
    fn context_of_the_last_main_answer() {
        let facts = collect(&fixtures::transcript());
        assert_eq!(
            facts.context,
            Some(ContextUsage {
                used_tokens: 1205,
                limit_tokens: Some(200_000)
            })
        );
        let large = fixtures::line(&fixtures::assistant(
            "a-large",
            "m-large",
            json!({ "type": "text", "text": "x" }),
            [1, 1, 250_000, 0],
        ));
        let facts = collect(&format!("{}{large}", fixtures::transcript()));
        assert_eq!(facts.context.unwrap().limit_tokens, Some(1_000_000));
    }

    #[test]
    fn builds_the_todo_list_from_todowrite_and_tasks() {
        let facts = collect(&fixtures::transcript());
        assert_eq!(
            facts.todos,
            vec![
                TodoItem {
                    content: "Read the plan".into(),
                    status: TodoStatus::Completed
                },
                TodoItem {
                    content: "Build it".into(),
                    status: TodoStatus::InProgress
                },
                TodoItem {
                    content: "Write tests".into(),
                    status: TodoStatus::Completed
                },
            ]
        );
    }

    #[test]
    fn deleted_and_failed_tasks_do_not_count() {
        let lines = [
            fixtures::tool_use(
                "a1",
                "m1",
                "t1",
                "TaskCreate",
                json!({ "subject": "Gone" }),
                [0; 4],
            ),
            fixtures::tool_result(
                "u1",
                "t1",
                json!("ok"),
                false,
                json!({ "task": { "id": 1 } }),
            ),
            fixtures::tool_use(
                "a2",
                "m2",
                "t2",
                "TaskCreate",
                json!({ "subject": "Failed" }),
                [0; 4],
            ),
            fixtures::tool_result("u2", "t2", json!("no"), true, json!("Error")),
            fixtures::tool_use(
                "a3",
                "m3",
                "t3",
                "TaskUpdate",
                json!({ "taskId": "1", "status": "deleted" }),
                [0; 4],
            ),
            fixtures::tool_result("u3", "t3", json!("ok"), false, json!({})),
        ];
        let content: String = lines.iter().map(fixtures::line).collect();
        assert_eq!(collect(&content).todos, Vec::new());
    }
}
