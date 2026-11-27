//! The local Claude settings of a project folder and the git exclude file of its repository.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::write_atomic;

/// The untracked local settings of a project, relative to its root.
pub const LOCAL_SETTINGS: &str = ".claude/settings.local.json";

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("{} is not valid JSON", .0.display())]
    Invalid(PathBuf),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> SettingsError {
    let path = path.to_path_buf();
    move |source| SettingsError::Io { path, source }
}

fn read_text(path: &Path) -> Result<Option<String>, SettingsError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(path)(error)),
    }
}

fn write(path: &Path, text: &str) -> Result<(), SettingsError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_error(dir))?;
    }
    write_atomic(path, text.as_bytes()).map_err(io_error(path))
}

/// Turns the plugin on in the local settings; true when written, broken JSON is never changed.
pub fn enable_plugin(root: &Path, id: &str) -> Result<bool, SettingsError> {
    let file = root.join(LOCAL_SETTINGS);
    let mut settings = match read_text(&file)? {
        Some(text) if !text.trim().is_empty() => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(settings)) => settings,
            _ => return Err(SettingsError::Invalid(file)),
        },
        _ => Map::new(),
    };
    let mut plugins = match settings.remove("enabledPlugins") {
        Some(Value::Object(plugins)) => plugins,
        _ => Map::new(),
    };
    if plugins.get(id) == Some(&Value::Bool(true)) {
        return Ok(false);
    }
    plugins.insert(id.to_owned(), Value::Bool(true));
    settings.insert("enabledPlugins".to_owned(), Value::Object(plugins));
    let text = serde_json::to_string_pretty(&Value::Object(settings))
        .map_err(|error| io_error(&file)(io::Error::other(error)))?;
    write(&file, &(text + "\n"))?;
    Ok(true)
}

/// Adds the missing lines to `info/exclude` of the common git folder; true when written.
pub fn add_git_excludes(common_dir: &Path, lines: &[String]) -> Result<bool, SettingsError> {
    let file = common_dir.join("info").join("exclude");
    let mut text = read_text(&file)?.unwrap_or_default();
    let mut changed = false;
    for line in lines {
        if !text.lines().any(|known| known == line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line);
            text.push('\n');
            changed = true;
        }
    }
    if changed {
        write(&file, &text)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "nullradix-dev@nullradix-dev-local";

    #[test]
    fn enables_once_and_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(LOCAL_SETTINGS);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            r#"{"permissions":{"allow":["Bash"]},"enabledPlugins":{"x@y":true}}"#,
        )
        .unwrap();

        assert!(enable_plugin(dir.path(), ID).unwrap());
        assert!(!enable_plugin(dir.path(), ID).unwrap());

        let kept: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(kept["permissions"]["allow"][0], "Bash");
        assert_eq!(kept["enabledPlugins"]["x@y"], true);
        assert_eq!(kept["enabledPlugins"][ID], true);
    }

    #[test]
    fn broken_settings_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(LOCAL_SETTINGS);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "{ not json").unwrap();

        let error = enable_plugin(dir.path(), ID).unwrap_err();

        assert!(matches!(error, SettingsError::Invalid(_)));
        assert_eq!(fs::read_to_string(&file).unwrap(), "{ not json");
    }

    #[test]
    fn excludes_are_appended_once_after_the_own_lines() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("info")).unwrap();
        fs::write(dir.path().join("info/exclude"), "/mine").unwrap();
        let lines = vec![String::from("/a"), String::from("/b")];

        assert!(add_git_excludes(dir.path(), &lines).unwrap());
        assert!(!add_git_excludes(dir.path(), &lines).unwrap());

        assert_eq!(
            fs::read_to_string(dir.path().join("info/exclude")).unwrap(),
            "/mine\n/a\n/b\n"
        );
    }
}
