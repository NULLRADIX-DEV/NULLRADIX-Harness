use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Whether a process with this id currently exists.
#[must_use]
pub fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        crate::windows::process_alive(pid)
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new("/proc").join(pid.to_string()).exists()
    }
}

/// Keeps a console window from flashing up for the child on Windows.
pub fn hide_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    command
}

/// Starts an outliving process with null stdio and no inherited handles, returning its id.
pub fn spawn_detached(program: &Path, args: &[OsString]) -> io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let pid = child.id();
        // reap it so it does not linger as a zombie while the caller lives
        std::thread::spawn(move || child.wait());
        Ok(pid)
    }
    #[cfg(windows)]
    {
        crate::windows::spawn_detached(program, args)
    }
}

/// A child process together with everything it starts, killed as one.
#[derive(Debug)]
pub struct ProcessTree {
    child: Child,
    #[cfg(windows)]
    job: Option<crate::windows::Job>,
}

impl ProcessTree {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        hide_console(command);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn()?;
        // without a job the kill falls back to taskkill
        #[cfg(windows)]
        let job = crate::windows::Job::assign(&child).ok();
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
        })
    }

    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Kills the whole tree; an already finished tree is not an error.
    pub fn kill(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            use nix::errno::Errno;
            use nix::sys::signal::{Signal, killpg};
            use nix::unistd::Pid;
            let group = i32::try_from(self.child.id())
                .map_err(|_| io::Error::other("process id out of range"))?;
            match killpg(Pid::from_raw(group), Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => {}
                Err(error) => return Err(io::Error::from(error)),
            }
        }
        #[cfg(windows)]
        {
            match &self.job {
                Some(job) => job.terminate()?,
                None => taskkill(self.child.id())?,
            }
        }
        match self.child.kill() {
            Err(error) if error.kind() != io::ErrorKind::InvalidInput => Err(error),
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
fn taskkill(pid: u32) -> io::Result<()> {
    let status =
        hide_console(Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
    // exit code 128 means the process is already gone
    if status.success() || status.code() == Some(128) {
        Ok(())
    } else {
        Err(io::Error::other(format!("taskkill failed with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    fn quick_exit() -> Command {
        if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit 0"]);
            command
        } else {
            Command::new("true")
        }
    }

    fn wait_for_file(path: &std::path::Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = fs::read_to_string(path)
                && !text.trim().is_empty()
            {
                return text.trim().to_owned();
            }
            assert!(Instant::now() < deadline, "grandchild did not report");
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// A child that starts a long-running grandchild and writes its pid to `pid_file`.
    fn parent_of_grandchild(pid_file: &std::path::Path) -> Command {
        if cfg!(windows) {
            let script = format!(
                "$p = Start-Process ping -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; \
                 Set-Content -Path '{}' -Value $p.Id; Wait-Process -Id $p.Id",
                pid_file.display()
            );
            let mut command = Command::new("powershell");
            command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args([
                "-c",
                &format!("sleep 60 & echo $! > '{}'; wait", pid_file.display()),
            ]);
            command
        }
    }

    #[test]
    fn the_current_process_is_alive() {
        assert!(process_alive(std::process::id()));
    }

    #[test]
    fn an_exited_process_is_not_alive() {
        let mut child = quick_exit().spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_alive(pid));
    }

    #[test]
    fn detached_processes_run_with_arguments_containing_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.txt");
        let (program, args) = if cfg!(windows) {
            let script = format!("echo two words>{}", out.display());
            ("cmd", ["/C", &script].map(OsString::from).to_vec())
        } else {
            let out = out.display().to_string();
            let script = r#"printf '%s' "$1" > "$2""#;
            let args = ["-c", script, "sh", "two words", &out].map(OsString::from);
            ("sh", args.to_vec())
        };
        spawn_detached(Path::new(program), &args).unwrap();
        assert_eq!(wait_for_file(&out), "two words");
    }

    #[test]
    fn killing_a_tree_also_kills_the_grandchild() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let mut tree = ProcessTree::spawn(&mut parent_of_grandchild(&pid_file)).unwrap();
        let grandchild: u32 = wait_for_file(&pid_file).parse().unwrap();
        assert!(process_alive(grandchild));
        tree.kill().unwrap();
        tree.child_mut().wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while process_alive(grandchild) {
            assert!(Instant::now() < deadline, "grandchild survived the kill");
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn killing_a_finished_tree_is_silent() {
        let mut tree = ProcessTree::spawn(&mut quick_exit()).unwrap();
        tree.child_mut().wait().unwrap();
        tree.kill().unwrap();
    }
}
