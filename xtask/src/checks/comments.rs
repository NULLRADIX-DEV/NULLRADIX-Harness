use std::path::Path;

use super::{Syntax, Violation};

const MAX_COMMENT_LINE: usize = 100;
const TOO_LONG: &str = "comment line longer than 100 characters";
const MULTI_LINE: &str = "comment spans more than one line";
const MULTI_LINE_BLOCK: &str = "block comment spans more than one line";
const NON_ASCII: &str = "non-ASCII character";
const CONTROL: &str = "control character other than tab";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Code,
    Comment,
    BlockOpen,
}

fn classify(text: &str, syntax: Syntax, first: bool) -> Line {
    let trimmed = text.trim_start();
    if syntax == Syntax::Hash {
        let shebang = first && trimmed.starts_with("#!");
        return if trimmed.starts_with('#') && !shebang {
            Line::Comment
        } else {
            Line::Code
        };
    }
    if let Some(rest) = trimmed.strip_prefix("/*") {
        return if rest.contains("*/") {
            Line::Comment
        } else {
            Line::BlockOpen
        };
    }
    if syntax == Syntax::Slash && trimmed.starts_with("//") {
        Line::Comment
    } else {
        Line::Code
    }
}

/// Enforces ASCII-only files and comments of at most one short line.
pub(super) fn check(path: &Path, content: &str, syntax: Syntax) -> Vec<Violation> {
    let violation = |line, message: &str| Violation {
        path: path.to_path_buf(),
        line,
        message: message.to_owned(),
    };
    let mut violations = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    let mut in_block = false;

    let close_run = |run: &mut Option<(usize, usize)>, violations: &mut Vec<Violation>| {
        if let Some((start, length)) = run.take()
            && length > 1
        {
            violations.push(violation(start, MULTI_LINE));
        }
    };

    for (index, text) in content.lines().enumerate() {
        let number = index + 1;
        if !text.is_ascii() {
            violations.push(violation(number, NON_ASCII));
        } else if text
            .chars()
            .any(|letter| letter.is_ascii_control() && letter != '\t')
        {
            violations.push(violation(number, CONTROL));
        }
        if in_block {
            in_block = !text.contains("*/");
            continue;
        }
        match classify(text, syntax, index == 0) {
            Line::Comment => {
                if text.trim_end().chars().count() > MAX_COMMENT_LINE {
                    violations.push(violation(number, TOO_LONG));
                }
                run = Some(run.map_or((number, 1), |(start, length)| (start, length + 1)));
            }
            Line::BlockOpen => {
                close_run(&mut run, &mut violations);
                violations.push(violation(number, MULTI_LINE_BLOCK));
                in_block = true;
            }
            Line::Code => close_run(&mut run, &mut violations),
        }
    }
    close_run(&mut run, &mut violations);
    violations.sort_by_key(|v| v.line);
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(content: &str, syntax: Syntax) -> Vec<(usize, String)> {
        check(Path::new("file"), content, syntax)
            .into_iter()
            .map(|v| (v.line, v.message))
            .collect()
    }

    #[test]
    fn accepts_single_line_comments() {
        let content = "// one\nlet a = 1;\n/// two\nfn b() {}\n/* three */\n";
        assert_eq!(lines(content, Syntax::Slash), vec![]);
    }

    #[test]
    fn rejects_consecutive_line_comments() {
        let content = "let a = 1;\n// one\n// two\n//! three\nlet b = 2;\n";
        assert_eq!(
            lines(content, Syntax::Slash),
            vec![(2, MULTI_LINE.to_owned())]
        );
    }

    #[test]
    fn reports_a_trailing_comment_run() {
        assert_eq!(
            lines("/// a\n/// b", Syntax::Slash),
            vec![(1, MULTI_LINE.to_owned())]
        );
    }

    #[test]
    fn ignores_trailing_comments_after_code() {
        let content = "let a = 1; // one\nlet b = 2; // two\n";
        assert_eq!(lines(content, Syntax::Slash), vec![]);
    }

    #[test]
    fn rejects_multi_line_block_comments_once() {
        let content = "/**\n * docs\n */\nfn a() {}\n";
        assert_eq!(
            lines(content, Syntax::Slash),
            vec![(1, MULTI_LINE_BLOCK.to_owned())]
        );
    }

    #[test]
    fn checks_css_blocks_only() {
        assert_eq!(lines("/* ok */\na { color: red; }\n", Syntax::Css), vec![]);
        assert_eq!(
            lines("/* a\n b */\n", Syntax::Css),
            vec![(1, MULTI_LINE_BLOCK.to_owned())]
        );
        assert_eq!(lines("// not a css comment\n", Syntax::Css), vec![]);
    }

    #[test]
    fn handles_hash_comments_and_shebang() {
        assert_eq!(lines("#!/bin/sh\n# one\necho hi\n", Syntax::Hash), vec![]);
        assert_eq!(
            lines("# one\n  # two\nkey = 1\n", Syntax::Hash),
            vec![(1, MULTI_LINE.to_owned())]
        );
    }

    #[test]
    fn rejects_long_comment_lines() {
        let content = format!("// {}\n", "x".repeat(MAX_COMMENT_LINE));
        assert_eq!(
            lines(&content, Syntax::Slash),
            vec![(1, TOO_LONG.to_owned())]
        );
    }

    #[test]
    fn rejects_control_characters_but_tabs() {
        let content = format!("a {{ content: '{}b7'; }}\n\tb {{}}\n", char::from(0));
        assert_eq!(lines(&content, Syntax::Css), vec![(1, CONTROL.to_owned())]);
    }

    #[test]
    fn rejects_non_ascii_anywhere() {
        let content = "let a = \"\u{e4}\";\n";
        assert_eq!(
            lines(content, Syntax::Slash),
            vec![(1, NON_ASCII.to_owned())]
        );
    }
}
