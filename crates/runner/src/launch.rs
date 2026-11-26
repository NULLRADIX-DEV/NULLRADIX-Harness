//! Launching on Windows: batch files go through `cmd`, Git Bash is looked up (paths as text).

use std::path::Path;

const EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

/// How the supervisor starts a program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Launch {
    /// The program is started as it is.
    Direct,
    /// A batch file: `cmd /d /s /c "<line>"`, with the line quoted by hand.
    Batch(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LaunchError {
    #[error("{}", not_found(.0))]
    NotFound(String),
    #[error("claude-arg-unsafe: an argument contains a character cmd would interpret")]
    UnsafeArgument,
}

/// The one message for a Claude program that cannot be found, the same on every system.
pub(crate) fn not_found(program: &str) -> String {
    format!("claude-not-found: {program} does not exist")
}

fn has_separator(program: &str) -> bool {
    program.contains(['\\', '/'])
}

fn join(dir: &str, name: &str) -> String {
    format!("{}\\{name}", dir.trim_end_matches(['\\', '/']))
}

fn extension(program: &str) -> Option<String> {
    let name = program.rsplit(['\\', '/']).next().unwrap_or(program);
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
}

/// A bare name is searched along `path`, trying `.exe`, `.cmd`, then `.bat` per folder.
fn search(name: &str, path: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    path.split(';')
        .filter(|dir| !dir.trim().is_empty())
        .find_map(|dir| {
            EXTENSIONS
                .iter()
                .map(|extension| join(dir.trim(), &format!("{name}.{extension}")))
                .find(|candidate| exists(candidate))
        })
}

/// Quotes one argument for a `cmd` line; characters cmd still interprets inside quotes are refused.
pub(crate) fn quote(argument: &str) -> Result<String, LaunchError> {
    if argument.contains(['"', '%', '!', '^']) || argument.chars().any(char::is_control) {
        return Err(LaunchError::UnsafeArgument);
    }
    let plain = !argument.is_empty()
        && argument.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/' | '\\' | '=')
        });
    Ok(if plain {
        argument.to_owned()
    } else {
        // backslashes before the closing quote would escape it
        let trailing = argument.len() - argument.trim_end_matches('\\').len();
        format!("\"{argument}{}\"", "\\".repeat(trailing))
    })
}

/// Which file to start and how, on Windows; `path` is the `PATH` of the run.
pub(crate) fn plan(
    program: &str,
    args: &[String],
    path: &str,
    exists: &dyn Fn(&str) -> bool,
) -> Result<(String, Launch), LaunchError> {
    let resolved = if has_separator(program) {
        if !exists(program) {
            return Err(LaunchError::NotFound(program.to_owned()));
        }
        program.to_owned()
    } else if extension(program).is_some() {
        program.to_owned()
    } else {
        search(program, path, exists).ok_or_else(|| LaunchError::NotFound(program.to_owned()))?
    };
    let batch = matches!(extension(&resolved).as_deref(), Some("cmd" | "bat"));
    if !batch {
        return Ok((resolved, Launch::Direct));
    }
    let mut line = vec![quote(&resolved)?];
    for argument in args {
        line.push(quote(argument)?);
    }
    Ok((resolved, Launch::Batch(line.join(" "))))
}

/// Looks for `bash.exe` of Git for Windows: the given path, next to `git`, the usual folders.
pub(crate) fn git_bash(
    env: &dyn Fn(&str) -> Option<String>,
    git: Option<&str>,
    exists: &dyn Fn(&str) -> bool,
) -> Option<String> {
    if let Some(given) = env("CLAUDE_CODE_GIT_BASH_PATH")
        .map(|given| given.trim().to_owned())
        .filter(|given| !given.is_empty() && exists(given))
    {
        return Some(given);
    }
    if let Some(found) = git.and_then(|git| next_to_git(git, exists)) {
        return Some(found);
    }
    let local = env("LOCALAPPDATA").map(|dir| join(&dir, "Programs"));
    [env("ProgramFiles"), env("ProgramFiles(x86)"), local]
        .into_iter()
        .flatten()
        .map(|root| join(&join(&root, "Git"), "bin\\bash.exe"))
        .find(|candidate| exists(candidate))
}

fn dirname(path: &str) -> &str {
    path.rsplit_once(['\\', '/']).map_or("", |(dir, _)| dir)
}

fn leaf(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// `git.exe` sits in `cmd`, `mingw64/bin`, `usr/bin` or `bin` of the Git root.
fn next_to_git(git: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let mut dir = dirname(git);
    if leaf(dir) == "bin" && ["mingw64", "mingw32", "usr"].contains(&leaf(dirname(dir)).as_str()) {
        dir = dirname(dir);
    }
    let candidate = join(dirname(dir), "bin\\bash.exe");
    exists(&candidate).then_some(candidate)
}

/// The first `git` the system finds on the `PATH`, asking `where` once.
pub(crate) fn where_git() -> Option<String> {
    let output = std::process::Command::new("where")
        .arg("git")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Whether a file exists on this machine.
pub(crate) fn file_exists(path: &str) -> bool {
    Path::new(path).is_file()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn files(paths: &[&str]) -> impl Fn(&str) -> bool {
        let set: BTreeSet<String> = paths.iter().map(|path| (*path).to_owned()).collect();
        move |path| set.contains(path)
    }

    #[test]
    fn a_bare_name_is_searched_in_path_order_exe_before_cmd() {
        let exists = files(&[
            "C:\\npm\\claude.cmd",
            "C:\\bin\\claude.exe",
            "C:\\bin\\claude.cmd",
        ]);
        let (found, launch) = plan("claude", &[], "C:\\x;C:\\bin;C:\\npm", &exists).unwrap();
        assert_eq!(
            (found.as_str(), launch),
            ("C:\\bin\\claude.exe", Launch::Direct)
        );
        let (found, launch) = plan("claude", &[], "C:\\npm;C:\\bin", &exists).unwrap();
        assert_eq!(found, "C:\\npm\\claude.cmd");
        assert!(matches!(launch, Launch::Batch(_)));
    }

    #[test]
    fn the_missing_program_message_names_the_program() {
        assert_eq!(
            not_found("claude"),
            "claude-not-found: claude does not exist"
        );
        assert_eq!(
            LaunchError::NotFound(String::from("claude")).to_string(),
            not_found("claude")
        );
    }

    #[test]
    fn a_path_must_exist_but_a_name_with_an_extension_is_left_to_the_system() {
        let exists = files(&[]);
        assert_eq!(
            plan("C:\\tools\\claude.exe", &[], "", &exists),
            Err(LaunchError::NotFound(String::from("C:\\tools\\claude.exe")))
        );
        assert_eq!(
            plan("claude.exe", &[], "", &exists).unwrap().1,
            Launch::Direct
        );
        assert!(plan("claude", &[], "C:\\bin", &exists).is_err());
    }

    #[test]
    fn batch_files_get_a_quoted_line_and_unsafe_arguments_are_refused() {
        let exists = files(&["C:\\npm\\claude.cmd"]);
        let args = vec![String::from("-p"), String::from("Bash(git status:*)")];
        let (_, launch) = plan("claude", &args, "C:\\npm", &exists).unwrap();
        assert_eq!(
            launch,
            Launch::Batch(String::from(
                "C:\\npm\\claude.cmd -p \"Bash(git status:*)\""
            ))
        );
        for bad in ["a\"b", "100%", "wow!", "a^b"] {
            assert_eq!(
                plan("claude", &[bad.to_owned()], "C:\\npm", &exists),
                Err(LaunchError::UnsafeArgument),
                "{bad}"
            );
        }
        for bad in ["a\rb", "a\nb", "a\tb", "a\u{0}b"] {
            assert_eq!(quote(bad), Err(LaunchError::UnsafeArgument), "{bad:?}");
        }
        assert_eq!(quote("C:\\my dir\\").unwrap(), "\"C:\\my dir\\\\\"");
        assert_eq!(quote("a b\\\\").unwrap(), "\"a b\\\\\\\\\"");
        assert_eq!(quote("").unwrap(), "\"\"");
        assert_eq!(quote("--model=opus").unwrap(), "--model=opus");
    }

    #[test]
    fn git_bash_comes_from_the_setting_then_git_then_the_usual_folders() {
        let exists = files(&[
            "D:\\bash\\bash.exe",
            "C:\\Program Files\\Git\\bin\\bash.exe",
            "E:\\Git\\bin\\bash.exe",
        ]);
        let env = |name: &str| match name {
            "CLAUDE_CODE_GIT_BASH_PATH" => Some(String::from("D:\\bash\\bash.exe")),
            "ProgramFiles" => Some(String::from("C:\\Program Files")),
            _ => None,
        };
        assert_eq!(
            git_bash(&env, None, &exists).as_deref(),
            Some("D:\\bash\\bash.exe")
        );
        let none = |_: &str| None;
        for git in [
            "E:\\Git\\cmd\\git.exe",
            "E:\\Git\\mingw64\\bin\\git.exe",
            "E:\\Git\\usr\\bin\\git.exe",
            "E:\\Git\\bin\\git.exe",
        ] {
            assert_eq!(
                git_bash(&none, Some(git), &exists).as_deref(),
                Some("E:\\Git\\bin\\bash.exe"),
                "{git}"
            );
        }
        let program_files =
            |name: &str| (name == "ProgramFiles").then(|| String::from("C:\\Program Files"));
        assert_eq!(
            git_bash(&program_files, None, &exists).as_deref(),
            Some("C:\\Program Files\\Git\\bin\\bash.exe")
        );
        assert_eq!(git_bash(&none, None, &exists), None);
    }
}
