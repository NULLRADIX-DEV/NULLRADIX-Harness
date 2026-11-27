use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::files;

mod comments;
mod layers;
mod release_lock;
mod sdk_version;
mod unsafe_code;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Violation {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
    pub(crate) message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.path.display(), self.line, self.message)
    }
}

/// Comment syntax of a checked source or config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Slash,
    Hash,
    Css,
}

/// Generated files the source checks skip.
const GENERATED: &[&str] = &[crate::codegen::OPENAPI_PATH, "web/src/shared/api/schema.ts"];

fn is_generated(path: &Path) -> bool {
    GENERATED
        .iter()
        .any(|generated| path == Path::new(generated))
}

fn syntax(path: &Path) -> Option<Syntax> {
    let name = path.file_name()?.to_str()?;
    if matches!(
        name,
        ".gitignore" | ".gitattributes" | ".editorconfig" | ".prettierignore" | "Dockerfile"
    ) {
        return Some(Syntax::Hash);
    }
    match path.extension()?.to_str()? {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(Syntax::Slash),
        "toml" | "yml" | "yaml" | "sh" | "ps1" | "py" => Some(Syntax::Hash),
        "css" => Some(Syntax::Css),
        _ => None,
    }
}

pub(crate) fn run(root: &Path) -> Result<Vec<Violation>> {
    let mut violations = Vec::new();
    for path in files::repository_files(root)? {
        if is_generated(&path) {
            continue;
        }
        let Some(syntax) = syntax(&path) else {
            continue;
        };
        let absolute = root.join(&path);
        if !absolute.is_file() {
            continue;
        }
        let content = fs::read_to_string(&absolute)
            .with_context(|| format!("failed to read {}", path.display()))?;
        violations.extend(comments::check(&path, &content, syntax));
        if path.extension().is_some_and(|extension| extension == "rs") {
            violations.extend(unsafe_code::check(&path, &content));
        }
    }
    violations.extend(layers::run(root)?);
    violations.extend(release_lock::run(root)?);
    violations.extend(sdk_version::run(root)?);
    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_syntax_by_extension_and_name() {
        assert_eq!(
            syntax(Path::new("crates/a/src/lib.rs")),
            Some(Syntax::Slash)
        );
        assert_eq!(
            syntax(Path::new(".github/workflows/ci.yml")),
            Some(Syntax::Hash)
        );
        assert_eq!(syntax(Path::new(".gitignore")), Some(Syntax::Hash));
        assert_eq!(syntax(Path::new("web/src/app.css")), Some(Syntax::Css));
        assert_eq!(syntax(Path::new("README.md")), None);
        assert_eq!(syntax(Path::new("LICENSE")), None);
    }

    #[test]
    fn skips_generated_files_only() {
        assert!(is_generated(Path::new("web/src/shared/api/schema.ts")));
        assert!(is_generated(Path::new("web/src/shared/api/openapi.json")));
        assert!(!is_generated(Path::new("web/src/shared/api/client.ts")));
        assert!(!is_generated(Path::new("schema.ts")));
    }

    #[test]
    fn formats_violation_as_path_line_message() {
        let violation = Violation {
            path: PathBuf::from("a.rs"),
            line: 3,
            message: String::from("boom"),
        };
        assert_eq!(violation.to_string(), "a.rs:3: boom");
    }
}
