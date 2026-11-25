use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(20);

/// What a finished helper program said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Runs `program` with `input` on its stdin and kills it when it takes longer than `limit`.
pub(crate) fn run(
    program: &str,
    args: &[String],
    input: Option<&[u8]>,
    limit: Duration,
) -> io::Result<Reply> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take()
        && let Some(bytes) = input
    {
        // a program that exits early closes the pipe; its status tells why
        let _ = stdin.write_all(bytes);
    }
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{program} did not answer in time"),
            ));
        }
        thread::sleep(POLL);
    };
    let text = |reader: thread::JoinHandle<Vec<u8>>| {
        String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned()
    };
    Ok(Reply {
        success: status.success(),
        stdout: text(stdout),
        stderr: text(stderr),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn shell(script: &str) -> (&'static str, Vec<String>) {
        ("cmd", vec![String::from("/C"), script.to_owned()])
    }

    #[cfg(unix)]
    fn shell(script: &str) -> (&'static str, Vec<String>) {
        ("sh", vec![String::from("-c"), script.to_owned()])
    }

    #[test]
    fn collects_output_and_status() {
        #[cfg(windows)]
        let script = "echo out& echo err 1>&2& exit 3";
        #[cfg(unix)]
        let script = "echo out; echo err 1>&2; exit 3";
        let (program, args) = shell(script);
        let reply = run(program, &args, None, Duration::from_secs(10)).unwrap();
        assert!(!reply.success);
        assert_eq!(reply.stdout.trim(), "out");
        assert_eq!(reply.stderr.trim(), "err");
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let error = run(
            "nullradix-no-such-program",
            &[],
            None,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_program_that_never_ends_is_killed() {
        #[cfg(windows)]
        let (program, args) = (
            "ping",
            vec![
                String::from("-n"),
                String::from("30"),
                String::from("127.0.0.1"),
            ],
        );
        #[cfg(unix)]
        let (program, args) = ("sleep", vec![String::from("30")]);
        let started = Instant::now();
        let error = run(program, &args, None, Duration::from_millis(300)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
