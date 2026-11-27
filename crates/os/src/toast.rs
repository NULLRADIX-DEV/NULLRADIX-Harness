//! A desktop notification: a Windows toast, `notify-send` on Linux, `osascript` on macOS.

use std::io;
use std::time::Duration;

use crate::command;

const LIMIT: Duration = Duration::from_secs(20);
const TITLE_MAX: usize = 100;
const BODY_MAX: usize = 400;
/// Id of Windows PowerShell, which every Windows install has registered for toasts.
#[cfg(any(windows, test))]
const WINDOWS_APP_ID: &str =
    r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

/// Shows a notice and waits until the system took it; a missing helper is an error.
pub fn show_toast(title: &str, body: &str) -> io::Result<()> {
    let title = clean(title, TITLE_MAX);
    let body = clean(body, BODY_MAX);
    let reply = platform_call(&title, &body)?;
    if reply.success {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the notification helper failed: {}",
            reply.stderr.trim()
        )))
    }
}

#[cfg(windows)]
fn platform_call(title: &str, body: &str) -> io::Result<command::Reply> {
    let args = ["-NoProfile", "-NonInteractive", "-Command", "-"].map(String::from);
    command::run(
        "powershell",
        &args,
        Some(windows_script(title, body).as_bytes()),
        LIMIT,
    )
}

#[cfg(target_os = "macos")]
fn platform_call(title: &str, body: &str) -> io::Result<command::Reply> {
    command::run("osascript", &macos_args(title, body), None, LIMIT)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_call(title: &str, body: &str) -> io::Result<command::Reply> {
    command::run("notify-send", &linux_args(title, body), None, LIMIT)
}

/// One line without control characters, cut to `max` characters.
fn clean(text: &str, max: usize) -> String {
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim();
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(max.saturating_sub(3)).collect();
    cut.push_str("...");
    cut
}

#[cfg(any(windows, test))]
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The PowerShell that shows the toast; escaping turns every quote of the texts into an entity, so a single-quoted string holds them.
#[cfg(any(windows, test))]
fn windows_script(title: &str, body: &str) -> String {
    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    );
    let xml = xml.replace('\'', "''");
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null\n\
         [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null\n\
         $xml = New-Object Windows.Data.Xml.Dom.XmlDocument\n\
         $xml.LoadXml('{xml}')\n\
         $toast = [Windows.UI.Notifications.ToastNotification]::new($xml)\n\
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{WINDOWS_APP_ID}').Show($toast)\n"
    )
}

#[cfg(any(not(any(windows, target_os = "macos")), test))]
fn linux_args(title: &str, body: &str) -> Vec<String> {
    ["--app-name=NULLRADIX", "--", title, body]
        .map(String::from)
        .to_vec()
}

#[cfg(any(target_os = "macos", test))]
fn macos_args(title: &str, body: &str) -> Vec<String> {
    let quote = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
    vec![
        String::from("-e"),
        format!(
            "display notification {} with title {}",
            quote(body),
            quote(title)
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_are_one_line_and_bounded() {
        assert_eq!(clean("a\nb\tc", 10), "a b c");
        let cut = clean(&"x".repeat(BODY_MAX + 1), BODY_MAX);
        assert_eq!(cut.chars().count(), BODY_MAX);
        assert!(cut.ends_with("..."));
        let fits = clean(&"y".repeat(BODY_MAX), BODY_MAX);
        assert_eq!(fits.chars().count(), BODY_MAX);
        assert!(!fits.ends_with("..."));
    }

    #[test]
    fn the_windows_script_escapes_xml_and_quotes() {
        let script = windows_script("It's <done>", "a & \"b\"");
        assert!(
            script.contains("<text>It&apos;s &lt;done&gt;</text>"),
            "{script}"
        );
        assert!(
            script.contains("<text>a &amp; &quot;b&quot;</text>"),
            "{script}"
        );
        assert!(script.contains(WINDOWS_APP_ID));
    }

    #[test]
    fn the_helper_arguments_keep_texts_apart_from_options() {
        assert_eq!(
            linux_args("-t", "body"),
            ["--app-name=NULLRADIX", "--", "-t", "body"]
        );
        let mac = macos_args("T \"x\"", "a\\b");
        assert_eq!(mac[0], "-e");
        assert_eq!(
            mac[1],
            "display notification \"a\\\\b\" with title \"T \\\"x\\\"\""
        );
    }
}
