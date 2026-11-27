use std::ffi::OsString;
use std::io;
use std::path::Path;
#[cfg(unix)]
use std::process::Stdio;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};

/// Whether a process with this id currently exists.
#[must_use]
pub fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        crate::windows::process_alive(pid)
    }
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        use nix::sys::signal::kill;
        let Ok(raw) = i32::try_from(pid) else {
            return false;
        };
        if raw == 0 {
            return false;
        }
        // signal 0 only checks; a process owned by someone else still exists
        matches!(
            kill(nix::unistd::Pid::from_raw(raw), None),
            Ok(()) | Err(Errno::EPERM)
        )
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

/// Makes this process own the orphans below it, so its one tree ends with them; Linux only.
#[cfg_attr(not(target_os = "linux"), allow(clippy::unnecessary_wraps))]
pub fn become_reaper() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        crate::linux::become_reaper()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// A process with all it starts, ended with it and, on Windows and Linux, with its holder.
#[derive(Debug)]
pub struct ProcessTree {
    child: Child,
    // once reaped, the id of the main process may belong to an unrelated process
    reaped: bool,
    #[cfg(target_os = "linux")]
    guard: crate::linux::Guard,
    #[cfg(windows)]
    job: crate::windows::Job,
}

impl ProcessTree {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // macOS has no subreaper or parent-death signal, so only the group ends a tree there
            #[cfg(target_os = "linux")]
            let mut guard = {
                let guard = crate::linux::Guard::start()?;
                crate::linux::adopt_orphans(command);
                guard
            };
            #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
            let mut child = command.process_group(0).spawn()?;
            #[cfg(target_os = "linux")]
            if let Err(error) = guard.watch(child.id()) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            Ok(Self {
                child,
                reaped: false,
                #[cfg(target_os = "linux")]
                guard,
            })
        }
        #[cfg(windows)]
        {
            let (child, job) = crate::windows::Job::spawn(command)?;
            Ok(Self {
                child,
                reaped: false,
                job,
            })
        }
    }

    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// The exit status once the main process ended, after killing what it left running.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.main_exited(false)? {
            self.finish().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Waits for the main process, then kills what it left running.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.main_exited(true)?;
        self.finish()
    }

    /// Kills the whole tree without waiting; an already finished tree is not an error.
    pub fn kill(&mut self) -> io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        self.kill_rest()?;
        match self.child.kill() {
            Err(error) if error.kind() != io::ErrorKind::InvalidInput => Err(error),
            _ => Ok(()),
        }
    }

    fn finish(&mut self) -> io::Result<ExitStatus> {
        if !self.reaped {
            // stragglers die first; a group with live members keeps its id
            let _ = self.kill_rest();
            #[cfg(target_os = "linux")]
            self.guard.release();
        }
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }

    /// Whether the main process exited, without reaping it.
    #[cfg(target_os = "linux")]
    fn main_exited(&mut self, block: bool) -> io::Result<bool> {
        use nix::errno::Errno;
        use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
        if self.reaped {
            return Ok(true);
        }
        let mut flags = WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT;
        if !block {
            flags |= WaitPidFlag::WNOHANG;
        }
        loop {
            match waitid(Id::Pid(self.pid()?), flags) {
                Ok(WaitStatus::StillAlive) => return Ok(false),
                Ok(_) => return Ok(true),
                Err(Errno::EINTR) => {}
                Err(error) => return Err(io::Error::from(error)),
            }
        }
    }

    /// Whether the main process exited; waitid is missing here, so it is reaped.
    #[cfg(not(target_os = "linux"))]
    fn main_exited(&mut self, block: bool) -> io::Result<bool> {
        if block {
            self.child.wait()?;
            Ok(true)
        } else {
            Ok(self.child.try_wait()?.is_some())
        }
    }

    #[cfg(unix)]
    fn pid(&self) -> io::Result<nix::unistd::Pid> {
        let raw = i32::try_from(self.child.id())
            .map_err(|_| io::Error::other("process id out of range"))?;
        Ok(nix::unistd::Pid::from_raw(raw))
    }

    #[cfg(unix)]
    fn kill_rest(&self) -> io::Result<()> {
        use nix::errno::Errno;
        use nix::sys::signal::{Signal, killpg};
        #[cfg(target_os = "linux")]
        if crate::linux::is_reaper() {
            let spare: Vec<u32> = [Some(self.child.id()), self.guard.id()]
                .into_iter()
                .flatten()
                .collect();
            crate::linux::sweep(&spare);
        }
        match killpg(self.pid()?, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(io::Error::from(error)),
        }
    }

    #[cfg(windows)]
    fn kill_rest(&self) -> io::Result<()> {
        self.job.terminate()
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        if !self.reaped && self.kill().is_ok() {
            #[cfg(target_os = "linux")]
            self.guard.release();
            let _ = self.child.wait();
        }
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

    /// A child that starts a long-running grandchild, writes its pid and maybe waits for it.
    fn parent_of_grandchild(pid_file: &std::path::Path, wait: bool) -> Command {
        if cfg!(windows) {
            let mut script = format!(
                "$p = Start-Process ping -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; \
                 Set-Content -Path '{}' -Value $p.Id",
                pid_file.display()
            );
            if wait {
                script.push_str("; Wait-Process -Id $p.Id");
            }
            let mut command = Command::new("powershell");
            command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
            command
        } else {
            let mut script = format!("sleep 60 & echo $! > '{}'", pid_file.display());
            if wait {
                script.push_str("; wait");
            }
            let mut command = Command::new("sh");
            command.args(["-c", &script]);
            command
        }
    }

    fn assert_dies(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while process_alive(pid) {
            assert!(Instant::now() < deadline, "process {pid} survived");
            thread::sleep(Duration::from_millis(50));
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
        let mut tree = ProcessTree::spawn(&mut parent_of_grandchild(&pid_file, true)).unwrap();
        let grandchild: u32 = wait_for_file(&pid_file).parse().unwrap();
        assert!(process_alive(grandchild));
        tree.kill().unwrap();
        tree.wait().unwrap();
        assert_dies(grandchild);
    }

    #[test]
    fn waiting_ends_what_the_main_process_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let mut tree = ProcessTree::spawn(&mut parent_of_grandchild(&pid_file, false)).unwrap();
        assert!(tree.wait().unwrap().success());
        assert_dies(wait_for_file(&pid_file).parse().unwrap());
        tree.kill().unwrap();
    }

    #[test]
    fn dropping_a_running_tree_kills_it() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let tree = ProcessTree::spawn(&mut parent_of_grandchild(&pid_file, true)).unwrap();
        let grandchild: u32 = wait_for_file(&pid_file).parse().unwrap();
        let parent = tree.id();
        drop(tree);
        assert_dies(grandchild);
        assert_dies(parent);
    }

    #[test]
    fn killing_a_finished_tree_is_silent() {
        let mut tree = ProcessTree::spawn(&mut quick_exit()).unwrap();
        tree.wait().unwrap();
        tree.kill().unwrap();
    }
}
