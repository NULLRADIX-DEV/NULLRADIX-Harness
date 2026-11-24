use std::path::Path;

use super::Violation;

/// Files that may opt out of the workspace `unsafe_code` lint.
const ALLOWED: &[&str] = &[];
const MESSAGE: &str = "unsafe_code opt-out outside the xtask allowlist";

fn opts_out(text: &str) -> bool {
    let trimmed = text.trim_start();
    let attribute = trimmed.starts_with("#[") || trimmed.starts_with("#![");
    attribute
        && trimmed.contains("unsafe_code")
        && (trimmed.contains("allow(") || trimmed.contains("expect("))
}

pub(super) fn check(path: &Path, content: &str) -> Vec<Violation> {
    if ALLOWED.iter().any(|allowed| path == Path::new(allowed)) {
        return Vec::new();
    }
    content
        .lines()
        .enumerate()
        .filter(|(_, text)| opts_out(text))
        .map(|(index, _)| Violation {
            path: path.to_path_buf(),
            line: index + 1,
            message: MESSAGE,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_lint_opt_outs() {
        let content = "fn a() {}\n#[allow(unsafe_code)]\nfn b() {}\n  #![expect(unsafe_code)]\n";
        let lines: Vec<usize> = check(Path::new("a.rs"), content)
            .into_iter()
            .map(|v| v.line)
            .collect();
        assert_eq!(lines, vec![2, 4]);
    }

    #[test]
    fn ignores_other_attributes_and_strings() {
        let content = "#[allow(dead_code)]\nlet s = \"#[allow(unsafe_code)]\";\n";
        assert_eq!(check(Path::new("a.rs"), content), vec![]);
    }
}
