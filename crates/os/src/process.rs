use std::ffi::OsString;
use std::io;
use std::path::Path;
#[cfg(unix)]
use std::process::Stdio;
use std::process::{Child, Command};

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

/// Starts an outliving process with null stdio and no inherited handles, returning its id.
pub fn spawn_detached(program: &Path, args: &[OsString], cwd: &Path) -> io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
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
        crate::windows::spawn_detached(program, args, cwd)
    }
}

/// A child process together with everything it starts, killed as one.
#[derive(Debug)]
pub struct ProcessTree {
    child: Child,
    #[cfg(windows)]
    job: crate::windows::Job,
}

impl ProcessTree {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let child = command.process_group(0).spawn()?;
            Ok(Self { child })
        }
        #[cfg(windows)]
        {
            let (child, job) = crate::windows::Job::spawn(command)?;
            Ok(Self { child, job })
        }
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
        self.job.terminate()?;
        match self.child.kill() {
            Err(error) if error.kind() != io::ErrorKind::InvalidInput => Err(error),
            _ => Ok(()),
        }
    }
}

impl Drop for ProcessTree {
    /// Whatever the tree left running dies with it, on every platform alike.
    fn drop(&mut self) {
        let _ = self.kill();
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
    fn detached_processes_run_in_their_directory_with_intact_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let (program, args) = if cfg!(windows) {
            (
                "cmd",
                ["/C", "echo two words>out.txt"]
                    .map(OsString::from)
                    .to_vec(),
            )
        } else {
            let script = r#"printf '%s' "$1" > out.txt"#;
            let args = ["-c", script, "sh", "two words"].map(OsString::from);
            ("sh", args.to_vec())
        };
        spawn_detached(Path::new(program), &args, dir.path()).unwrap();
        assert_eq!(wait_for_file(&dir.path().join("out.txt")), "two words");
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
    fn dropping_a_tree_kills_what_it_left_running() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let tree = ProcessTree::spawn(&mut parent_of_grandchild(&pid_file)).unwrap();
        let grandchild: u32 = wait_for_file(&pid_file).parse().unwrap();
        drop(tree);
        let deadline = Instant::now() + Duration::from_secs(10);
        while process_alive(grandchild) {
            assert!(Instant::now() < deadline, "grandchild survived the drop");
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
