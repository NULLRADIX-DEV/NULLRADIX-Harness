//! Work steps of a ticket, numbered from 1, with the history lines legacy wrote for them.

use crate::{TextError, required_text, utf16_prefix};

pub const STEP_MAX: usize = 1000;
/// How much of a step's text the history note of a check keeps, as in legacy.
const NOTE_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub text: String,
    pub done: bool,
    /// Guidance for the step; blank guidance is none.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StepError {
    #[error("the step text {0}")]
    Text(TextError),
    #[error("there is no step {number} ({})", range(*.count))]
    NotFound { number: i64, count: usize },
    #[error("position {at} does not exist (1 to {})", .count + 1)]
    NoPosition { at: i64, count: usize },
    #[error("the change names neither text nor detail")]
    NothingChanged,
    #[error("step {number} changed in the meantime: {}", .now.as_deref().unwrap_or("it is gone"))]
    Changed { number: i64, now: Option<String> },
}

fn range(count: usize) -> String {
    if count == 0 {
        String::from("there are none")
    } else {
        format!("1 to {count}")
    }
}

/// A list of steps after an edit, with the history line that records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepEdit {
    pub steps: Vec<Step>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub note: String,
}

fn text(value: &str) -> Result<String, StepError> {
    required_text(value, STEP_MAX)
        .map(str::to_owned)
        .map_err(StepError::Text)
}

fn detail(value: Option<&str>) -> Option<String> {
    value
        .filter(|detail| !detail.trim().is_empty())
        .map(str::to_owned)
}

fn index(steps: &[Step], number: i64) -> Result<usize, StepError> {
    usize::try_from(number)
        .ok()
        .filter(|number| (1..=steps.len()).contains(number))
        .map(|number| number - 1)
        .ok_or(StepError::NotFound {
            number,
            count: steps.len(),
        })
}

/// New steps from their texts, none done.
pub fn new_steps<S: AsRef<str>>(texts: &[S]) -> Result<Vec<Step>, StepError> {
    texts
        .iter()
        .map(|value| {
            Ok(Step {
                text: text(value.as_ref())?,
                done: false,
                detail: None,
            })
        })
        .collect()
}

/// Checks that step `number` still reads `expected`; without an expectation nothing is checked.
pub fn expect(steps: &[Step], number: i64, expected: Option<&str>) -> Result<(), StepError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let current = index(steps, number).ok().map(|at| &steps[at].text);
    match current {
        Some(text) if text.trim() == expected.trim() => Ok(()),
        _ => Err(StepError::Changed {
            number,
            now: current.cloned(),
        }),
    }
}

/// Appends a step, or inserts it at position `at` from 1 to the count plus one.
pub fn add(
    steps: &[Step],
    value: &str,
    guidance: Option<&str>,
    at: Option<i64>,
) -> Result<StepEdit, StepError> {
    let count = steps.len();
    let at = at.unwrap_or_else(|| i64::try_from(count + 1).unwrap_or(i64::MAX));
    let slot = usize::try_from(at)
        .ok()
        .filter(|at| (1..=count + 1).contains(at))
        .ok_or(StepError::NoPosition { at, count })?;
    let step = Step {
        text: text(value)?,
        done: false,
        detail: detail(guidance),
    };
    let mut next = steps.to_vec();
    next.insert(slot - 1, step.clone());
    Ok(StepEdit {
        steps: next,
        from: None,
        to: Some(format!("{at}: {}", step.text)),
        note: String::from("step added"),
    })
}

/// Changes text or guidance of a step; `Some(None)` removes the guidance, the check stays.
pub fn edit(
    steps: &[Step],
    number: i64,
    value: Option<&str>,
    guidance: Option<Option<&str>>,
) -> Result<StepEdit, StepError> {
    if value.is_none() && guidance.is_none() {
        return Err(StepError::NothingChanged);
    }
    let at = index(steps, number)?;
    let old = &steps[at];
    let step = Step {
        text: match value {
            Some(value) => text(value)?,
            None => old.text.clone(),
        },
        done: old.done,
        detail: detail(guidance.unwrap_or(old.detail.as_deref())),
    };
    let note = if step.text == old.text {
        "detail changed"
    } else {
        "step edited"
    };
    let mut next = steps.to_vec();
    next[at] = step.clone();
    Ok(StepEdit {
        steps: next,
        from: Some(format!("{number}: {}", old.text)),
        to: Some(format!("{number}: {}", step.text)),
        note: String::from(note),
    })
}

/// Removes a step; later ones move up.
pub fn remove(steps: &[Step], number: i64) -> Result<StepEdit, StepError> {
    let at = index(steps, number)?;
    let mut next = steps.to_vec();
    let removed = next.remove(at);
    Ok(StepEdit {
        steps: next,
        from: Some(format!("{number}: {}", removed.text)),
        to: None,
        note: String::from("step removed"),
    })
}

/// Moves step `from` to number `to`; both must exist.
pub fn move_step(steps: &[Step], from: i64, to: i64) -> Result<StepEdit, StepError> {
    let source = index(steps, from)?;
    let target = index(steps, to)?;
    let mut next = steps.to_vec();
    let step = next.remove(source);
    next.insert(target, step.clone());
    Ok(StepEdit {
        steps: next,
        from: Some(format!("{from}: {}", step.text)),
        to: Some(format!("{to}: {}", step.text)),
        note: String::from("step moved"),
    })
}

/// Checks or unchecks a step; the history note keeps the start of its text.
pub fn check(steps: &[Step], number: i64, done: bool) -> Result<StepEdit, StepError> {
    let at = index(steps, number)?;
    let flag = |done: bool| if done { "done" } else { "open" };
    let old = &steps[at];
    let mut next = steps.to_vec();
    next[at].done = done;
    Ok(StepEdit {
        from: Some(format!("{number}: {}", flag(old.done))),
        to: Some(format!("{number}: {}", flag(done))),
        note: utf16_prefix(&old.text, NOTE_MAX),
        steps: next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(texts: &[&str]) -> Vec<Step> {
        new_steps(texts).unwrap()
    }

    #[test]
    fn steps_are_added_at_the_end_or_a_free_position() {
        let list = steps(&["a", "b"]);
        let appended = add(&list, " c ", Some(" "), None).unwrap();
        assert_eq!(appended.steps[2].text, "c");
        assert_eq!(appended.steps[2].detail, None);
        assert_eq!(appended.to.as_deref(), Some("3: c"));
        let first = add(&list, "z", Some("how"), Some(1)).unwrap();
        assert_eq!(first.steps[0].detail.as_deref(), Some("how"));
        assert_eq!(
            add(&list, "x", None, Some(4)),
            Err(StepError::NoPosition { at: 4, count: 2 })
        );
        assert_eq!(
            add(&list, " ", None, None),
            Err(StepError::Text(TextError::Empty))
        );
        assert_eq!(
            add(&list, &"x".repeat(1001), None, None),
            Err(StepError::Text(TextError::TooLong(1000)))
        );
    }

    #[test]
    fn edits_keep_the_check_and_name_what_changed() {
        let mut list = steps(&["a", "b"]);
        list[1].done = true;
        let guidance = edit(&list, 2, None, Some(Some("how"))).unwrap();
        assert_eq!(
            (guidance.steps[1].done, guidance.note.as_str()),
            (true, "detail changed")
        );
        let renamed = edit(&guidance.steps, 2, Some("c"), Some(None)).unwrap();
        assert_eq!(renamed.steps[1].detail, None);
        assert_eq!(
            (renamed.from.as_deref(), renamed.to.as_deref()),
            (Some("2: b"), Some("2: c"))
        );
        assert_eq!(edit(&list, 2, None, None), Err(StepError::NothingChanged));
        assert_eq!(
            edit(&list, 3, Some("x"), None),
            Err(StepError::NotFound {
                number: 3,
                count: 2
            })
        );
    }

    #[test]
    fn removal_moves_and_checks_keep_numbers_dense() {
        let list = steps(&["a", "b", "c"]);
        let removed = remove(&list, 1).unwrap();
        assert_eq!(removed.steps, steps(&["b", "c"]));
        let moved = move_step(&list, 3, 1).unwrap();
        assert_eq!(moved.steps, steps(&["c", "a", "b"]));
        assert_eq!(
            move_step(&list, 1, 4),
            Err(StepError::NotFound {
                number: 4,
                count: 3
            })
        );
        let checked = check(&list, 2, true).unwrap();
        assert!(checked.steps[1].done);
        assert_eq!(
            (checked.from.as_deref(), checked.to.as_deref()),
            (Some("2: open"), Some("2: done"))
        );
        assert_eq!(checked.note, "b");
        assert_eq!(
            remove(&[], 0).unwrap_err().to_string(),
            "there is no step 0 (there are none)"
        );
    }

    #[test]
    fn expectations_guard_against_changed_steps() {
        let list = steps(&["a", "b"]);
        assert_eq!(expect(&list, 2, Some(" b ")), Ok(()));
        assert_eq!(expect(&list, 2, None), Ok(()));
        assert_eq!(
            expect(&list, 1, Some("b")),
            Err(StepError::Changed {
                number: 1,
                now: Some(String::from("a"))
            })
        );
        assert_eq!(
            expect(&list, 5, Some("b")),
            Err(StepError::Changed {
                number: 5,
                now: None
            })
        );
    }
}
