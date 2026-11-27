//! Start at login via the Run key or an XDG file; a desktop that disabled it stays authoritative.

use std::ffi::OsString;
use std::io;
#[cfg(any(target_os = "linux", test))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use crate::command;

#[cfg(any(windows, test))]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// The same key below `HKCU`.
#[cfg(windows)]
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Windows keeps the switch of the Startup apps settings page.
#[cfg(any(windows, test))]
const APPROVED_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
#[cfg(windows)]
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

/// What the system holds for a name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutostartState {
    pub exists: bool,
    /// The person switched the entry off in the desktop's own settings.
    pub disabled: bool,
    /// The program the entry starts.
    pub program: Option<PathBuf>,
}

impl AutostartState {
    /// Whether the entry starts at login.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.exists && !self.disabled
    }
}

pub fn autostart_state(name: &str) -> io::Result<AutostartState> {
    platform::state(name)
}

/// Creates the entry, or replaces one of the same name, and switches it on.
pub fn autostart_enable(entry: &AutostartEntry) -> io::Result<()> {
    platform::enable(entry)
}

/// Switches an existing entry on again and keeps what it starts.
pub fn autostart_reenable(name: &str) -> io::Result<()> {
    platform::reenable(name)
}

/// Points an existing entry at another program and keeps its switch; no entry stays none.
pub fn autostart_retarget(entry: &AutostartEntry) -> io::Result<()> {
    platform::retarget(entry)
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
fn reg_query_args(key: &str, name: &str) -> Vec<String> {
    ["query", key, "/v", name].map(String::from).to_vec()
}

#[cfg(any(windows, test))]
fn reg_delete_args(key: &str, name: &str) -> Vec<String> {
    ["delete", key, "/v", name, "/f"].map(String::from).to_vec()
}

/// The program a Run value starts: its first word.
#[cfg(any(windows, test))]
fn command_program(data: &str) -> Option<PathBuf> {
    let data = data.trim();
    let program = match data.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => data.split_whitespace().next()?,
    };
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// Whether the `StartupApproved` value says off: its first byte is odd (3), on is even (2).
#[cfg(any(windows, test))]
fn approved_off(output: &str) -> bool {
    let Some(line) = output.lines().find(|line| line.contains("REG_BINARY")) else {
        return false;
    };
    let hex = line.rsplit("REG_BINARY").next().unwrap_or("").trim();
    hex.get(..2)
        .and_then(|byte| u8::from_str_radix(byte, 16).ok())
        .is_some_and(|byte| byte & 1 == 1)
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

/// The first word of an `Exec` value, undone from `desktop_word`.
#[cfg(any(target_os = "linux", test))]
fn desktop_program(exec: &str) -> Option<String> {
    let exec = exec.trim();
    let Some(rest) = exec.strip_prefix('"') else {
        let word = exec.split_whitespace().next()?;
        return Some(word.replace("%%", "%"));
    };
    let mut word = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => {
                // the file format halves backslashes, then the quoting escape takes one more
                let mut run = 1;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    run += 1;
                }
                word.push_str(&"\\".repeat(run / 4));
                if run % 4 >= 2
                    && let Some(next) = chars.next()
                {
                    word.push(next);
                }
            }
            _ => word.push(c),
        }
    }
    Some(word.replace("%%", "%"))
}

#[cfg(any(target_os = "linux", test))]
fn exec_line(entry: &AutostartEntry) -> String {
    let mut exec = vec![desktop_word(&entry.program.to_string_lossy())];
    exec.extend(
        entry
            .args
            .iter()
            .map(|arg| desktop_word(&arg.to_string_lossy())),
    );
    format!("Exec={}", exec.join(" "))
}

/// The content of the autostart file.
#[cfg(any(target_os = "linux", test))]
fn desktop_file(entry: &AutostartEntry) -> String {
    let label: String = entry.label.chars().filter(|c| !c.is_control()).collect();
    format!(
        "[Desktop Entry]\nType=Application\nName={label}\n{}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec_line(entry)
    )
}

/// What an autostart file says; a desktop that hides or disables it wins.
#[cfg(any(target_os = "linux", test))]
fn desktop_state(content: &str) -> AutostartState {
    let mut state = AutostartState {
        exists: true,
        ..AutostartState::default()
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(exec) = line.strip_prefix("Exec=") {
            state.program = desktop_program(exec).map(PathBuf::from);
        } else if line.eq_ignore_ascii_case("Hidden=true")
            || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
        {
            state.disabled = true;
        }
    }
    state
}

/// The file with another `Exec` line and every other line, flags included, as it was.
#[cfg(any(target_os = "linux", test))]
fn desktop_retargeted(content: &str, entry: &AutostartEntry) -> String {
    let exec = exec_line(entry);
    let mut replaced = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("Exec=") {
                replaced = true;
                exec.clone()
            } else {
                line.to_owned()
            }
        })
        .collect();
    if !replaced {
        lines.push(exec);
    }
    lines.join("\n") + "\n"
}

/// The file with the switches turned on and the rest as it was.
#[cfg(any(target_os = "linux", test))]
fn desktop_reenabled(content: &str) -> String {
    let mut lines: Vec<String> = content
        .lines()
        .filter(|line| !line.trim().to_ascii_lowercase().starts_with("hidden="))
        .map(|line| {
            if line
                .trim()
                .to_ascii_lowercase()
                .starts_with("x-gnome-autostart-enabled=")
            {
                String::from("X-GNOME-Autostart-enabled=true")
            } else {
                line.to_owned()
            }
        })
        .collect();
    if !lines
        .iter()
        .any(|line| line.starts_with("X-GNOME-Autostart-enabled="))
    {
        lines.push(String::from("X-GNOME-Autostart-enabled=true"));
    }
    lines.join("\n") + "\n"
}

#[cfg(any(target_os = "linux", test))]
fn desktop_path(config_dir: &Path, name: &str) -> PathBuf {
    config_dir.join("autostart").join(format!("{name}.desktop"))
}

#[cfg(any(target_os = "linux", test))]
fn read_state(config_dir: &Path, name: &str) -> io::Result<AutostartState> {
    match std::fs::read_to_string(desktop_path(config_dir, name)) {
        Ok(content) => Ok(desktop_state(&content)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AutostartState::default()),
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "linux", test))]
fn write_desktop(config_dir: &Path, name: &str, content: &str) -> io::Result<()> {
    let path = desktop_path(config_dir, name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::write_atomic(&path, content.as_bytes())
}

#[cfg(any(target_os = "linux", test))]
fn enable_desktop(config_dir: &Path, entry: &AutostartEntry) -> io::Result<()> {
    write_desktop(config_dir, &entry.name, &desktop_file(entry))
}

/// Applies `change` to the file when there is one.
#[cfg(any(target_os = "linux", test))]
fn change_desktop(
    config_dir: &Path,
    name: &str,
    change: impl FnOnce(&str) -> String,
) -> io::Result<()> {
    match std::fs::read_to_string(desktop_path(config_dir, name)) {
        Ok(content) => write_desktop(config_dir, name, &change(&content)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "linux", test))]
fn remove_desktop(config_dir: &Path, name: &str) -> io::Result<()> {
    match std::fs::remove_file(desktop_path(config_dir, name)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(windows)]
mod platform {
    use std::io;

    use super::{
        APPROVED_KEY, AutostartEntry, AutostartState, RUN_KEY, RUN_SUBKEY, approved_off,
        command_program, reg, reg_add_args, reg_delete_args, reg_failed, reg_query_args,
    };

    pub(super) fn state(name: &str) -> io::Result<AutostartState> {
        // read as UTF-16: reg.exe writes the OEM code page to a pipe and garbles other paths
        let Some(command) = crate::windows::registry_read_string(RUN_SUBKEY, name)? else {
            return Ok(AutostartState::default());
        };
        let approved = reg(&reg_query_args(APPROVED_KEY, name))?;
        Ok(AutostartState {
            exists: true,
            disabled: approved.success && approved_off(&approved.stdout),
            program: command_program(&command),
        })
    }

    fn add(entry: &AutostartEntry) -> io::Result<()> {
        let reply = reg(&reg_add_args(entry))?;
        if reply.success {
            Ok(())
        } else {
            Err(reg_failed(&reply))
        }
    }

    /// Drops the off marker of the Startup apps page; none is fine.
    fn clear_marker(name: &str) -> io::Result<()> {
        if reg(&reg_query_args(APPROVED_KEY, name))?.success {
            let reply = reg(&reg_delete_args(APPROVED_KEY, name))?;
            if !reply.success {
                return Err(reg_failed(&reply));
            }
        }
        Ok(())
    }

    pub(super) fn enable(entry: &AutostartEntry) -> io::Result<()> {
        add(entry)?;
        clear_marker(&entry.name)
    }

    pub(super) fn reenable(name: &str) -> io::Result<()> {
        clear_marker(name)
    }

    pub(super) fn retarget(entry: &AutostartEntry) -> io::Result<()> {
        if state(&entry.name)?.exists {
            add(entry)
        } else {
            Ok(())
        }
    }

    pub(super) fn disable(name: &str) -> io::Result<()> {
        if reg(&reg_query_args(RUN_KEY, name))?.success {
            let reply = reg(&reg_delete_args(RUN_KEY, name))?;
            if !reply.success {
                return Err(reg_failed(&reply));
            }
        }
        clear_marker(name)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::io;
    use std::path::PathBuf;

    use super::{
        AutostartEntry, AutostartState, change_desktop, desktop_reenabled, desktop_retargeted,
        enable_desktop, read_state, remove_desktop,
    };

    fn config_dir() -> io::Result<PathBuf> {
        directories::BaseDirs::new()
            .map(|dirs| dirs.config_dir().to_path_buf())
            .ok_or_else(|| io::Error::other("no home directory found"))
    }

    pub(super) fn state(name: &str) -> io::Result<AutostartState> {
        read_state(&config_dir()?, name)
    }

    pub(super) fn enable(entry: &AutostartEntry) -> io::Result<()> {
        enable_desktop(&config_dir()?, entry)
    }

    pub(super) fn reenable(name: &str) -> io::Result<()> {
        change_desktop(&config_dir()?, name, desktop_reenabled)
    }

    pub(super) fn retarget(entry: &AutostartEntry) -> io::Result<()> {
        change_desktop(&config_dir()?, &entry.name, |content| {
            desktop_retargeted(content, entry)
        })
    }

    pub(super) fn disable(name: &str) -> io::Result<()> {
        remove_desktop(&config_dir()?, name)
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use std::io;

    use super::{AutostartEntry, AutostartState};

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "starting at login is not supported on this system",
        )
    }

    pub(super) fn state(_name: &str) -> io::Result<AutostartState> {
        Err(unsupported())
    }

    pub(super) fn enable(_entry: &AutostartEntry) -> io::Result<()> {
        Err(unsupported())
    }

    pub(super) fn reenable(_name: &str) -> io::Result<()> {
        Err(unsupported())
    }

    pub(super) fn retarget(_entry: &AutostartEntry) -> io::Result<()> {
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
        assert_eq!(
            reg_query_args(APPROVED_KEY, "x"),
            ["query", APPROVED_KEY, "/v", "x"]
        );
        assert_eq!(
            reg_delete_args(RUN_KEY, "x"),
            ["delete", RUN_KEY, "/v", "x", "/f"]
        );
    }

    #[test]
    fn the_run_value_names_its_program() {
        assert_eq!(
            command_program("\"C:\\Program Files\\N\\nullradix-desktop.exe\" --hidden"),
            Some(PathBuf::from(r"C:\Program Files\N\nullradix-desktop.exe"))
        );
        assert_eq!(
            command_program("C:\\bin\\nullradix.exe serve"),
            Some(PathBuf::from(r"C:\bin\nullradix.exe"))
        );
        assert_eq!(
            command_program("\"C:\\Pf\u{e4}d\\\u{4e2d}.exe\" --x"),
            Some(PathBuf::from("C:\\Pf\u{e4}d\\\u{4e2d}.exe"))
        );
        assert_eq!(command_program("  "), None);
    }

    #[test]
    fn the_startup_apps_page_marks_an_entry_off_with_an_odd_first_byte() {
        let value = |hex: &str| format!("    nullradix    REG_BINARY    {hex}\n");
        assert!(approved_off(&value("030000000000000000000000")));
        assert!(approved_off(&value("070000000000000000000000")));
        assert!(!approved_off(&value("020000000000000000000000")));
        assert!(!approved_off(&value("060000000000000000000000")));
        assert!(!approved_off("no value"));
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
    fn the_program_of_an_exec_line_survives_the_quoting() {
        for program in [
            "/usr/bin/nullradix",
            "/opt/my app/x",
            "/a$b/c",
            "/100%/x",
            "/q\"x/y",
        ] {
            let line = exec_line(&entry(program, &["--hidden"]));
            let exec = line.strip_prefix("Exec=").unwrap();
            assert_eq!(desktop_program(exec).as_deref(), Some(program), "{line}");
        }
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
    fn a_desktop_that_disabled_the_file_makes_it_off() {
        let base = desktop_file(&entry("/usr/bin/nullradix", &["serve"]));
        let on = desktop_state(&base);
        assert!(on.enabled());
        assert_eq!(on.program, Some(PathBuf::from("/usr/bin/nullradix")));
        let gnome = base.replace("Autostart-enabled=true", "Autostart-enabled=false");
        assert!(!desktop_state(&gnome).enabled());
        let hidden = format!("{base}Hidden=true\n");
        assert!(!desktop_state(&hidden).enabled());
        assert!(!AutostartState::default().enabled());
    }

    #[test]
    fn retargeting_keeps_the_flags_and_reenabling_clears_them() {
        let off = format!(
            "{}Hidden=true\n",
            desktop_file(&entry("/old/nullradix", &["serve"]))
                .replace("Autostart-enabled=true", "Autostart-enabled=false")
        );
        let moved = desktop_retargeted(&off, &entry("/new/nullradix", &["serve"]));
        assert!(moved.contains("Exec=/new/nullradix serve\n"));
        assert!(!moved.contains("/old/"));
        let state = desktop_state(&moved);
        assert!(state.disabled);
        assert_eq!(state.program, Some(PathBuf::from("/new/nullradix")));

        let back = desktop_reenabled(&moved);
        let state = desktop_state(&back);
        assert!(state.enabled());
        // the program stays as it was
        assert_eq!(state.program, Some(PathBuf::from("/new/nullradix")));
        assert!(!back.contains("Hidden"));
    }

    #[test]
    fn the_autostart_file_lives_in_the_config_autostart_folder() {
        let dir = tempfile::tempdir().unwrap();
        let wanted = entry("/usr/bin/nullradix", &["serve"]);
        let path = desktop_path(dir.path(), "nullradix");
        assert_eq!(path, dir.path().join("autostart").join("nullradix.desktop"));
        assert!(!read_state(dir.path(), "nullradix").unwrap().exists);
        enable_desktop(dir.path(), &wanted).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            desktop_file(&wanted)
        );
        // writing again replaces the file
        enable_desktop(dir.path(), &entry("/other/nullradix", &[])).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("Exec=/other/nullradix\n")
        );
        // a desktop switches it off; a refresh of the program leaves that alone
        let off = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Autostart-enabled=true", "Autostart-enabled=false");
        std::fs::write(&path, off).unwrap();
        change_desktop(dir.path(), "nullradix", |content| {
            desktop_retargeted(content, &entry("/moved/nullradix", &[]))
        })
        .unwrap();
        let state = read_state(dir.path(), "nullradix").unwrap();
        assert!(state.exists && state.disabled);
        assert_eq!(state.program, Some(PathBuf::from("/moved/nullradix")));
        change_desktop(dir.path(), "nullradix", desktop_reenabled).unwrap();
        assert!(read_state(dir.path(), "nullradix").unwrap().enabled());
        remove_desktop(dir.path(), "nullradix").unwrap();
        assert!(!path.exists());
        remove_desktop(dir.path(), "nullradix").unwrap();
        // nothing to change, nothing created
        change_desktop(dir.path(), "nullradix", desktop_reenabled).unwrap();
        assert!(!path.exists());
    }
}
