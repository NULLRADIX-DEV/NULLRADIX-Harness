#![allow(unsafe_code, reason = "pre_exec and raw descriptors have no safe form")]

use std::collections::{HashMap, HashSet};
use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::libc;
use nix::sys::prctl;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, kill, raise, sigaction};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, pipe2};

const PASSES: usize = 20;
const PAUSE: Duration = Duration::from_millis(5);
/// How often the reaper looks for children to reap and signals to pass on, as the supervisor does.
const TICK: Duration = Duration::from_millis(100);
/// The descriptor on which the reaper reports whether the command started; the command lacks it.
const STATUS_VAR: &str = "NULLRADIX_REAPER_STATUS";
/// The name the reaper shows in process lists.
const REAPER_NAME: &str = "nullradix";
/// Signals the reaper passes on to its command; any other would end it before it sweeps.
const FORWARDED: [Signal; 7] = [
    Signal::SIGTERM,
    Signal::SIGINT,
    Signal::SIGHUP,
    Signal::SIGQUIT,
    Signal::SIGUSR1,
    Signal::SIGUSR2,
    Signal::SIGALRM,
];

/// Takes a pid and its start time; when the pipe closes it ends that tree if the pid still matches.
const GUARD: &str = r#"set -f
trap '' HUP INT
read -r main start || exit 0
case $main in ''|*[!0-9]*) exit 0 ;; esac
case $start in ''|*[!0-9]*) exit 0 ;; esac
while read -r note; do :; done
stat_line() {
  line=
  while read -r part || [ -n "$part" ]; do line="$line$part"; done < "$1" || return 1
  [ -n "$line" ]
}
members() {
  stat_line "/proc/$main/stat" || return 0
  set -- ${line##*) }
  [ "${20}" = "$start" ] || return 0
  pairs=
  set +f
  for stat in /proc/[0-9]*/stat; do
    set -f
    stat_line "$stat" || continue
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
    command
        .arg0(REAPER_NAME)
        .args(&shim.args)
        .arg("--")
        .arg(program);
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

/// The descriptor the holder named for the report: past the standard streams and open.
fn status_descriptor(text: Option<&str>) -> Option<OwnedFd> {
    let raw: i32 = text?.parse().ok().filter(|&raw| raw >= 3)?;
    // SAFETY: F_GETFD only asks whether the number is an open descriptor
    if unsafe { libc::fcntl(raw, libc::F_GETFD) } < 0 {
        return None;
    }
    // SAFETY: the holder passed this open descriptor on for the reaper alone
    Some(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// Lets no core file of the reaper itself come of a signal that ended the command.
fn forbid_core_dumps() {
    let none = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: setrlimit reads the struct it is given
    let _ = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const none) };
}

/// Runs `args` as the only child of a subreaper; orphans are reaped and leftovers die with it.
pub(crate) fn run_reaper(args: &[OsString]) -> i32 {
    if let Ok(name) = CString::new(REAPER_NAME) {
        let _ = prctl::set_name(&name);
    }
    let status = status_descriptor(std::env::var(STATUS_VAR).ok().as_deref());
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
    let handled = FORWARDED
        // SAFETY: the handler only stores into an atomic, so it is async-signal-safe
        .map(|signal| unsafe { sigaction(signal, &catch) });
    if handled.iter().any(Result::is_err) || prctl::set_child_subreaper(true).is_err() {
        tell("err 1");
        return 1;
    }
    // an own group, so a signal to the group of the reaper reaches the command once, through us
    let child = match Command::new(program)
        .args(rest)
        .env_remove(STATUS_VAR)
        .process_group(0)
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
        pass_on(main);
        thread::sleep(TICK);
    };
    sweep();
    match ended {
        WaitStatus::Signaled(_, signal, _) => {
            let default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
            // SAFETY: the default action is no handler code
            let _ = unsafe { sigaction(signal, &default) };
            forbid_core_dumps();
            let _ = raise(signal);
            128 + signal as i32
        }
        WaitStatus::Exited(_, code) => code,
        _ => 1,
    }
}

/// One bit per signal that arrived since the last pass.
static PENDING: AtomicU64 = AtomicU64::new(0);

extern "C" fn note(signal: i32) {
    PENDING.fetch_or(1 << (signal & 63), Ordering::SeqCst);
}

/// Sends the signals that arrived to the command, each once however often it came.
fn pass_on(main: u32) {
    let arrived = PENDING.swap(0, Ordering::SeqCst);
    let Some(raw) = to_pid(main) else { return };
    for signal in FORWARDED {
        if arrived & (1 << (signal as i32 & 63)) != 0 {
            let _ = kill(raw, signal);
        }
    }
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
    use std::os::fd::IntoRawFd;
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

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "{what} did not happen");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn lines_of(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn only_an_open_descriptor_past_the_standard_streams_is_taken_as_the_report() {
        assert!(status_descriptor(None).is_none());
        for text in ["", "x", "-1", "0", "1", "2", &i32::MAX.to_string()] {
            assert!(status_descriptor(Some(text)).is_none(), "{text:?}");
        }
        let (read, write) = pipe2(OFlag::O_CLOEXEC).unwrap();
        let raw = write.into_raw_fd();
        let taken = status_descriptor(Some(&raw.to_string())).unwrap();
        assert_eq!(taken.as_raw_fd(), raw);
        drop((taken, read));
    }

    #[test]
    fn the_command_does_not_see_the_report_variable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("seen");
        let text = format!(
            "printf '%s' \"${{{STATUS_VAR}-unset}}\" > '{file}'",
            file = file.display()
        );
        let mut tree = ProcessTree::spawn(&mut script(&text)).unwrap();
        assert!(tree.wait().unwrap().success());
        assert_eq!(fs::read_to_string(&file).unwrap(), "unset");
    }

    #[test]
    fn the_reaper_shows_as_nullradix() {
        let mut tree = ProcessTree::spawn(&mut script("sleep 60")).unwrap();
        let id = tree.id();
        let cmdline = fs::read(format!("/proc/{id}/cmdline")).unwrap();
        assert!(cmdline.starts_with(b"nullradix\0"), "{cmdline:?}");
        // a test runs the reaper on a thread of its own, so any thread may carry the name
        let names: Vec<String> = fs::read_dir(format!("/proc/{id}/task"))
            .unwrap()
            .filter_map(|task| fs::read_to_string(task.ok()?.path().join("comm")).ok())
            .map(|name| name.trim_end().to_owned())
            .collect();
        assert!(names.iter().any(|name| name == REAPER_NAME), "{names:?}");
        tree.kill().unwrap();
    }

    #[test]
    fn a_signal_to_the_group_of_the_reaper_reaches_the_command_once() {
        let dir = tempfile::tempdir().unwrap();
        let (log, ready) = (dir.path().join("log"), dir.path().join("ready"));
        let note = |signal: &str, mark: &str| {
            format!("trap 'echo {mark} >> \"{}\"' {signal}; ", log.display())
        };
        let text = format!(
            "{}{}{}echo > '{}'; n=0; while [ $n -lt 400 ]; do sleep 0.05; n=$((n+1)); done",
            note("TERM", "t"),
            note("USR1", "u"),
            note("USR2", "v"),
            ready.display()
        );
        let mut tree = ProcessTree::spawn(&mut script(&text)).unwrap();
        let reaper = to_pid(tree.id()).unwrap();
        wait_until("the command", || ready.exists());
        killpg(reaper, Signal::SIGTERM).unwrap();
        wait_until("the first TERM", || {
            lines_of(&log).contains(&String::from("t"))
        });
        // later signals are passed on in later passes, so a second TERM would show before them
        kill(reaper, Signal::SIGUSR1).unwrap();
        wait_until("USR1", || lines_of(&log).contains(&String::from("u")));
        kill(reaper, Signal::SIGUSR2).unwrap();
        wait_until("USR2", || lines_of(&log).contains(&String::from("v")));
        let seen = lines_of(&log);
        assert_eq!(
            seen.iter().filter(|line| *line == "t").count(),
            1,
            "{seen:?}"
        );
        tree.kill().unwrap();
    }

    #[test]
    fn every_signal_that_would_end_the_reaper_is_passed_on_so_it_still_sweeps() {
        for (name, signal) in [
            ("QUIT", Signal::SIGQUIT),
            ("USR1", Signal::SIGUSR1),
            ("USR2", Signal::SIGUSR2),
            ("ALRM", Signal::SIGALRM),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (orphan, ready) = (dir.path().join("orphan.pid"), dir.path().join("ready"));
            let text = format!(
                "trap 'exit 7' {name}; ( sleep 60 & echo $! > '{}' ); echo > '{}'; \
                 n=0; while [ $n -lt 400 ]; do sleep 0.05; n=$((n+1)); done",
                orphan.display(),
                ready.display()
            );
            let mut tree = ProcessTree::spawn(&mut script(&text)).unwrap();
            wait_until("the command", || ready.exists());
            let orphan = wait_for_pid(&orphan);
            kill(to_pid(tree.id()).unwrap(), signal).unwrap();
            let status = tree.wait().unwrap();
            assert_eq!((status.code(), status.signal()), (Some(7), None), "{name}");
            assert!(gone(orphan), "{name}: the orphan survived the sweep");
        }
    }

    #[test]
    fn a_command_that_dumps_core_ends_the_reaper_the_same_way() {
        ensure_reaper();
        let mut command = ProcessTree::command("sleep");
        command.arg("60");
        let mut tree = ProcessTree::spawn(&mut command).unwrap();
        let reaper = tree.id();
        let child = || {
            table()
                .unwrap()
                .iter()
                .find(|entry| entry.parent == reaper)
                .map(|entry| entry.id)
        };
        wait_until("the command", || child().is_some());
        kill(to_pid(child().unwrap()).unwrap(), Signal::SIGQUIT).unwrap();
        let status = tree.wait().unwrap();
        assert_eq!(status.signal(), Some(3), "{status:?}");
    }

    #[test]
    #[ignore = "runs as the helper of the test below"]
    fn core_limit_helper() {
        if std::env::var(STAND_IN).is_err() {
            return;
        }
        forbid_core_dumps();
        let mut limit = libc::rlimit {
            rlim_cur: 1,
            rlim_max: 1,
        };
        // SAFETY: getrlimit fills the struct it is given
        unsafe { libc::getrlimit(libc::RLIMIT_CORE, &raw mut limit) };
        println!("limit {} {}", limit.rlim_cur, limit.rlim_max);
    }

    #[test]
    fn forbidding_core_dumps_sets_both_limits_to_zero() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "linux::tests::core_limit_helper",
                "--ignored",
                "--nocapture",
            ])
            .env(STAND_IN, "core")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("limit 0 0"), "{text}");
    }

    /// The signals a process ignores, from its status file.
    fn ignored(id: u32) -> u64 {
        let text = fs::read_to_string(format!("/proc/{id}/status")).unwrap_or_default();
        text.lines()
            .find_map(|line| line.strip_prefix("SigIgn:"))
            .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
            .unwrap_or(0)
    }

    #[test]
    fn the_guard_ignores_a_hangup_and_still_ends_the_tree() {
        let mut other = Command::new("sleep").arg("60").spawn().unwrap();
        let mut guard = Guard::start().unwrap();
        let id = guard.child.as_ref().unwrap().id();
        // HUP is signal 1 and INT signal 2: the first two bits
        wait_until("the traps", || ignored(id) & 0b11 == 0b11);
        kill(to_pid(id).unwrap(), Signal::SIGHUP).unwrap();
        kill(to_pid(id).unwrap(), Signal::SIGINT).unwrap();
        guard.watch(other.id()).unwrap();
        let mut child = guard.child.take().unwrap();
        drop(guard.stdin.take());
        child.wait().unwrap();
        assert!(gone(other.id()), "the guard died of the hangup");
        let _ = other.kill();
        other.wait().unwrap();
    }

    #[test]
    fn a_newline_in_a_process_name_does_not_hide_it_from_the_guard() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        // the name of a script is the name its process shows, whatever it holds
        let odd = dir.path().join("odd\nname");
        fs::write(&odd, "#!/bin/sh\nsleep 60; true\n").unwrap();
        fs::set_permissions(&odd, fs::Permissions::from_mode(0o755)).unwrap();
        let pid_file = dir.path().join("odd.pid");
        let text = format!("\"$1\" & echo $! > '{}'; wait", pid_file.display());
        let mut main = Command::new("sh")
            .args(["-c", &text, "sh"])
            .arg(&odd)
            .spawn()
            .unwrap();
        let hidden = wait_for_pid(&pid_file);
        let comm = fs::read_to_string(format!("/proc/{hidden}/comm")).unwrap();
        assert!(comm.trim_end_matches('\n').contains('\n'), "{comm:?}");
        let leaf = || {
            table()
                .unwrap()
                .iter()
                .find(|entry| entry.parent == hidden)
                .map(|entry| entry.id)
        };
        wait_until("the sleep under the script", || leaf().is_some());
        let leaf = leaf().unwrap();
        let mut guard = Guard::start().unwrap();
        guard.watch(main.id()).unwrap();
        let mut child = guard.child.take().unwrap();
        drop(guard.stdin.take());
        child.wait().unwrap();
        assert!(gone(hidden), "the process with the newline survived");
        assert!(gone(leaf), "the child of that process survived");
        let _ = main.wait();
    }
}
