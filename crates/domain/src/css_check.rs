//! Checks style text with a real CSS tokenizer, so it reads the text as a browser does.

use cssparser::{ParseError, Parser, SourcePosition, Token};

/// Deepest nesting of blocks and functions; deeper text is refused.
const MAX_DEPTH: usize = 32;

/// Functions that load a file by name or url besides `url()`.
const LOADING_FUNCTIONS: [&str; 5] = ["image-set", "image", "src", "element", "cross-fade"];

type Failure = ParseError<String>;

/// Refuses text that is malformed or loads anything but `data:` urls (and package paths if allowed).
pub(crate) fn check_css(css: &str, allow_relative: bool) -> Result<(), String> {
    if css.contains('\\') {
        return Err(String::from("backslash escapes are not allowed"));
    }
    // CSS also ends lines at CR and FF
    if css.contains(['\r', '\u{c}']) {
        return Err(String::from(
            "carriage returns and form feeds are not allowed",
        ));
    }
    let mut parser = Parser::new(css);
    walk(&mut parser, 0, allow_relative, Mode::Plain)
        .map(|_| ())
        .map_err(|error| match error.kind {
            cssparser::ParseErrorKind::Custom(message) => message,
            cssparser::ParseErrorKind::Basic(_) => String::from("the style text is not valid"),
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Plain,
    /// Inside `url(`: one quoted string and nothing else.
    UrlArgument,
}

fn refuse(message: impl Into<String>) -> Failure {
    ParseError::custom(message.into())
}

/// Whether a url value stays inside the package (or is a `data:` url).
fn url_allowed(value: &str, allow_relative: bool) -> Result<(), String> {
    // browsers drop tabs and line breaks inside urls and trim control characters and spaces
    let clean: String = value
        .chars()
        .filter(|letter| !letter.is_ascii_control() && *letter != ' ')
        .collect();
    let lower = clean.to_ascii_lowercase();
    if lower.starts_with("data:") {
        return Ok(());
    }
    if !allow_relative {
        return Err(format!("only data: urls are allowed: {value}"));
    }
    let first = lower.split('/').next().unwrap_or_default();
    if lower.starts_with('/') || first.contains(':') || lower.contains('\\') {
        return Err(format!(
            "external or absolute url() is not allowed: {value}"
        ));
    }
    if lower.contains("/*") || lower.contains("*/") {
        return Err(String::from("comment markers inside url() are not allowed"));
    }
    Ok(())
}

/// Reads every token up to the end of the current block; returns where that was.
fn walk(
    parser: &mut Parser<'_>,
    depth: usize,
    allow_relative: bool,
    mode: Mode,
) -> Result<SourcePosition, Failure> {
    let mut strings = 0_usize;
    loop {
        let start = parser.position();
        let Ok(token) = parser.next_including_whitespace_and_comments() else {
            if mode == Mode::UrlArgument && strings != 1 {
                return Err(refuse("url() takes one quoted string"));
            }
            return Ok(start);
        };
        let token = token.clone();
        let raw = parser.slice_from(start);
        if mode == Mode::UrlArgument
            && !matches!(
                token,
                Token::WhiteSpace(_) | Token::Comment(_) | Token::QuotedString(_)
            )
        {
            return Err(refuse("url() takes one quoted string"));
        }
        match &token {
            Token::BadUrl(_) | Token::BadString(_) => {
                return Err(refuse("an unterminated string or url is not allowed"));
            }
            Token::Comment(_) if raw.len() < 4 || !raw.ends_with("*/") => {
                return Err(refuse("an unterminated comment is not allowed"));
            }
            Token::QuotedString(value) => {
                let quote = raw.chars().next().unwrap_or('"');
                if raw.len() < 2 || !raw.ends_with(quote) {
                    return Err(refuse("an unterminated string is not allowed"));
                }
                if mode == Mode::UrlArgument {
                    strings += 1;
                    url_allowed(value, allow_relative).map_err(refuse)?;
                }
            }
            Token::UnquotedUrl(value) => {
                if !raw.ends_with(')') {
                    return Err(refuse("an unterminated url is not allowed"));
                }
                url_allowed(value, allow_relative).map_err(refuse)?;
            }
            Token::AtKeyword(name) if name.eq_ignore_ascii_case("import") => {
                return Err(refuse("@import is not allowed"));
            }
            Token::CloseParenthesis | Token::CloseSquareBracket | Token::CloseCurlyBracket => {
                return Err(refuse("an unbalanced bracket is not allowed"));
            }
            Token::Function(name) => {
                let lower = name.to_ascii_lowercase();
                if LOADING_FUNCTIONS
                    .iter()
                    .any(|banned| lower == *banned || lower.ends_with(&format!("-{banned}")))
                {
                    return Err(refuse(
                        "image-set(), image(), src(), element() and cross-fade() are not allowed",
                    ));
                }
                let inner = if lower == "url" {
                    Mode::UrlArgument
                } else {
                    Mode::Plain
                };
                enter(parser, depth, allow_relative, inner)?;
            }
            Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                enter(parser, depth, allow_relative, Mode::Plain)?;
            }
            _ => {}
        }
    }
}

/// Reads the block that was just opened and checks that it was closed.
fn enter(
    parser: &mut Parser<'_>,
    depth: usize,
    allow_relative: bool,
    mode: Mode,
) -> Result<(), Failure> {
    if depth >= MAX_DEPTH {
        return Err(refuse("blocks are nested too deeply"));
    }
    let end = parser.parse_nested_block(|inner| walk(inner, depth + 1, allow_relative, mode))?;
    // a closing bracket takes one more byte than the last token inside
    if parser.position() == end {
        return Err(refuse("an unclosed bracket is not allowed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(css: &str, allow_relative: bool) -> bool {
        check_css(css, allow_relative).is_err()
    }

    #[test]
    fn normal_css_passes() {
        for css in [
            "",
            "a { color: red }",
            "/* a \"quote' */ a { color: red } /* it's */",
            "@media (min-width: 600px) and (max-width: 900px) { .a > b ~ c + d { margin: 0 auto } }",
            "@supports (display: grid) { a { grid-template-columns: repeat(2, minmax(0, 1fr)) } }",
            ":root { --a: calc(100% - (2 * var(--b, 4px))); --c: rgb(0 0 0 / 50%) }",
            "a::before { content: \") } /*\" }",
            "a { content: \"a ) } ] ( { [ b\" }",
            "a { background: url(data:image/png;base64,AAAA) }",
            "a { background: url( \"data:image/svg+xml;utf8,<svg xmlns='x'/>\" ) }",
            "a { background: URL(DATA:image/gif;base64,R0lGODlhAQABAAAAACw=) }",
            "a[href^=\"http\"] { color: red } a:not(.b):nth-child(2n+1) { color: blue }",
            "@font-face { font-family: x; src: local(x) }",
            "a { width: calc(1px + calc(2px + calc(3px + 4px))) }",
            "@keyframes k { from { opacity: 0 } to { opacity: 1 } }",
        ] {
            assert_eq!(check_css(css, false), Ok(()), "{css}");
        }
    }

    #[test]
    fn a_theme_may_use_paths_inside_the_package() {
        assert!(check_css("a { background: url(assets/a.png) }", true).is_ok());
        assert!(check_css("a { background: url( 'assets/a.png' ) }", true).is_ok());
        assert!(check_css("@font-face { src: url(assets/a.woff2) }", true).is_ok());
        assert!(refused("a { background: url(assets/a.png) }", false));
        assert!(refused("a { background: url('assets/a.png') }", false));
    }

    #[test]
    fn urls_that_reach_out_are_refused() {
        for css in [
            "a { background: url(https://evil.test/x.png) }",
            "a { background: url('https://evil.test/x.png') }",
            "a { background: url(\"//evil.test/x.png\") }",
            "a { background: url(\t//evil.test/x.png) }",
            "a { background: url(\"\t//evil.test/x.png\") }",
            "a { background: url(\"ht\ttps://evil.test/x.png\") }",
            "a { background: url(/abs.png) }",
            "a { background: url(file:x) }",
            "a { background: url(javascript:alert(1)) }",
            "a { background: URL( \"https://evil.test\" ) }",
            "a { background: url(\"data:x\" \"https://evil.test/x.png\") }",
            "a { background: url(\"data:x\" format(\"y\")) }",
            "a { background: url(var(--x)) }",
        ] {
            assert!(refused(css, true), "{css}");
            assert!(refused(css, false), "{css}");
        }
    }

    #[test]
    fn other_loading_functions_and_imports_are_refused() {
        for css in [
            "@import 'x.css';",
            "@IMPORT url(a.css);",
            "a { background: image-set(\"https://evil/x.png\" 1x) }",
            "a { background: -webkit-image-set(\"https://evil/x.png\" 1x) }",
            "a { background: image(\"https://evil/x.png\") }",
            "@font-face { src: src(\"https://evil/f.woff2\") }",
            "a { background: element(#x) }",
            "a { background: cross-fade(url(data:x), \"y\") }",
        ] {
            assert!(refused(css, true), "{css}");
            assert!(refused(css, false), "{css}");
        }
    }

    #[test]
    fn browser_tokenizing_cannot_hide_a_url() {
        for css in [
            // an unterminated quoted url swallows the line, a browser goes on at the next
            "a{background:url(\"data:x\n);}\nb{background:url(https://evil.test/x.png)}",
            // a quote inside an unquoted url
            "a{background:url(data:x\np{content:\")\"}/*\"} y{background:url(https://evil.test/x.png)} */",
            "a{background:url(x/*)}b{background:url(https://evil/p)}c{background:url(*/)}",
            "a{content:\"x\r\"/* \"}b{background:url(https://evil)}c{d:*/}",
            "a{content:\"x\u{c}\"/* \"}b{background:url(https://evil)}c{d:*/}",
            "a{content:\"/*\"} b{background:url(https://evil/x)} c{content:\"*/\"}",
        ] {
            assert!(refused(css, true), "{css}");
            assert!(refused(css, false), "{css}");
        }
    }

    #[test]
    fn unterminated_or_unbalanced_text_is_refused() {
        for css in [
            "/* open",
            "/*/",
            "a { color: red } /* open",
            "a { content: \"open }",
            "a { content: 'open",
            "a { background: url(data:x",
            "a { background: url(\"data:x\"",
            "a { color: red",
            "@media print {",
            "a { width: calc(1px + (2px }",
            "a { width: f(url(data:x) }",
            "a { width: f(a ]",
            "a { color: red } }",
            "a ) { color: red }",
            "a ] { color: red }",
            "a { content: \"a\nb\" }",
        ] {
            assert!(refused(css, true), "{css}");
            assert!(refused(css, false), "{css}");
        }
    }

    #[test]
    fn escapes_and_stray_line_ends_stay_refused() {
        for css in [
            "a { background: \\75 rl(https://evil/x) }",
            "@\\69mport 'https://evil/x.css';",
            "a { background: url(a\\b.png) }",
            "a {\r}",
            "a {\u{c}}",
        ] {
            assert!(refused(css, true), "{css}");
        }
    }

    #[test]
    fn nesting_is_bounded() {
        let ok = format!(
            "a {{ b: {}1{} }}",
            "f(".repeat(MAX_DEPTH - 2),
            ")".repeat(MAX_DEPTH - 2)
        );
        assert_eq!(check_css(&ok, false), Ok(()));
        let deep = format!("a {{ b: {}1{} }}", "f(".repeat(5000), ")".repeat(5000));
        assert!(refused(&deep, false));
        let wide = "(".repeat(100_000);
        assert!(refused(&wide, false));
    }
}
