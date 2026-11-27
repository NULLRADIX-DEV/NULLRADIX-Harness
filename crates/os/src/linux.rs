#![allow(unsafe_code, reason = "pre_exec and raw descriptors have no safe form")]

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::sys::prctl;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, kill, raise, sigaction};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, pipe2};

const PASSES: usize = 20;
const PAUSE: Duration = Duration::from_millis(5);
/// How often the reaper looks for children to reap and signals to pass on.
const TICK: Duration = Duration::from_millis(20);
/// The descriptor on which the reaper reports whether the command started; the command lacks it.
const STATUS_VAR: &str = "NULLRADIX_REAPER_STATUS";

/// Takes a pid and its start time; when the pipe closes it ends that tree if the pid still matches.
const GUARD: &str = r#"trap '' HUP INT
read -r main start || exit 0
case $main in ''|*[!0-9]*) exit 0 ;; esac
case $start in ''|*[!0-9]*) exit 0 ;; esac
while read -r note; do :; done
members() {
  line=
  read -r line < "/proc/$main/stat" || return 0
  set -- ${line##*) }
  [ "${20}" = "$start" ] || return 0
  pairs=
  for stat in /proc/[0-9]*/stat; do
    line=
    read -r line < "$stat" || continue
    set -- ${line##*) }
    id=${stat#/proc/}
    pairs="$pairs ${id%/stat}:$2"
  done
  found=$main
  grew=1
  while [ "$grew" = 1 ]; do
    grew=0
    for pair in $pairs; do
      case " $found " in
        *" ${pair%%:*} "*) ;;
        *" ${pair##*:} "*) found="$found ${pair%%:*}"; grew=1 ;;
      esac
    done
  done
  echo $found
}
before=
while :; do
  now=$(members)
  [ -n "$now" ] || exit 0
  [ "$now" = "$before" ] && break
  kill -STOP $now 2>/dev/null
  before=$now
done
tries=0
while [ "$tries" -lt 20 ]; do
  kill -KILL $now 2>/dev/null
  tries=$((tries + 1))
  now=$(members)
  [ -z "$now" ] && exit 0
  [ "$now" = "$main" ] && exit 0
done
"#;

struct Shim {
    program: PathBuf,
    args: Vec<OsString>,
}

static SHIM: OnceLock<Shim> = OnceLock::new();

/// Runs every command made by `reaped` through `program args -- <command>`.
pub(crate) fn use_reaper(program: PathBuf, args: Vec<OsString>) {
    let _ = SHIM.set(Shim { program, args });
}

/// A command that runs `program` under the reaper; none while no reaper is registered.
pub(crate) fn reaped(program: &OsStr) -> Option<Command> {
    let shim = SHIM.get()?;
    let mut command = Command::new(&shim.program);
    command.args(&shim.args).arg("--").arg(program);
    Some(command)
}

fn is_reaped(command: &Command) -> bool {
    let Some(shim) = SHIM.get() else { return false };
    let mut args = command.get_args();
    command.get_program() == shim.program
        && shim.args.iter().all(|arg| args.next() == Some(arg))
        && args.next() == Some(OsStr::new("--"))
}

struct Entry {
    id: u32,
    parent: u32,
    zombie: bool,
}

/// One line of `/proc/<pid>/stat`: the command name may hold spaces and parentheses.
fn parse_stat(id: u32, text: &str) -> Option<Entry> {
    let rest = text.rsplit_once(") ")?.1;
    let mut fields = rest.split_ascii_whitespace();
    let state = fields.next()?;
    let parent = fields.next()?.parse().ok()?;
    Some(Entry {
        id,
        parent,
        zombie: state == "Z",
    })
}

/// The start time in clock ticks since boot, which tells a process from a later owner of its id.
fn start_time(id: u32) -> Option<u64> {
    let text = fs::read_to_string(format!("/proc/{id}/stat")).ok()?;
    text.rsplit_once(") ")?
        .1
        .split_ascii_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn table() -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir("/proc")? {
        let Ok(item) = item else { continue };
        let Some(id) = item.file_name().to_str().and_then(|name| name.parse().ok()) else {
            continue;
        };
        if let Ok(text) = fs::read_to_string(format!("/proc/{id}/stat"))
            && let Some(entry) = parse_stat(id, &text)
        {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// The live processes below `root`, found through their parents.
fn below(table: &[Entry], root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<&Entry>> = HashMap::new();
    for entry in table {
        children.entry(entry.parent).or_default().push(entry);
    }
    let mut seen = HashSet::from([root]);
    let mut queue = vec![root];
    let mut found = Vec::new();
    while let Some(parent) = queue.pop() {
        for child in children.get(&parent).into_iter().flatten() {
            if !seen.insert(child.id) {
                continue;
            }
            queue.push(child.id);
            if !child.zombie {
                found.push(child.id);
            }
        }
    }
    found
}

fn to_pid(raw: u32) -> Option<Pid> {
    i32::try_from(raw).ok().map(Pid::from_raw)
}

fn signal_all(ids: &[u32], signal: Signal) {
    for raw in ids.iter().filter_map(|&raw| to_pid(raw)) {
        let _ = kill(raw, signal);
    }
}

/// Freezes `root` and everything below it, then kills them; a frozen process cannot fork away.
pub(crate) fn kill_tree(root: u32) {
    let members = |table: &[Entry]| {
        let mut found = below(table, root);
        if table.iter().any(|entry| entry.id == root && !entry.zombie) {
            found.push(root);
        }
        found.sort_unstable();
        found
    };
    let mut frozen = Vec::new();
    for _ in 0..PASSES {
        let Ok(table) = table() else { break };
        let now = members(&table);
        if now.is_empty() || now == frozen {
            break;
        }
        signal_all(&now, Signal::SIGSTOP);
        frozen = now;
    }
    for _ in 0..PASSES {
        let Ok(table) = table() else { return };
        let now = members(&table);
        if now.is_empty() {
            return;
        }
        signal_all(&now, Signal::SIGKILL);
        thread::sleep(PAUSE);
    }
}

/// Kills everything below this subreaper and reaps the dead; orphans of the killed die next pass.
fn sweep() {
    let me = std::process::id();
    for _ in 0..PASSES {
        let Ok(table) = table() else { return };
        for entry in table
            .iter()
            .filter(|entry| entry.parent == me && entry.zombie)
        {
            // only reaps what is already dead
            let _ = to_pid(entry.id).map(|raw| waitpid(raw, Some(WaitPidFlag::WNOHANG)));
        }
        let live = below(&table, me);
        if live.is_empty() {
            return;
        }
        signal_all(&live, Signal::SIGKILL);
        thread::sleep(PAUSE);
    }
}

/// How a reaper tells its holder that the command started, or why it did not.
pub(crate) struct Report {
    read: fs::File,
    write: OwnedFd,
}

/// Opens the channel on which the reaper of `command` reports; none for a plain command.
pub(crate) fn prepare(command: &mut Command) -> io::Result<Option<Report>> {
    if !is_reaped(command) {
        return Ok(None);
    }
    let (read, write) = pipe2(OFlag::O_CLOEXEC).map_err(io::Error::from)?;
    let raw = write.as_raw_fd();
    if raw < 3 {
        return Err(io::Error::other("the standard streams are closed"));
    }
    command.env(STATUS_VAR, raw.to_string());
    // SAFETY: between fork and exec the closure only calls fcntl, which is async-signal-safe
    unsafe {
        command.pre_exec(move || {
            let fd = BorrowedFd::borrow_raw(raw);
            fcntl(fd, FcntlArg::F_SETFD(FdFlag::empty()))
                .map(drop)
                .map_err(io::Error::from)
        });
    }
    Ok(Some(Report {
        read: fs::File::from(read),
        write,
    }))
}

/// A reaper that cannot be started is no missing program.
pub(crate) fn reaper_failed(error: &io::Error) -> io::Error {
    io::Error::other(format!("cannot start the run reaper: {error}"))
}

impl Report {
    /// What the reaper reported; nothing means it ended before starting the command.
    pub(crate) fn outcome(self) -> io::Result<()> {
        drop(self.write);
        let mut text = String::new();
        let mut read = self.read;
        read.read_to_string(&mut text)?;
        let text = text.trim();
        if text == "ok" {
            return Ok(());
        }
        match text.strip_prefix("err ") {
            Some(code) => Err(code.parse().map_or_else(
                |_| io::Error::other("the run reaper reported an unreadable error"),
                io::Error::from_raw_os_error,
            )),
            None => Err(io::Error::other(
                "the run reaper ended before it started the command",
            )),
        }
    }
}

/// Runs `args` as the only child of a subreaper; orphans are reaped and leftovers die with it.
pub(crate) fn run_reaper(args: &[OsString]) -> i32 {
    let status = std::env::var(STATUS_VAR)
        .ok()
        .and_then(|text| text.parse::<i32>().ok())
        // SAFETY: the holder passed this descriptor on for the reaper alone
        .map(|raw| unsafe { OwnedFd::from_raw_fd(raw) });
    if let Some(fd) = &status {
        let _ = fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC));
    }
    let tell = |text: &str| {
        if let Some(fd) = status.as_ref().and_then(|fd| fd.try_clone().ok()) {
            let _ = fs::File::from(fd).write_all(text.as_bytes());
        }
    };
    let Some((program, rest)) = args.split_first() else {
        tell("err 22");
        return 2;
    };
    let catch = SigAction::new(SigHandler::Handler(note), SaFlags::empty(), SigSet::empty());
    let handled = [Signal::SIGTERM, Signal::SIGINT, Signal::SIGHUP]
        // SAFETY: the handler only stores into an atomic, so it is async-signal-safe
        .map(|signal| unsafe { sigaction(signal, &catch) });
    if handled.iter().any(Result::is_err) || prctl::set_child_subreaper(true).is_err() {
        tell("err 1");
        return 1;
    }
    let child = match Command::new(program)
        .args(rest)
        .env_remove(STATUS_VAR)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tell(&format!("err {}", error.raw_os_error().unwrap_or(0)));
            return if error.kind() == io::ErrorKind::NotFound {
                127
            } else {
                126
            };
        }
    };
    tell("ok");
    drop(status);
    let main = child.id();
    let ended = loop {
        if let Some(ended) = reap_ready(main) {
            break ended;
        }
        let pending = SIGNALED.swap(0, Ordering::SeqCst);
        if let (Ok(signal), Some(raw)) = (Signal::try_from(pending), to_pid(main)) {
            let _ = kill(raw, signal);
        }
        thread::sleep(TICK);
    };
    sweep();
    match ended {
        WaitStatus::Signaled(_, signal, _) => {
            let default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
            // SAFETY: the default action is no handler code
            let _ = unsafe { sigaction(signal, &default) };
            let _ = raise(signal);
            128 + signal as i32
        }
        WaitStatus::Exited(_, code) => code,
        _ => 1,
    }
}

static SIGNALED: AtomicI32 = AtomicI32::new(0);

extern "C" fn note(signal: i32) {
    SIGNALED.store(signal, Ordering::SeqCst);
}

/// Reaps every child that ended; the status of `main` once it is among them.
fn reap_ready(main: u32) -> Option<WaitStatus> {
    loop {
        match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::StillAlive) => return None,
            Ok(status) if is_pid(&status, main) => return Some(status),
            Ok(_) | Err(Errno::EINTR) => {}
            Err(_) => return Some(WaitStatus::Exited(Pid::from_raw(0), 1)),
        }
    }
}

fn is_pid(status: &WaitStatus, id: u32) -> bool {
    status
        .pid()
        .and_then(|pid| u32::try_from(pid.as_raw()).ok())
        == Some(id)
}

/// A shell that outlives the supervisor and ends the tree if the supervisor dies unreleased.
#[derive(Debug)]
pub(crate) struct Guard {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

impl Guard {
    pub(crate) fn start() -> io::Result<Self> {
        // an own group, so a signal to the group of the holder does not reach it
        let mut child = Command::new("/bin/sh")
            .args(["-c", GUARD])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|error| io::Error::other(format!("cannot start the run guard: {error}")))?;
        let stdin = child.stdin.take();
        Ok(Self {
            child: Some(child),
            stdin,
        })
    }

    /// Names the process whose tree the guard ends from now on.
    pub(crate) fn watch(&mut self, main: u32) -> io::Result<()> {
        let started =
            start_time(main).ok_or_else(|| io::Error::other("the main process is gone"))?;
        self.watch_since(main, &started.to_string())
    }

    fn watch_since(&mut self, main: u32, started: &str) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("the guard is released"))?;
        writeln!(stdin, "{main} {started}")?;
        stdin.flush()
    }

    /// The tree is over; the guard is stopped, whatever it is doing.
    pub(crate) fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(self.stdin.take());
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // a closed pipe arms the guard, which ends the tree, so only its exit is collected
        drop(self.stdin.take());
        if let Some(mut child) = self.child.take() {
            thread::spawn(move || child.wait());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::path::Path;
    use std::sync::Once;
    use std::time::Instant;

    use nix::sys::signal::killpg;

    use super::*;
    use crate::{ProcessTree, write_atomic};

    const STAND_IN: &str = "NULLRADIX_TEST_STAND_IN";

    fn entry(id: u32, parent: u32, zombie: bool) -> Entry {
        Entry { id, parent, zombie }
    }

    /// Runs the reaper as this test binary, asked for the helper test below.
    fn ensure_reaper() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let args = [
                "--exact",
                "linux::tests::reaper_helper",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ];
            let program = std::env::current_exe().unwrap();
            use_reaper(program, args.map(OsString::from).to_vec());
        });
    }

    #[test]
    #[ignore = "runs as the reaper of the tests below"]
    fn reaper_helper() {
        let args: Vec<OsString> = std::env::args_os()
            .skip_while(|arg| arg != "--")
            .skip(1)
            .collect();
        if !args.is_empty() {
            std::process::exit(run_reaper(&args));
        }
    }

    fn script(text: &str) -> Command {
        ensure_reaper();
        let mut command = ProcessTree::command("sh");
        command.args(["-c", text]);
        command
    }

    #[test]
    fn stat_lines_parse_whatever_the_command_is_called() {
        let line = "42 (a) b (c) S 7 42 42 0 -1";
        let parsed = parse_stat(42, line).unwrap();
        assert_eq!((parsed.parent, parsed.zombie), (7, false));
        let parsed = parse_stat(9, "9 (sleep) Z 3 9 9 0 -1").unwrap();
        assert_eq!((parsed.parent, parsed.zombie), (3, true));
        assert!(parse_stat(1, "garbage").is_none());
    }

    #[test]
    fn the_live_descendants_follow_the_parents() {
        let table = [
            entry(10, 1, false),
            entry(11, 10, false),
            entry(12, 11, false),
            entry(13, 10, true),
            entry(14, 1, false),
            entry(20, 2, false),
        ];
        let mut found = below(&table, 10);
        found.sort_unstable();
        assert_eq!(found, [11, 12]);
        assert_eq!(below(&table, 99), Vec::<u32>::new());
    }

    #[test]
    fn the_exit_status_passes_through_the_reaper() {
        let mut tree = ProcessTree::spawn(&mut script("exit 3")).unwrap();
        assert_eq!(tree.wait().unwrap().code(), Some(3));
        let mut tree = ProcessTree::spawn(&mut script("kill -KILL $$")).unwrap();
        let status = tree.wait().unwrap();
        assert_eq!((status.code(), status.signal()), (None, Some(9)));
    }

    #[test]
    fn a_missing_program_fails_the_start_as_it_does_without_a_reaper() {
        ensure_reaper();
        let mut command = ProcessTree::command("/nonexistent/program");
        let error = ProcessTree::spawn(&mut command).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(error.raw_os_error(), Some(2));
    }

    #[test]
    fn termination_reaches_the_command_and_ends_the_reaper_the_same_way() {
        let mut tree = ProcessTree::spawn(&mut script("sleep 60")).unwrap();
        kill(to_pid(tree.id()).unwrap(), Signal::SIGTERM).unwrap();
        assert_eq!(tree.wait().unwrap().signal(), Some(15));
    }

    #[test]
    fn an_orphan_that_exits_is_reaped_while_the_run_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("orphan.pid");
        let text = format!(
            "( sh -c 'echo $$ > \"{}\"; exit 0' & ); sleep 60",
            file.display()
        );
        let mut tree = ProcessTree::spawn(&mut script(&text)).unwrap();
        let orphan = wait_for_pid(&file);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Path::new(&format!("/proc/{orphan}")).exists() {
            assert!(Instant::now() < deadline, "the orphan {orphan} is a zombie");
            thread::sleep(Duration::from_millis(50));
        }
        let ended = tree.try_wait().unwrap();
        assert!(ended.is_none(), "{ended:?}");
    }

    #[test]
    fn the_guard_ends_only_the_process_it_was_given() {
        let mut other = Command::new("sleep").arg("60").spawn().unwrap();
        let id = other.id();
        let mut guard = Guard::start().unwrap();
        guard.watch_since(id, "1").unwrap();
        let mut child = guard.child.take().unwrap();
        drop(guard.stdin.take());
        child.wait().unwrap();
        assert!(!gone(id), "a reused id was killed");
        let mut guard = Guard::start().unwrap();
        guard.watch(id).unwrap();
        let mut child = guard.child.take().unwrap();
        drop(guard.stdin.take());
        child.wait().unwrap();
        assert!(gone(id), "the watched process survived");
        let _ = other.kill();
        other.wait().unwrap();
    }

    /// A main process with a child that left its group and one that was orphaned.
    fn leaving_script(dir: &Path, wait: bool) -> String {
        let (direct, daemon) = (dir.join("direct.pid"), dir.join("daemon.pid"));
        let tail = if wait {
            String::from("wait")
        } else {
            format!(
                "while [ ! -s '{}' ] || [ ! -s '{}' ]; do sleep 0.05; done",
                direct.display(),
                daemon.display()
            )
        };
        format!(
            "setsid sh -c 'echo $$ > \"{direct}\"; exec sleep 60' & \
             ( setsid sh -c 'echo $$ > \"{daemon}\"; exec sleep 60' & ); {tail}",
            direct = direct.display(),
            daemon = daemon.display(),
        )
    }

    #[test]
    #[ignore = "runs as the supervisor of the tests below"]
    fn supervisor_stand_in() {
        let Ok(spec) = std::env::var(STAND_IN) else {
            return;
        };
        let (mode, dir) = spec.split_once(':').unwrap();
        let dir = Path::new(dir);
        let waits = mode == "wait";
        let mut tree = ProcessTree::spawn(&mut script(&leaving_script(dir, waits))).unwrap();
        write_atomic(&dir.join("main.pid"), tree.id().to_string().as_bytes()).unwrap();
        if waits {
            loop {
                thread::park();
            }
        }
        assert!(tree.wait().unwrap().success());
        write_atomic(&dir.join("done"), b"").unwrap();
    }

    /// The stand-in leads a group of its own, which a test may kill as a whole.
    fn start_stand_in(mode: &str, dir: &Path) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "linux::tests::supervisor_stand_in",
                "--ignored",
                "--nocapture",
            ])
            .env(STAND_IN, format!("{mode}:{}", dir.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap()
    }

    fn wait_for_pid(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "{} was not written",
                path.display()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// A process that is dead but not yet collected by its parent counts as gone.
    fn gone(id: u32) -> bool {
        fs::read_to_string(format!("/proc/{id}/stat"))
            .ok()
            .and_then(|text| parse_stat(id, &text))
            .is_none_or(|entry| entry.zombie)
    }

    /// Kills what a failing test would leave running.
    struct Pids(Vec<u32>);

    impl Drop for Pids {
        fn drop(&mut self) {
            signal_all(&self.0, Signal::SIGKILL);
        }
    }

    fn tree_pids(dir: &Path) -> Pids {
        Pids(
            ["main", "direct", "daemon"]
                .map(|name| wait_for_pid(&dir.join(format!("{name}.pid"))))
                .to_vec(),
        )
    }

    fn assert_all_gone(pids: &Pids) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Some(alive) = pids.0.iter().find(|&&raw| !gone(raw)) {
            assert!(Instant::now() < deadline, "process {alive} survived");
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_killed_supervisor_takes_its_whole_tree_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut stand_in = start_stand_in("wait", dir.path());
        let pids = tree_pids(dir.path());
        assert!(pids.0.iter().all(|&raw| !gone(raw)));
        stand_in.kill().unwrap();
        stand_in.wait().unwrap();
        assert_all_gone(&pids);
    }

    #[test]
    fn a_signal_to_the_group_of_the_supervisor_does_not_spare_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let mut stand_in = start_stand_in("wait", dir.path());
        let pids = tree_pids(dir.path());
        killpg(to_pid(stand_in.id()).unwrap(), Signal::SIGKILL).unwrap();
        stand_in.wait().unwrap();
        assert_all_gone(&pids);
    }

    #[test]
    fn leftovers_that_left_the_group_die_with_the_main_process() {
        let dir = tempfile::tempdir().unwrap();
        let mut stand_in = start_stand_in("finish", dir.path());
        let pids = tree_pids(dir.path());
        assert!(stand_in.wait().unwrap().success());
        assert!(dir.path().join("done").exists());
        assert_all_gone(&pids);
    }
}
