//! Session detail on the runner: transcript pages, figures and changes, with path guards.

mod changes;
mod facts;
#[cfg(test)]
mod fixtures;
mod git_changes;
mod jsonl;
mod line_diff;
mod reader;
mod summary;
mod unified_diff;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use nullradix_app::AppError;
use nullradix_app::session_detail::{
    ChangeOrigin, ChangeRequest, DiffRequest, FileDiff, SessionChanges, TRANSCRIPT_NOTE,
    TranscriptChunk, TranscriptRead, TranscriptSource, TranscriptSummary, change_not_found,
    check_change_path, check_transcript_path,
};
use serde_json::Value;

use self::changes::TranscriptChanges;
use self::facts::FactsCollector;

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

/// The transcript file when its path passes the guard and it exists; `None` when it is gone.
fn transcript_file(path: &str) -> Result<Option<PathBuf>, AppError> {
    check_transcript_path(path)?;
    let file = PathBuf::from(path);
    Ok(fs::metadata(&file)
        .is_ok_and(|meta| meta.is_file())
        .then_some(file))
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

    /// Reads what is new; a file shorter than read so far starts over.
    fn update(&mut self, path: &Path) -> io::Result<()> {
        let len = fs::metadata(path)?.len();
        if len < self.offset {
            *self = Self::new(&self.cwd);
        }
        let bytes = jsonl::read_from(path, self.offset)?;
        self.feed(&bytes);
        self.size = len;
        Ok(())
    }

    fn feed(&mut self, bytes: &[u8]) {
        for line in jsonl::lines(bytes, self.offset) {
            if let Ok(value) = serde_json::from_str::<Value>(line.text) {
                self.facts.feed(&value);
                self.changes.feed(&value);
            }
        }
        self.offset = jsonl::complete_end(bytes, self.offset);
    }
}

#[derive(Debug)]
struct CachedScan {
    scan: TranscriptScan,
    used: Instant,
}

/// Read states of recent transcripts, so a growing file is read only anew.
#[derive(Debug, Default)]
pub(crate) struct ScanCache {
    scans: Mutex<HashMap<PathBuf, CachedScan>>,
}

impl ScanCache {
    /// Brings the state of `path` up to date and hands it to `read`; blocking.
    fn with_scan<T>(
        &self,
        path: &Path,
        cwd: &str,
        read: impl FnOnce(&TranscriptScan) -> T,
    ) -> io::Result<T> {
        let mut scans = self.scans.lock().unwrap_or_else(PoisonError::into_inner);
        let fresh = scans.get(path).is_none_or(|cached| cached.scan.cwd != cwd);
        if fresh {
            if scans.len() >= MAX_SCANS {
                let oldest = scans
                    .iter()
                    .min_by_key(|(_, cached)| cached.used)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    scans.remove(&oldest);
                }
            }
            scans.insert(
                path.to_path_buf(),
                CachedScan {
                    scan: TranscriptScan::new(cwd),
                    used: Instant::now(),
                },
            );
        }
        let cached = scans
            .get_mut(path)
            .ok_or_else(|| io::Error::other("the scan vanished"))?;
        cached.used = Instant::now();
        if let Err(error) = cached.scan.update(path) {
            scans.remove(path);
            return Err(error);
        }
        Ok(read(&cached.scan))
    }
}

/// A page of the transcript; `None` when the file is gone.
pub(crate) async fn read(request: &TranscriptRead) -> Result<Option<TranscriptChunk>, AppError> {
    let path = request.path.clone();
    let after = request.after;
    let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
    blocking(move || {
        let Some(file) = transcript_file(&path)? else {
            return Ok(None);
        };
        match jsonl::read_from(&file, after) {
            Ok(bytes) => Ok(Some(reader::parse(&bytes, after, limit))),
            // gone between the check and the read
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unavailable(&error)),
        }
    })
    .await?
}

/// Figures of the transcript with its size; `None` when the file is gone.
pub(crate) async fn facts(
    cache: &Arc<ScanCache>,
    source: &TranscriptSource,
) -> Result<Option<TranscriptSummary>, AppError> {
    let cache = Arc::clone(cache);
    let source = source.clone();
    blocking(move || {
        let Some(file) = transcript_file(&source.path)? else {
            return Ok(None);
        };
        match cache.with_scan(&file, &source.cwd, |scan| TranscriptSummary {
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
    cache: &Arc<ScanCache>,
    request: &ChangeRequest,
) -> Result<SessionChanges, AppError> {
    let cwd = PathBuf::from(&request.cwd);
    let git = tokio::time::timeout(GIT_TIMEOUT, async {
        let Some(scope) = git_changes::scope(&cwd).await? else {
            return Ok::<_, AppError>(None);
        };
        let files = git_changes::files(&scope).await?;
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
    let cache = Arc::clone(cache);
    let cwd = request.cwd.clone();
    blocking(move || {
        let Some(file) = transcript_file(&path)? else {
            return Ok(SessionChanges::none());
        };
        cache
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
    cache: &Arc<ScanCache>,
    request: &DiffRequest,
) -> Result<FileDiff, AppError> {
    let wanted = request.path.clone();
    check_change_path(&wanted)?;
    let cwd = PathBuf::from(&request.changes.cwd);
    let git = tokio::time::timeout(GIT_TIMEOUT, async {
        match git_changes::scope(&cwd).await? {
            Some(scope) => git_changes::file(&scope, &wanted).await.map(Some),
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
    let cache = Arc::clone(cache);
    let cwd = request.changes.cwd.clone();
    blocking(move || {
        let Some(file) = transcript_file(&path)? else {
            return Err(change_not_found(&wanted));
        };
        cache
            .with_scan(&file, &cwd, |scan| scan.changes.diff(&wanted))
            .map_err(|error| unavailable(&error))?
            .ok_or_else(|| change_not_found(&wanted))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        fs::write(&path, fixtures::transcript()).unwrap();
        let chunk = read(&page(text(&path))).await.unwrap().unwrap();
        assert_eq!(chunk.entries.len(), 18);
        assert_eq!(chunk.skipped, 1);
        let rest = read(&TranscriptRead {
            after: chunk.cursor,
            ..page(text(&path))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!((rest.entries.len(), rest.cursor), (0, chunk.cursor));
        let cache = Arc::new(ScanCache::default());
        let source = TranscriptSource {
            path: text(&path),
            cwd: fixtures::CWD.to_owned(),
        };
        let summary = facts(&cache, &source).await.unwrap().unwrap();
        assert_eq!(summary.size_bytes, fs::metadata(&path).unwrap().len());
        assert_eq!(summary.facts.title.as_deref(), Some("Demo title"));

        let missing = text(&dir.path().join("gone.jsonl"));
        assert_eq!(read(&page(missing.clone())).await.unwrap(), None);
        let gone = TranscriptSource {
            path: missing,
            ..source
        };
        assert_eq!(facts(&cache, &gone).await.unwrap(), None);

        let other = dir.path().join("secret.txt");
        fs::write(&other, "secret").unwrap();
        let sneaky = format!("{}/x/../s.jsonl", text(dir.path()));
        for path in [text(&other), String::from("s.jsonl"), sneaky] {
            let error = read(&page(path.clone())).await.unwrap_err();
            assert_eq!(error.code(), "transcript-path-invalid", "{path}");
        }
        let folder = dir.path().join("folder.jsonl");
        fs::create_dir(&folder).unwrap();
        assert_eq!(read(&page(text(&folder))).await.unwrap(), None);
    }

    #[tokio::test]
    async fn changes_outside_git_come_from_the_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        fs::write(&path, fixtures::transcript()).unwrap();
        let cache = Arc::new(ScanCache::default());
        let request = ChangeRequest {
            cwd: fixtures::CWD.to_owned(),
            transcript: Some(text(&path)),
        };
        let found = changes(&cache, &request).await.unwrap();
        assert_eq!(found.origin, ChangeOrigin::Transcript);
        assert_eq!(found.note.as_deref(), Some(TRANSCRIPT_NOTE));
        let paths: Vec<&str> = found.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["src/main.rs", "src/new.rs"]);

        let wanted = |path: &str| DiffRequest {
            changes: request.clone(),
            path: path.to_owned(),
        };
        let shown = diff(&cache, &wanted("src/main.rs")).await.unwrap();
        assert_eq!(shown.hunks[0].header, "@@ -1,1 +1,3 @@");
        let error = diff(&cache, &wanted("src/other.rs")).await.unwrap_err();
        assert_eq!(error.code(), "change-not-found");
        for path in ["../s.jsonl", "/etc/passwd", "C:/x"] {
            let error = diff(&cache, &wanted(path)).await.unwrap_err();
            assert_eq!(error.code(), "change-path-invalid", "{path}");
        }

        let bare = ChangeRequest {
            transcript: None,
            ..request
        };
        assert_eq!(
            changes(&cache, &bare).await.unwrap(),
            SessionChanges::none()
        );
        let error = diff(
            &cache,
            &DiffRequest {
                changes: bare,
                path: String::from("src/main.rs"),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "change-not-found");
    }
}
