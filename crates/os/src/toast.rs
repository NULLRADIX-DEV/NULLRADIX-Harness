//! A desktop notification: a Windows toast, `notify-send` on Linux, `osascript` on macOS.

use std::io;
use std::time::Duration;

use crate::command;

const LIMIT: Duration = Duration::from_secs(20);
const TITLE_MAX: usize = 100;
const BODY_MAX: usize = 400;
#[cfg(any(windows, test))]
const TITLE_VAR: &str = "NULLRADIX_TOAST_TITLE";
#[cfg(any(windows, test))]
const BODY_VAR: &str = "NULLRADIX_TOAST_BODY";

/// The script never holds the texts: it reads them from the environment and ends non-zero on failure.
/// The id is the one of Windows PowerShell, which every Windows install has registered for toasts.
#[cfg(any(windows, test))]
const WINDOWS_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
try {
  [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
  [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
  $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
  $xml.LoadXml('<toast><visual><binding template="ToastGeneric"><text/><text/></binding></visual></toast>')
  $texts = $xml.GetElementsByTagName('text')
  $texts.Item(0).InnerText = $env:NULLRADIX_TOAST_TITLE
  $texts.Item(1).InnerText = $env:NULLRADIX_TOAST_BODY
  $toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
  [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe').Show($toast)
} catch {
  exit 1
}
"#;

/// Shows a notice and waits until the system took it; a missing helper is an error.
pub fn show_toast(title: &str, body: &str) -> io::Result<()> {
    let title = clean(title, TITLE_MAX);
    let body = clean(body, BODY_MAX);
    check(&platform_call(&title, &body)?)
}

fn check(reply: &command::Reply) -> io::Result<()> {
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
    let args = windows_args();
    command::run_env(
        "powershell",
        &args,
        &[(TITLE_VAR, title), (BODY_VAR, body)],
        None,
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

/// Base64 of the UTF-16 little-endian text, the form `-EncodedCommand` takes.
#[cfg(any(windows, test))]
fn encoded_command(script: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let [a, b, c] = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let group = (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c);
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[((group >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(any(windows, test))]
fn windows_args() -> Vec<String> {
    vec![
        String::from("-NoProfile"),
        String::from("-NonInteractive"),
        String::from("-EncodedCommand"),
        encoded_command(WINDOWS_SCRIPT),
    ]
}

/// `notify-send` reads the body as markup.
#[cfg(any(not(any(windows, target_os = "macos")), test))]
fn markup_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(any(not(any(windows, target_os = "macos")), test))]
fn linux_args(title: &str, body: &str) -> Vec<String> {
    [
        "--app-name=NULLRADIX",
        "--",
        title,
        markup_escape(body).as_str(),
    ]
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

    const HOSTILE: &[&str] = &[
        "x\u{2019}+(calc)+\u{2019}",
        "\u{2018}a\u{201A}b\u{201B}",
        "`$(calc)` \"q\" 'q' $env:PATH",
        "</text><text>x</text>",
        "umlaute \u{e4}\u{f6}\u{fc} \u{1F600}",
    ];

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
    fn the_windows_script_is_fixed_ascii_without_any_text() {
        assert!(WINDOWS_SCRIPT.is_ascii());
        // the same script whatever the notice says, so no text can reach the script source
        let args = windows_args();
        assert_eq!(args[2], "-EncodedCommand");
        assert_eq!(args[3], encoded_command(WINDOWS_SCRIPT));
        assert!(WINDOWS_SCRIPT.contains(&format!("$env:{TITLE_VAR}")));
        assert!(WINDOWS_SCRIPT.contains(&format!("$env:{BODY_VAR}")));
        for text in HOSTILE {
            assert!(!WINDOWS_SCRIPT.contains(text));
            assert!(args.iter().all(|arg| !arg.contains(text)));
        }
        // a failing toast must end the process non-zero
        assert!(WINDOWS_SCRIPT.contains("catch {\n  exit 1\n}"));
    }

    #[test]
    fn the_command_is_utf16_base64() {
        assert_eq!(encoded_command("a"), "YQA=");
        assert_eq!(encoded_command("ab"), "YQBiAA==");
        assert_eq!(encoded_command("abc"), "YQBiAGMA");
        assert_eq!(encoded_command("\u{e4}"), "5AA=");
    }

    #[test]
    fn a_failing_helper_is_an_error_with_its_message() {
        let failed = command::Reply {
            success: false,
            stdout: String::new(),
            stderr: String::from(" boom \n"),
        };
        let error = check(&failed).unwrap_err();
        assert!(error.to_string().contains("boom"), "{error}");
        let fine = command::Reply {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(check(&fine).is_ok());
    }

    #[test]
    fn the_helper_arguments_keep_texts_apart_from_options() {
        assert_eq!(
            linux_args("-t", "a <b> & c"),
            ["--app-name=NULLRADIX", "--", "-t", "a &lt;b&gt; &amp; c"]
        );
        let mac = macos_args("T \"x\"", "a\\b");
        assert_eq!(mac[0], "-e");
        assert_eq!(
            mac[1],
            "display notification \"a\\\\b\" with title \"T \\\"x\\\"\""
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "shows a real toast on this desktop"]
    fn a_real_toast_with_hostile_text_shows_and_runs_no_code() {
        let marker = std::env::temp_dir().join("nullradix-toast-injection");
        let _ = std::fs::remove_file(&marker);
        let title = format!(
            "x\u{2019}+(New-Item -Path '{}' -ItemType File)+\u{2019}",
            marker.display()
        );
        show_toast(&title, "umlaute \u{e4}\u{f6}\u{fc}").unwrap();
        assert!(!marker.exists());
    }
}
