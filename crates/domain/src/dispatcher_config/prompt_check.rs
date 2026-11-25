use super::prompt_defaults::placeholders;
use super::prompt_template::placeholder_names;
use super::values::PromptKind;

/// A part every template of a kind must keep; the dispatcher and the agent rely on it.
#[derive(Debug, Clone, Copy)]
pub struct RequiredPart {
    pub id: &'static str,
    pub label: &'static str,
    holds: fn(&str) -> bool,
}

/// Something the checks need: the template as words, split at whitespace.
fn words(template: &str) -> Vec<&str> {
    template.split_whitespace().collect()
}

fn is_flag(word: &str) -> bool {
    word.len() > 1 && word.starts_with('-')
}

/// The longest template an own prompt may be, in bytes.
const MAX_TEMPLATE_BYTES: usize = 64 * 1024;

/// What is wrong with the size of a template; none when it may be checked further.
#[must_use]
pub fn size_problem(template: &str) -> Option<String> {
    (template.len() > MAX_TEMPLATE_BYTES)
        .then(|| format!("is longer than {} KiB", MAX_TEMPLATE_BYTES / 1024))
}

/// A command such as `nullradix ticket done {{ticketId}}`, flags allowed between words; one pass.
fn has_command(template: &str, wanted: &[&str]) -> bool {
    // reached[k]: some start has matched the first k words
    let last = wanted.len() - 1;
    let mut reached = vec![false; wanted.len()];
    for word in template.split_whitespace() {
        let mut next = vec![false; wanted.len()];
        for k in 1..=last {
            if !reached[k] {
                continue;
            }
            let matches = if k == last {
                word.starts_with(wanted[k])
            } else {
                word == wanted[k]
            };
            if matches {
                if k == last {
                    return true;
                }
                next[k + 1] = true;
            }
            next[k] |= is_flag(word);
        }
        if word.ends_with(wanted[0]) {
            if last == 0 {
                return true;
            }
            next[1] = true;
        }
        reached = next;
    }
    false
}

fn has_text(template: &str, text: &str) -> bool {
    template.contains(text)
}

fn has_outcomes(template: &str) -> bool {
    has_text(template, "{{outcomes}}")
}

fn has_question_add(template: &str) -> bool {
    let all = words(template);
    // The earliest start leaves the most words to look at for the ticket.
    let Some(at) = all.iter().enumerate().position(|(at, word)| {
        word.ends_with("nullradix") && all[at + 1..].starts_with(&["question", "add"])
    }) else {
        return false;
    };
    all[at + 3..]
        .windows(2)
        .any(|pair| pair[0] == "--ticket" && pair[1].starts_with("{{ticketId}}"))
}

macro_rules! placeholder_part {
    ($name:literal) => {
        RequiredPart {
            id: $name,
            label: concat!("Placeholder {{", $name, "}}"),
            holds: |template| has_text(template, concat!("{{", $name, "}}")),
        }
    };
}

const WORK: [RequiredPart; 6] = [
    placeholder_part!("ticketId"),
    placeholder_part!("agent"),
    RequiredPart {
        id: "done",
        label: "Outcome done: nullradix ticket done {{ticketId}}",
        holds: |template| {
            has_command(template, &["nullradix", "ticket", "done", "{{ticketId}}"])
                || has_outcomes(template)
        },
    },
    RequiredPart {
        id: "blocked",
        label: "Outcome blocked: nullradix ticket set {{ticketId}} --status blocked",
        holds: |template| {
            has_command(
                template,
                &[
                    "nullradix",
                    "ticket",
                    "set",
                    "{{ticketId}}",
                    "--status",
                    "blocked",
                ],
            ) || has_outcomes(template)
        },
    },
    RequiredPart {
        id: "question",
        label: "Outcome question: nullradix question add ... --ticket {{ticketId}}",
        holds: |template| has_question_add(template) || has_outcomes(template),
    },
    RequiredPart {
        id: "author-agent",
        label: "Attribution --author {{agent}}",
        holds: |template| {
            has_command(template, &["--author", "{{agent}}"]) || has_outcomes(template)
        },
    },
];

const PLAN: [RequiredPart; 4] = [
    placeholder_part!("ticketId"),
    placeholder_part!("agent"),
    RequiredPart {
        id: "plan-import",
        label: "Saving: nullradix plan import {{ticketId}}",
        holds: |template| has_command(template, &["nullradix", "plan", "import", "{{ticketId}}"]),
    },
    RequiredPart {
        id: "plan-file",
        label: "File plan.json",
        holds: |template| has_text(template, "plan.json"),
    },
];

const RESEARCH: [RequiredPart; 4] = [
    placeholder_part!("questionId"),
    placeholder_part!("agent"),
    RequiredPart {
        id: "research-submit",
        label: "Saving: nullradix research submit {{questionId}}",
        holds: |template| {
            has_command(
                template,
                &["nullradix", "research", "submit", "{{questionId}}"],
            )
        },
    },
    RequiredPart {
        id: "research-file",
        label: "File research.json",
        holds: |template| has_text(template, "research.json"),
    },
];

const MERGE: [RequiredPart; 5] = [
    placeholder_part!("ticketId"),
    placeholder_part!("agent"),
    placeholder_part!("branch"),
    placeholder_part!("target"),
    RequiredPart {
        id: "merge-abort",
        label: "Retreat: git merge --abort",
        holds: |template| has_command(template, &["git", "merge", "--abort"]),
    },
];

/// Every part a template of this kind must keep.
#[must_use]
pub fn required_parts(kind: PromptKind) -> &'static [RequiredPart] {
    match kind {
        PromptKind::Work => &WORK,
        PromptKind::Plan => &PLAN,
        PromptKind::Research => &RESEARCH,
        PromptKind::Merge => &MERGE,
    }
}

/// The required parts the template lacks.
#[must_use]
pub fn missing_parts(kind: PromptKind, template: &str) -> Vec<&'static RequiredPart> {
    required_parts(kind)
        .iter()
        .filter(|part| !(part.holds)(template))
        .collect()
}

/// Placeholders the template uses that the kind does not know.
#[must_use]
pub fn unknown_placeholders(kind: PromptKind, template: &str) -> Vec<String> {
    let known = placeholders(kind);
    placeholder_names(template)
        .into_iter()
        .filter(|name| !known.iter().any(|placeholder| placeholder.name == name))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use strum::IntoEnumIterator;

    use super::*;

    fn sample(kind: PromptKind) -> String {
        match kind {
            PromptKind::Work => [
                "You are {{agent}}. Implement ticket {{ticketId}}.",
                "done: nullradix ticket done {{ticketId}} --acceptance \"<proof>\";",
                "blocked: nullradix ticket set {{ticketId}} --status blocked --blocked-reason \"<why>\";",
                "question: nullradix question add \"<q>\" --description \"<d>\" --ticket {{ticketId}} --author {{agent}}",
            ]
            .join("\n"),
            PromptKind::Plan => [
                "You are {{agent}}. Plan {{ticketId}}.",
                "Write plan.json, then: nullradix plan import {{ticketId}} --file plan.json",
            ]
            .join("\n"),
            PromptKind::Research => [
                "You are {{agent}}. Research {{questionId}}.",
                "Write research.json, then: nullradix research submit {{questionId}} --file research.json",
            ]
            .join("\n"),
            PromptKind::Merge => [
                "You are {{agent}}. Merge {{branch}} of {{ticketId}} into {{target}}.",
                "If it fails: git merge --abort",
            ]
            .join("\n"),
        }
    }

    fn ids(kind: PromptKind, template: &str) -> Vec<&'static str> {
        missing_parts(kind, template)
            .iter()
            .map(|part| part.id)
            .collect()
    }

    #[test]
    fn a_complete_sample_misses_nothing() {
        for kind in PromptKind::iter() {
            assert_eq!(ids(kind, &sample(kind)), Vec::<&str>::new(), "{kind:?}");
        }
    }

    #[test]
    fn an_empty_template_misses_everything() {
        for kind in PromptKind::iter() {
            assert_eq!(missing_parts(kind, "").len(), required_parts(kind).len());
        }
    }

    #[test]
    fn ids_are_distinct_and_labels_say_something() {
        for kind in PromptKind::iter() {
            let parts = required_parts(kind);
            let distinct: HashSet<&str> = parts.iter().map(|part| part.id).collect();
            assert_eq!(distinct.len(), parts.len(), "{kind:?}");
            assert!(parts.iter().all(|part| !part.label.trim().is_empty()));
        }
    }

    #[test]
    fn each_part_is_missed_when_its_text_is_removed() {
        let cases = [
            (
                PromptKind::Work,
                "done",
                "nullradix ticket done {{ticketId}}",
            ),
            (
                PromptKind::Work,
                "blocked",
                "nullradix ticket set {{ticketId}} --status blocked",
            ),
            (PromptKind::Work, "author-agent", "--author {{agent}}"),
            (
                PromptKind::Plan,
                "plan-import",
                "nullradix plan import {{ticketId}}",
            ),
            (PromptKind::Plan, "plan-file", "plan.json"),
            (
                PromptKind::Research,
                "research-submit",
                "nullradix research submit {{questionId}}",
            ),
            (PromptKind::Merge, "merge-abort", "git merge --abort"),
            (PromptKind::Merge, "target", "{{target}}"),
        ];
        for (kind, id, text) in cases {
            let broken = sample(kind).replace(text, "X");
            assert!(ids(kind, &broken).contains(&id), "{kind:?} {id}");
        }
    }

    #[test]
    fn the_question_needs_the_ticket_behind_the_flag() {
        let broken = sample(PromptKind::Work).replace("--ticket {{ticketId}}", "--ticket ARC-1");
        assert!(ids(PromptKind::Work, &broken).contains(&"question"));
    }

    #[test]
    fn flags_and_spacing_between_words_are_tolerated() {
        let loose = "{{agent}} nullradix   ticket --json   done --force {{ticketId}}\n\
            nullradix ticket set {{ticketId}} --status   blocked\n\
            nullradix question add x --ticket {{ticketId}} --author {{agent}}";
        assert_eq!(ids(PromptKind::Work, loose), Vec::<&str>::new());
    }

    #[test]
    fn the_outcomes_placeholder_stands_in_for_the_commands() {
        let template = "{{agent}} {{ticketId}}\n{{outcomes}}";
        assert_eq!(ids(PromptKind::Work, template), Vec::<&str>::new());
    }

    #[test]
    fn commands_are_found_with_flags_between_the_words_in_one_pass() {
        let wanted = [
            "nullradix",
            "ticket",
            "set",
            "{{ticketId}}",
            "--status",
            "blocked",
        ];
        assert!(has_command(
            "x /bin/nullradix -v ticket --a set --b {{ticketId}} --status --status blocked",
            &wanted
        ));
        assert!(!has_command(
            "nullradix ticket set {{ticketId}} --status",
            &wanted
        ));
        assert!(!has_command(
            "nullradix ticket x set {{ticketId}} --status blocked",
            &wanted
        ));
        assert!(has_command(
            "nullradix nullradix ticket set {{ticketId}} --status blocked",
            &wanted
        ));
        let flags = "--author ".repeat(200_000);
        assert!(!has_command(&flags, &["--author", "{{agent}}"]));
        assert!(has_command(
            &format!("{flags}{{{{agent}}}}"),
            &["--author", "{{agent}}"]
        ));
        assert!(!has_question_add(
            &"nullradix question add ".repeat(100_000)
        ));
    }

    #[test]
    fn unknown_placeholders_are_found_per_kind() {
        assert_eq!(
            unknown_placeholders(PromptKind::Research, "{{agent}} {{ticketId}} {{nope}}"),
            ["ticketId", "nope"]
        );
        assert_eq!(
            unknown_placeholders(PromptKind::Work, "{{answers}} {{feedback}}"),
            Vec::<String>::new()
        );
    }
}
