//! Session detail on the runner: transcript pages, figures and changes, with path guards.

mod changes;
mod checkout;
mod facts;
#[cfg(test)]
mod fixtures;
mod git_changes;
mod guard;
mod jsonl;
mod line_diff;
mod reader;
mod summary;
mod unified_diff;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use nullradix_app::AppError;
use nullradix_app::session_detail::{
    ChangeOrigin, ChangeRequest, DiffRequest, FileDiff, SessionChanges, TRANSCRIPT_NOTE,
    TranscriptChunk, TranscriptRead, TranscriptSource, TranscriptSummary, change_not_found,
    check_change_path,
};
use serde_json::Value;

use self::changes::TranscriptChanges;
use self::checkout::{Abort, CheckoutGate};
use self::facts::FactsCollector;
use self::guard::TranscriptGuard;

/// Git gets this long before the transcript answers instead, as legacy.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Transcripts whose read state is kept.
const MAX_SCANS: usize = 32;

fn unavailable(error: &io::Error) -> AppError {
    AppError::unavailable("runner-failed", format!("the runner failed: {error}"))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(AppError::internal)
}

/// Figures and changes of a transcript up to an offset.
#[derive(Debug, Clone)]
struct TranscriptScan {
    cwd: String,
    offset: u64,
    size: u64,
    facts: FactsCollector,
    changes: TranscriptChanges,
}

impl TranscriptScan {
    fn new(cwd: &str) -> Self {
        Self {
            cwd: cwd.to_owned(),
            offset: 0,
            size: 0,
            facts: FactsCollector::default(),
            changes: TranscriptChanges::new(cwd),
        }
    }

    /// Reads what is new, line by line; a file shorter than read so far starts over.
    fn update(&mut self, path: &Path) -> io::Result<()> {
        let len = std::fs::metadata(path)?.len();
        if len < self.offset {
            *self = Self::new(&self.cwd);
        }
        let mut lines = jsonl::open_lines(path, self.offset)?;
        while let Some(line) = lines.next_line()? {
            if let Ok(value) = serde_json::from_str::<Value>(line.text) {
                self.facts.feed(&value);
                self.changes.feed(&value);
            }
        }
        self.offset = lines.complete();
        self.size = len;
        Ok(())
    }
}

#[derive(Debug)]
struct CachedScan {
    scan: Arc<Mutex<TranscriptScan>>,
    used: Instant,
}

/// Read states of recent transcripts, so a growing file is read only anew.
#[derive(Debug, Default)]
pub(crate) struct ScanCache {
    scans: Mutex<HashMap<PathBuf, CachedScan>>,
}

impl ScanCache {
    /// Brings `path` up to date and hands it to `read`; locks only this transcript while reading.
    fn with_scan<T>(
        &self,
        path: &Path,
        cwd: &str,
        read: impl FnOnce(&TranscriptScan) -> T,
    ) -> io::Result<T> {
        let slot = {
            let mut scans = self.scans.lock().unwrap_or_else(PoisonError::into_inner);
            if !scans.contains_key(path) && scans.len() >= MAX_SCANS {
                let oldest = scans
                    .iter()
                    .min_by_key(|(_, cached)| cached.used)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    scans.remove(&oldest);
                }
            }
            let cached = scans
                .entry(path.to_path_buf())
                .or_insert_with(|| CachedScan {
                    scan: Arc::new(Mutex::new(TranscriptScan::new(cwd))),
                    used: Instant::now(),
                });
            cached.used = Instant::now();
            Arc::clone(&cached.scan)
        };
        let mut scan = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if scan.cwd != cwd {
            *scan = TranscriptScan::new(cwd);
        }
        if let Err(error) = scan.update(path) {
            *scan = TranscriptScan::new(cwd);
            return Err(error);
        }
        Ok(read(&scan))
    }
}

/// Transcript reads of one runner: what may be read, what was read and who is busy in git.
#[derive(Debug)]
pub(crate) struct Transcripts {
    guard: TranscriptGuard,
    scans: ScanCache,
    checkouts: CheckoutGate,
}

impl Transcripts {
    /// Reads only below the `projects` folder of the config folders `roots`.
    pub(crate) fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            guard: TranscriptGuard::new(roots),
            scans: ScanCache::default(),
            checkouts: CheckoutGate::default(),
        }
    }

    /// A page of the transcript; `None` when the file is gone.
    pub(crate) async fn read(
        self: &Arc<Self>,
        request: &TranscriptRead,
    ) -> Result<Option<TranscriptChunk>, AppError> {
        let this = Arc::clone(self);
        let path = request.path.clone();
        let after = request.after;
        let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
        blocking(move || {
            let Some(file) = this.guard.file(&path)? else {
                return Ok(None);
            };
            match reader::parse_file(&file, after, limit) {
                Ok(chunk) => Ok(Some(chunk)),
                // gone between the check and the read
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(unavailable(&error)),
            }
        })
        .await?
    }

    /// Figures of the transcript with its size; `None` when the file is gone.
    pub(crate) async fn facts(
        self: &Arc<Self>,
        source: &TranscriptSource,
    ) -> Result<Option<TranscriptSummary>, AppError> {
        let this = Arc::clone(self);
        let source = source.clone();
        blocking(move || {
            let Some(file) = this.guard.file(&source.path)? else {
                return Ok(None);
            };
            match this
                .scans
                .with_scan(&file, &source.cwd, |scan| TranscriptSummary {
                    size_bytes: scan.size,
                    facts: scan.facts.facts(),
                }) {
                Ok(summary) => Ok(Some(summary)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(unavailable(&error)),
            }
        })
        .await?
    }

    /// Git when the session has its own branch and git answers in time, else the transcript.
    pub(crate) async fn changes(
        self: &Arc<Self>,
        request: &ChangeRequest,
    ) -> Result<SessionChanges, AppError> {
        let cwd = PathBuf::from(&request.cwd);
        let git = tokio::time::timeout(GIT_TIMEOUT, async {
            let _turn = self.checkouts.enter(&cwd).await;
            // blocking reads of a request that timed out stop when this ends
            let abort = Abort::new();
            let Some(scope) = git_changes::scope(&cwd).await? else {
                return Ok::<_, AppError>(None);
            };
            let files = git_changes::files(&scope, &abort.flag()).await?;
            Ok(Some(SessionChanges {
                origin: ChangeOrigin::Git,
                base: Some(scope.base),
                note: None,
                files,
            }))
        })
        .await;
        // a failing or slow git leaves the answer to the transcript, as legacy
        if let Ok(Ok(Some(found))) = git {
            return Ok(found);
        }
        let Some(path) = request.transcript.clone() else {
            return Ok(SessionChanges::none());
        };
        let this = Arc::clone(self);
        let cwd = request.cwd.clone();
        blocking(move || {
            let Some(file) = this.guard.file(&path)? else {
                return Ok(SessionChanges::none());
            };
            this.scans
                .with_scan(&file, &cwd, |scan| SessionChanges {
                    origin: ChangeOrigin::Transcript,
                    base: None,
                    note: Some(TRANSCRIPT_NOTE.to_owned()),
                    files: scan.changes.files(),
                })
                .or_else(|error| {
                    if error.kind() == io::ErrorKind::NotFound {
                        Ok(SessionChanges::none())
                    } else {
                        Err(unavailable(&error))
                    }
                })
        })
        .await?
    }

    /// Diff of one changed file from the source `changes` uses.
    pub(crate) async fn diff(
        self: &Arc<Self>,
        request: &DiffRequest,
    ) -> Result<FileDiff, AppError> {
        let wanted = request.path.clone();
        check_change_path(&wanted)?;
        let cwd = PathBuf::from(&request.changes.cwd);
        let git = tokio::time::timeout(GIT_TIMEOUT, async {
            let _turn = self.checkouts.enter(&cwd).await;
            let abort = Abort::new();
            match git_changes::scope(&cwd).await? {
                Some(scope) => git_changes::file(&scope, &wanted, &abort.flag())
                    .await
                    .map(Some),
                None => Ok(None),
            }
        })
        .await;
        match git {
            Ok(Ok(Some(found))) => return Ok(found),
            Ok(Err(error)) => return Err(error),
            _ => {}
        }
        let Some(path) = request.changes.transcript.clone() else {
            return Err(change_not_found(&wanted));
        };
        let this = Arc::clone(self);
        let cwd = request.changes.cwd.clone();
        blocking(move || {
            let Some(file) = this.guard.file(&path)? else {
                return Err(change_not_found(&wanted));
            };
            this.scans
                .with_scan(&file, &cwd, |scan| scan.changes.diff(&wanted))
                .map_err(|error| unavailable(&error))?
                .ok_or_else(|| change_not_found(&wanted))
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use nullradix_app::session_detail::DiffRequest;

    use super::*;

    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    /// A config folder with its transcripts and the reads that may use them.
    fn setup() -> (tempfile::TempDir, PathBuf, Arc<Transcripts>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("claude");
        let folder = root.join("projects").join("demo");
        fs::create_dir_all(&folder).unwrap();
        (dir, folder, Arc::new(Transcripts::new(vec![root])))
    }

    #[test]
    fn a_growing_file_is_read_only_in_its_new_part() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let content = fixtures::transcript();
        let middle = content.len() / 2;
        fs::write(&path, &content.as_bytes()[..middle]).unwrap();
        let mut scan = TranscriptScan::new(fixtures::CWD);
        scan.update(&path).unwrap();
        assert!(scan.offset <= u64::try_from(middle).unwrap());
        fs::write(&path, &content).unwrap();
        scan.update(&path).unwrap();
        assert_eq!(scan.offset, u64::try_from(content.len()).unwrap());
        assert_eq!(scan.facts.facts().todos.len(), 3);
        assert_eq!(scan.changes.files().len(), 2);
        fs::write(
            &path,
            fixtures::line(&serde_json::json!({ "type": "custom-title", "customTitle": "New" })),
        )
        .unwrap();
        scan.update(&path).unwrap();
        assert_eq!(scan.facts.facts().title.as_deref(), Some("New"));
        assert_eq!(scan.facts.facts().todos, Vec::new());
    }

    #[test]
    fn a_first_full_read_blocks_no_other_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let (slow, quick) = (
            dir.path().join("slow.jsonl"),
            dir.path().join("quick.jsonl"),
        );
        fs::write(&slow, fixtures::transcript()).unwrap();
        fs::write(&quick, fixtures::transcript()).unwrap();
        let cache = Arc::new(ScanCache::default());
        cache.with_scan(&slow, fixtures::CWD, |_| ()).unwrap();
        let held = {
            let scans = cache.scans.lock().unwrap();
            Arc::clone(&scans[&slow].scan)
        };
        // a read of `slow` now waits for the lock a long read would hold
        let guard = held.lock().unwrap();
        let waiting = {
            let (cache, slow) = (Arc::clone(&cache), slow.clone());
            std::thread::spawn(move || cache.with_scan(&slow, fixtures::CWD, |scan| scan.size))
        };
        let size = cache
            .with_scan(&quick, fixtures::CWD, |scan| scan.size)
            .unwrap();
        assert_eq!(size, fs::metadata(&quick).unwrap().len());
        assert!(!waiting.is_finished());
        drop(guard);
        assert_eq!(waiting.join().unwrap().unwrap(), size);
    }

    #[test]
    fn old_scans_make_room_and_a_vanished_file_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ScanCache::default();
        for number in 0..=MAX_SCANS {
            let path = dir.path().join(format!("{number}.jsonl"));
            fs::write(&path, fixtures::transcript()).unwrap();
            cache.with_scan(&path, fixtures::CWD, |_| ()).unwrap();
        }
        assert_eq!(cache.scans.lock().unwrap().len(), MAX_SCANS);
        let gone = dir.path().join("gone.jsonl");
        assert!(cache.with_scan(&gone, fixtures::CWD, |_| ()).is_err());
    }

    fn page(path: String) -> TranscriptRead {
        TranscriptRead {
            path,
            after: 0,
            limit: 500,
        }
    }

    #[tokio::test]
    async fn reads_a_transcript_only_through_the_guard() {
        let (dir, folder, transcripts) = setup();
        let path = folder.join("s.jsonl");
        fs::write(&path, fixtures::transcript()).unwrap();
        let chunk = transcripts.read(&page(text(&path))).await.unwrap().unwrap();
        assert_eq!(chunk.entries.len(), 18);
        assert_eq!(chunk.skipped, 1);
        let rest = transcripts
            .read(&TranscriptRead {
                after: chunk.cursor,
                ..page(text(&path))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!((rest.entries.len(), rest.cursor), (0, chunk.cursor));
        let source = TranscriptSource {
            path: text(&path),
            cwd: fixtures::CWD.to_owned(),
        };
        let summary = transcripts.facts(&source).await.unwrap().unwrap();
        assert_eq!(summary.size_bytes, fs::metadata(&path).unwrap().len());
        assert_eq!(summary.facts.title.as_deref(), Some("Demo title"));

        let missing = text(&folder.join("gone.jsonl"));
        assert_eq!(
            transcripts.read(&page(missing.clone())).await.unwrap(),
            None
        );
        let gone = TranscriptSource {
            path: missing,
            ..source
        };
        assert_eq!(transcripts.facts(&gone).await.unwrap(), None);

        let other = dir.path().join("secret.txt");
        fs::write(&other, "secret").unwrap();
        let outside = dir.path().join("outside.jsonl");
        fs::write(&outside, "{}\n").unwrap();
        let sneaky = format!("{}/x/../s.jsonl", text(&folder));
        for path in [
            text(&other),
            text(&outside),
            String::from("s.jsonl"),
            sneaky,
        ] {
            let error = transcripts.read(&page(path.clone())).await.unwrap_err();
            assert_eq!(error.code(), "transcript-path-invalid", "{path}");
            let source = TranscriptSource {
                path: path.clone(),
                cwd: fixtures::CWD.to_owned(),
            };
            let error = transcripts.facts(&source).await.unwrap_err();
            assert_eq!(error.code(), "transcript-path-invalid", "{path}");
        }
        let directory = folder.join("folder.jsonl");
        fs::create_dir(&directory).unwrap();
        assert_eq!(
            transcripts.read(&page(text(&directory))).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn pages_of_a_large_transcript_add_up_to_the_whole() {
        let (_dir, folder, transcripts) = setup();
        let path = folder.join("big.jsonl");
        fs::write(&path, fixtures::repeated(300)).unwrap();
        let whole = transcripts
            .read(&TranscriptRead {
                limit: 2000,
                ..page(text(&path))
            })
            .await
            .unwrap()
            .unwrap();
        let mut entries = Vec::new();
        let mut after = 0;
        loop {
            let chunk = transcripts
                .read(&TranscriptRead {
                    after,
                    limit: 40,
                    ..page(text(&path))
                })
                .await
                .unwrap()
                .unwrap();
            let done = chunk.entries.is_empty();
            // a page may repeat an entry to give it its result
            for entry in chunk.entries {
                match entries.iter().position(
                    |seen: &nullradix_app::session_detail::TranscriptEntry| seen.id() == entry.id(),
                ) {
                    Some(index) => entries[index] = entry,
                    None => entries.push(entry),
                }
            }
            after = chunk.cursor;
            if done {
                break;
            }
        }
        assert_eq!(entries, whole.entries);
        assert_eq!(after, whole.cursor);
    }

    #[tokio::test]
    async fn changes_outside_git_come_from_the_transcript() {
        let (dir, folder, transcripts) = setup();
        let path = folder.join("s.jsonl");
        fs::write(&path, fixtures::transcript()).unwrap();
        let request = ChangeRequest {
            cwd: fixtures::CWD.to_owned(),
            transcript: Some(text(&path)),
        };
        let found = transcripts.changes(&request).await.unwrap();
        assert_eq!(found.origin, ChangeOrigin::Transcript);
        assert_eq!(found.note.as_deref(), Some(TRANSCRIPT_NOTE));
        let paths: Vec<&str> = found.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["src/main.rs", "src/new.rs"]);

        let wanted = |path: &str| DiffRequest {
            changes: request.clone(),
            path: path.to_owned(),
        };
        let shown = transcripts.diff(&wanted("src/main.rs")).await.unwrap();
        assert_eq!(shown.hunks[0].header, "@@ -1,1 +1,3 @@");
        let error = transcripts.diff(&wanted("src/other.rs")).await.unwrap_err();
        assert_eq!(error.code(), "change-not-found");
        for path in ["../s.jsonl", "/etc/passwd", "C:/x"] {
            let error = transcripts.diff(&wanted(path)).await.unwrap_err();
            assert_eq!(error.code(), "change-path-invalid", "{path}");
        }

        let outside = dir.path().join("outside.jsonl");
        fs::write(&outside, fixtures::transcript()).unwrap();
        let stranger = ChangeRequest {
            transcript: Some(text(&outside)),
            ..request.clone()
        };
        let error = transcripts.changes(&stranger).await.unwrap_err();
        assert_eq!(error.code(), "transcript-path-invalid");

        let bare = ChangeRequest {
            transcript: None,
            ..request
        };
        assert_eq!(
            transcripts.changes(&bare).await.unwrap(),
            SessionChanges::none()
        );
        let error = transcripts
            .diff(&DiffRequest {
                changes: bare,
                path: String::from("src/main.rs"),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code(), "change-not-found");
    }
}
