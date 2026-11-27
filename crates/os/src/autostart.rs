//! Starting a program when the person logs in: the Run key on Windows, an XDG autostart file on Linux.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
#[cfg(any(windows, test))]
use std::time::Duration;

#[cfg(any(windows, test))]
use crate::command;

#[cfg(any(windows, test))]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(any(windows, test))]
const REG_LIMIT: Duration = Duration::from_secs(10);

/// What starts at login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartEntry {
    /// File or value name, such as `nullradix`.
    pub name: String,
    /// Shown by the desktop environment.
    pub label: String,
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Whether an entry of this name starts at login.
pub fn autostart_enabled(name: &str) -> io::Result<bool> {
    platform::enabled(name)
}

/// Creates the entry, or replaces one of the same name.
pub fn autostart_enable(entry: &AutostartEntry) -> io::Result<()> {
    platform::enable(entry)
}

/// Removes the entry; none is fine.
pub fn autostart_disable(name: &str) -> io::Result<()> {
    platform::disable(name)
}

/// Quotes one word of a Windows command line.
#[cfg(any(windows, test))]
fn windows_word(word: &str) -> String {
    if !word.is_empty() && !word.contains([' ', '\t', '"']) {
        return word.to_owned();
    }
    format!("\"{}\"", word.replace('"', "\\\""))
}

/// The command line the Run key holds.
#[cfg(any(windows, test))]
fn windows_command(entry: &AutostartEntry) -> String {
    let mut words = vec![windows_word(&entry.program.to_string_lossy())];
    words.extend(
        entry
            .args
            .iter()
            .map(|arg| windows_word(&arg.to_string_lossy())),
    );
    words.join(" ")
}

#[cfg(any(windows, test))]
fn reg_add_args(entry: &AutostartEntry) -> Vec<String> {
    [
        "add",
        RUN_KEY,
        "/v",
        &entry.name,
        "/t",
        "REG_SZ",
        "/d",
        &windows_command(entry),
        "/f",
    ]
    .map(String::from)
    .to_vec()
}

#[cfg(any(windows, test))]
fn reg_query_args(name: &str) -> Vec<String> {
    ["query", RUN_KEY, "/v", name].map(String::from).to_vec()
}

#[cfg(any(windows, test))]
fn reg_delete_args(name: &str) -> Vec<String> {
    ["delete", RUN_KEY, "/v", name, "/f"]
        .map(String::from)
        .to_vec()
}

#[cfg(windows)]
fn reg(args: &[String]) -> io::Result<command::Reply> {
    command::run("reg", args, None, REG_LIMIT)
}

#[cfg(windows)]
fn reg_failed(reply: &command::Reply) -> io::Error {
    io::Error::other(format!("reg.exe failed: {}", reply.stderr.trim()))
}

/// One word of an `Exec` line: quoted when it holds a character the format reserves.
#[cfg(any(target_os = "linux", test))]
fn desktop_word(word: &str) -> String {
    let reserved = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '\\'
                    | '>'
                    | '<'
                    | '~'
                    | '|'
                    | '&'
                    | ';'
                    | '$'
                    | '*'
                    | '?'
                    | '#'
                    | '('
                    | ')'
                    | '`'
            )
    };
    let word = word.replace('%', "%%");
    if !word.is_empty() && !word.contains(reserved) {
        return word;
    }
    let mut quoted = String::from('"');
    for c in word.chars() {
        match c {
            // the quoting escape, then the escape of the file format itself
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The content of the autostart file.
#[cfg(any(target_os = "linux", test))]
fn desktop_file(entry: &AutostartEntry) -> String {
    let label: String = entry.label.chars().filter(|c| !c.is_control()).collect();
    let mut exec = vec![desktop_word(&entry.program.to_string_lossy())];
    exec.extend(
        entry
            .args
            .iter()
            .map(|arg| desktop_word(&arg.to_string_lossy())),
    );
    format!(
        "[Desktop Entry]\nType=Application\nName={label}\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec.join(" ")
    )
}

#[cfg(any(target_os = "linux", test))]
fn desktop_path(config_dir: &std::path::Path, name: &str) -> PathBuf {
    config_dir.join("autostart").join(format!("{name}.desktop"))
}

#[cfg(any(target_os = "linux", test))]
fn write_desktop(config_dir: &std::path::Path, entry: &AutostartEntry) -> io::Result<()> {
    let path = desktop_path(config_dir, &entry.name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::write_atomic(&path, desktop_file(entry).as_bytes())
}

#[cfg(any(target_os = "linux", test))]
fn remove_desktop(config_dir: &std::path::Path, name: &str) -> io::Result<()> {
    match std::fs::remove_file(desktop_path(config_dir, name)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(windows)]
mod platform {
    use std::io;

    use super::{AutostartEntry, reg, reg_add_args, reg_delete_args, reg_failed, reg_query_args};

    pub(super) fn enabled(name: &str) -> io::Result<bool> {
        Ok(reg(&reg_query_args(name))?.success)
    }

    pub(super) fn enable(entry: &AutostartEntry) -> io::Result<()> {
        let reply = reg(&reg_add_args(entry))?;
        if reply.success {
            Ok(())
        } else {
            Err(reg_failed(&reply))
        }
    }

    pub(super) fn disable(name: &str) -> io::Result<()> {
        if !enabled(name)? {
            return Ok(());
        }
        let reply = reg(&reg_delete_args(name))?;
        if reply.success {
            Ok(())
        } else {
            Err(reg_failed(&reply))
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::io;
    use std::path::PathBuf;

    use super::{AutostartEntry, desktop_path, remove_desktop, write_desktop};

    fn config_dir() -> io::Result<PathBuf> {
        directories::BaseDirs::new()
            .map(|dirs| dirs.config_dir().to_path_buf())
            .ok_or_else(|| io::Error::other("no home directory found"))
    }

    pub(super) fn enabled(name: &str) -> io::Result<bool> {
        Ok(desktop_path(&config_dir()?, name).is_file())
    }

    pub(super) fn enable(entry: &AutostartEntry) -> io::Result<()> {
        write_desktop(&config_dir()?, entry)
    }

    pub(super) fn disable(name: &str) -> io::Result<()> {
        remove_desktop(&config_dir()?, name)
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use std::io;

    use super::AutostartEntry;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "starting at login is not supported on this system",
        )
    }

    pub(super) fn enabled(_name: &str) -> io::Result<bool> {
        Err(unsupported())
    }

    pub(super) fn enable(_entry: &AutostartEntry) -> io::Result<()> {
        Err(unsupported())
    }

    pub(super) fn disable(_name: &str) -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(program: &str, args: &[&str]) -> AutostartEntry {
        AutostartEntry {
            name: String::from("nullradix"),
            label: String::from("NULLRADIX"),
            program: PathBuf::from(program),
            args: args.iter().map(OsString::from).collect(),
        }
    }

    #[test]
    fn a_windows_command_quotes_only_what_needs_it() {
        let line = windows_command(&entry(
            r"C:\Program Files\NULLRADIX\nullradix-desktop.exe",
            &["--hidden", "--home", r"D:\my data"],
        ));
        assert_eq!(
            line,
            r#""C:\Program Files\NULLRADIX\nullradix-desktop.exe" --hidden --home "D:\my data""#
        );
        assert_eq!(windows_word(""), "\"\"");
    }

    #[test]
    fn the_run_key_commands_name_the_key_value_and_data() {
        let add = reg_add_args(&entry("nullradix.exe", &["serve"]));
        assert_eq!(&add[..4], ["add", RUN_KEY, "/v", "nullradix"]);
        assert_eq!(add[7], "nullradix.exe serve");
        assert_eq!(add.last().map(String::as_str), Some("/f"));
        assert_eq!(reg_query_args("x"), ["query", RUN_KEY, "/v", "x"]);
        assert_eq!(reg_delete_args("x"), ["delete", RUN_KEY, "/v", "x", "/f"]);
    }

    #[test]
    fn the_exec_line_quotes_reserved_characters_and_doubles_percent() {
        assert_eq!(desktop_word("/usr/bin/nullradix"), "/usr/bin/nullradix");
        assert_eq!(desktop_word("/opt/my app/x"), "\"/opt/my app/x\"");
        assert_eq!(desktop_word("100%"), "100%%");
        assert_eq!(desktop_word("a$b"), "\"a\\\\$b\"");
        assert_eq!(desktop_word(""), "\"\"");
    }

    #[test]
    fn the_autostart_file_starts_the_program_with_its_arguments() {
        let text = desktop_file(&entry(
            "/usr/bin/nullradix-desktop",
            &["--hidden", "--profile", "release"],
        ));
        assert_eq!(
            text,
            "[Desktop Entry]\nType=Application\nName=NULLRADIX\nExec=/usr/bin/nullradix-desktop --hidden --profile release\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
    }

    #[test]
    fn the_autostart_file_lives_in_the_config_autostart_folder() {
        let dir = tempfile::tempdir().unwrap();
        let wanted = entry("/usr/bin/nullradix", &["serve"]);
        let path = desktop_path(dir.path(), "nullradix");
        assert_eq!(path, dir.path().join("autostart").join("nullradix.desktop"));
        assert!(!path.exists());
        write_desktop(dir.path(), &wanted).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            desktop_file(&wanted)
        );
        // writing again replaces the file
        write_desktop(dir.path(), &entry("/other/nullradix", &[])).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("Exec=/other/nullradix\n")
        );
        remove_desktop(dir.path(), "nullradix").unwrap();
        assert!(!path.exists());
        remove_desktop(dir.path(), "nullradix").unwrap();
    }
}
