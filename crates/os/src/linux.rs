#![allow(unsafe_code, reason = "pre_exec has no safe form")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::Duration;

use nix::sys::prctl;
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, waitpid};
use nix::unistd::Pid;

const SWEEP_PASSES: usize = 20;
const SWEEP_PAUSE: Duration = Duration::from_millis(5);

/// Reads the main pid; "done" ends it quietly, a closed pipe freezes and kills the tree.
const GUARD: &str = r#"read -r main || exit 0
case $main in ''|*[!0-9]*) exit 0 ;; esac
read -r note && exit 0
members() {
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
  [ "$now" = "$before" ] && break
  kill -STOP $now 2>/dev/null
  before=$now
done
kill -KILL $now 2>/dev/null
"#;

/// Makes the child a subreaper, so what its descendants leave behind stays under it.
pub(crate) fn adopt_orphans(command: &mut Command) {
    // SAFETY: between fork and exec the closure only calls prctl, which is async-signal-safe
    unsafe { command.pre_exec(|| prctl::set_child_subreaper(true).map_err(io::Error::from)) };
}

/// Makes this process the parent of every orphan below it.
pub(crate) fn become_reaper() -> io::Result<()> {
    prctl::set_child_subreaper(true).map_err(io::Error::from)
}

/// Whether this process is a subreaper and so owns one tree.
pub(crate) fn is_reaper() -> bool {
    prctl::get_child_subreaper().unwrap_or(false)
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

/// Live processes below `root` but `spare`; the descendants of the spared still count.
fn victims(table: &[Entry], root: u32, spare: &[u32]) -> Vec<u32> {
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
            if !child.zombie && !spare.contains(&child.id) {
                found.push(child.id);
            }
        }
    }
    found
}

fn to_pid(raw: u32) -> Option<Pid> {
    i32::try_from(raw).ok().map(Pid::from_raw)
}

/// Kills everything below this subreaper but `spare`; orphans of the killed die on a later pass.
pub(crate) fn sweep(spare: &[u32]) {
    let me = std::process::id();
    for _ in 0..SWEEP_PASSES {
        let Ok(table) = table() else { return };
        for entry in &table {
            if entry.parent == me && entry.zombie && !spare.contains(&entry.id) {
                // only reaps what is already dead
                let _ = to_pid(entry.id).map(|raw| waitpid(raw, Some(WaitPidFlag::WNOHANG)));
            }
        }
        let live = victims(&table, me, spare);
        if live.is_empty() {
            return;
        }
        for victim in live {
            if let Some(raw) = to_pid(victim) {
                let _ = kill(raw, Signal::SIGKILL);
            }
        }
        thread::sleep(SWEEP_PAUSE);
    }
}

/// A shell that outlives the supervisor and ends the tree if the supervisor dies unreleased.
#[derive(Debug)]
pub(crate) struct Guard {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

impl Guard {
    pub(crate) fn start() -> io::Result<Self> {
        let mut child = Command::new("/bin/sh")
            .args(["-c", GUARD])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take();
        Ok(Self {
            child: Some(child),
            stdin,
        })
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Names the process whose tree the guard ends from now on.
    pub(crate) fn watch(&mut self, main: u32) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("the guard is released"))?;
        writeln!(stdin, "{main}")?;
        stdin.flush()
    }

    /// The tree is over; the guard exits without touching anything.
    pub(crate) fn release(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = writeln!(stdin, "done");
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
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
    use std::path::Path;
    use std::time::Instant;

    use super::*;
    use crate::{ProcessTree, write_atomic};

    const STAND_IN: &str = "NULLRADIX_TEST_STAND_IN";

    fn entry(id: u32, parent: u32, zombie: bool) -> Entry {
        Entry { id, parent, zombie }
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
    fn victims_are_the_live_descendants_without_the_spared() {
        let table = [
            entry(10, 1, false),
            entry(11, 10, false),
            entry(12, 11, false),
            entry(13, 10, true),
            entry(14, 1, false),
            entry(20, 2, false),
        ];
        let mut found = victims(&table, 10, &[]);
        found.sort_unstable();
        assert_eq!(found, [11, 12]);
        assert_eq!(victims(&table, 10, &[11]), [12]);
        assert_eq!(victims(&table, 99, &[]), Vec::<u32>::new());
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
            "setsid sh -c 'echo $$ > \"{direct}\"; exec sleep 60' &              ( setsid sh -c 'echo $$ > \"{daemon}\"; exec sleep 60' & ); {tail}",
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
        become_reaper().unwrap();
        let waits = mode == "wait";
        let mut command = Command::new("sh");
        command.args(["-c", &leaving_script(dir, waits)]);
        let mut tree = ProcessTree::spawn(&mut command).unwrap();
        write_atomic(&dir.join("main.pid"), tree.id().to_string().as_bytes()).unwrap();
        if waits {
            loop {
                thread::park();
            }
        }
        assert!(tree.wait().unwrap().success());
        write_atomic(&dir.join("done"), b"").unwrap();
    }

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
            for raw in self.0.iter().filter_map(|&raw| to_pid(raw)) {
                let _ = kill(raw, Signal::SIGKILL);
            }
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
    fn leftovers_that_left_the_group_die_with_the_main_process() {
        let dir = tempfile::tempdir().unwrap();
        let mut stand_in = start_stand_in("finish", dir.path());
        let pids = tree_pids(dir.path());
        assert!(stand_in.wait().unwrap().success());
        assert!(dir.path().join("done").exists());
        assert_all_gone(&pids);
    }
}
