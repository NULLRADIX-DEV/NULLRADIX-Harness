use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use nullradix_app::AppError;
use nullradix_app::dispatch::runs::RunFilesPort;
use nullradix_app::repositories::{Located, network_path_refused};
use nullradix_app::review::{BranchChanges, BranchFile, FilePatch, ReviewRequest};
use nullradix_app::runs::{RunProgress, RunSpec, RunnerPort};
use nullradix_app::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary,
};
use nullradix_app::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, WorktreeRemoval,
};
use nullradix_domain::{Id, NetworkShares, Run};
use nullradix_os::{create_data_dir, spawn_detached, write_atomic};

use crate::protocol::{EnvFile, RunFiles, SpecFile, read_output, read_report, supervisor_alive};
use crate::transcript::{TranscriptRoots, Transcripts};

/// How the runner starts the supervisor: a program plus leading arguments, the run dir follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Runs processes on this machine, each under a detached supervisor.
#[derive(Debug, Clone)]
pub struct LocalRunner {
    runs: PathBuf,
    supervisor: SupervisorCommand,
    transcripts: Arc<Transcripts>,
    plugin: Option<String>,
    shares: NetworkShares,
}

fn unavailable(error: &io::Error) -> AppError {
    AppError::unavailable("runner-failed", format!("the runner failed: {error}"))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(AppError::internal)?
        .map_err(|error| unavailable(&error))
}

impl LocalRunner {
    #[must_use]
    pub fn new(runs: PathBuf, supervisor: SupervisorCommand, plugin: Option<String>) -> Self {
        Self {
            runs,
            supervisor,
            plugin,
            transcripts: Arc::new(Transcripts::new(nullradix_os::claude_config_dirs())),
            shares: NetworkShares::default(),
        }
    }

    /// Lets folders on these hosts be used.
    #[must_use]
    pub fn with_network_shares(mut self, shares: NetworkShares) -> Self {
        self.shares = shares;
        self
    }

    /// Lets transcripts be read below `projects` of these config folders and any a path names.
    #[must_use]
    pub fn with_transcript_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.transcripts = Arc::new(Transcripts::new(roots));
        self
    }

    /// Like `with_transcript_roots`, with folders that are kept current by the holder of `roots`.
    #[must_use]
    pub fn with_live_transcript_roots(mut self, roots: TranscriptRoots) -> Self {
        self.transcripts = Arc::new(Transcripts::shared(roots));
        self
    }

    /// Turns away a network path before any file call.
    fn refuse_networks(&self, paths: &[&str]) -> Result<(), AppError> {
        match paths.iter().find(|path| self.shares.refuses(path)) {
            Some(path) => Err(network_path_refused(path)),
            None => Ok(()),
        }
    }

    fn files(&self, run: Id<Run>) -> RunFiles {
        RunFiles::at(&self.runs.join(run.to_string()))
    }
}

fn validate(spec: &RunSpec, shares: &NetworkShares) -> Result<(), AppError> {
    let cwd = spec.cwd.to_string_lossy();
    if shares.refuses(&cwd) {
        return Err(network_path_refused(&cwd));
    }
    spec.validate()?;
    if !spec.cwd.is_dir() {
        return Err(AppError::invalid(
            "run-cwd-invalid",
            format!("{} is not a directory", spec.cwd.display()),
        ));
    }
    Ok(())
}

#[async_trait]
impl RunnerPort for LocalRunner {
    async fn check(&self, spec: &RunSpec) -> Result<(), AppError> {
        validate(spec, &self.shares)
    }

    async fn start(&self, run: Id<Run>, spec: &RunSpec) -> Result<u32, AppError> {
        validate(spec, &self.shares)?;
        let files = self.files(run);
        let file = SpecFile {
            program: spec.program.clone(),
            args: spec.args.clone(),
            cwd: spec.cwd.clone(),
            timeout_ms: spec
                .timeout
                .map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
            env: EnvFile {
                set: spec.env.set.clone(),
                remove: spec.env.remove.clone(),
                defaults: spec.env.defaults.clone(),
            },
            stdin: spec.stdin.clone(),
            claude: spec.claude,
        };
        let supervisor = self.supervisor.clone();
        let runs = self.runs.clone();
        blocking(move || {
            // private run folders, tightened when they are older
            create_data_dir(&runs)?;
            create_data_dir(files.dir())?;
            let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
            write_atomic(&files.spec(), &json)?;
            let mut args: Vec<OsString> = supervisor.args.iter().map(OsString::from).collect();
            args.push(files.dir().as_os_str().to_owned());
            let pid = spawn_detached(&supervisor.program, &args, files.dir())?;
            if let Err(error) = write_atomic(&files.supervisor(), pid.to_string().as_bytes()) {
                // the start counts as failed, so the command must not run unseen
                let _ = write_atomic(&files.cancel(), b"");
                return Err(error);
            }
            Ok(pid)
        })
        .await
    }

    async fn cancel(&self, run: Id<Run>) -> Result<(), AppError> {
        let files = self.files(run);
        blocking(move || write_atomic(&files.cancel(), b"")).await
    }

    async fn progress(
        &self,
        run: Id<Run>,
        after: i64,
        limit: u32,
    ) -> Result<RunProgress, AppError> {
        let files = self.files(run);
        blocking(move || {
            // the report comes first: once it exists, all output is already written
            let report = read_report(&files)?;
            let lines = if limit == 0 {
                Vec::new()
            } else {
                read_output(&files, after, limit)?
            };
            let alive = supervisor_alive(&files)?;
            Ok(RunProgress {
                lines,
                report,
                alive,
            })
        })
        .await
    }

    async fn locate(&self, path: &str) -> Result<Located, AppError> {
        crate::git::locate(path, &self.shares).await
    }

    async fn create_worktree(&self, request: &NewWorktree) -> Result<CreatedWorktree, AppError> {
        self.refuse_networks(&[&request.cwd, &request.root, &request.path])?;
        crate::worktree::create(request, self.plugin.as_deref()).await
    }

    async fn folders_exist(&self, paths: &[String]) -> Result<Vec<bool>, AppError> {
        crate::worktree::folders_exist(paths, &self.shares).await
    }

    async fn ensure_worktree(&self, request: &EnsureWorktree) -> Result<EnsuredWorktree, AppError> {
        self.refuse_networks(&[&request.root, &request.path])?;
        crate::ensure::ensure(request, self.plugin.as_deref()).await
    }

    async fn remove_worktree(&self, request: &RemoveWorktree) -> Result<WorktreeRemoval, AppError> {
        self.refuse_networks(&[&request.cwd, &request.root, &request.path])?;
        crate::cleanup::remove(request).await
    }

    async fn read_transcript(
        &self,
        request: &TranscriptRead,
    ) -> Result<Option<TranscriptChunk>, AppError> {
        self.transcripts.read(request).await
    }

    async fn transcript_facts(
        &self,
        source: &TranscriptSource,
    ) -> Result<Option<TranscriptSummary>, AppError> {
        self.transcripts.facts(source).await
    }

    async fn session_changes(&self, request: &ChangeRequest) -> Result<SessionChanges, AppError> {
        self.refuse_networks(&[&request.cwd])?;
        self.transcripts.changes(request).await
    }

    async fn session_diff(&self, request: &DiffRequest) -> Result<FileDiff, AppError> {
        self.refuse_networks(&[&request.changes.cwd])?;
        self.transcripts.diff(request).await
    }

    async fn review_changes(&self, request: &ReviewRequest) -> Result<BranchChanges, AppError> {
        self.refuse_networks(&[&request.root])?;
        crate::review::changes(request).await
    }

    async fn review_patch(
        &self,
        request: &ReviewRequest,
        path: &str,
    ) -> Result<FilePatch, AppError> {
        self.refuse_networks(&[&request.root])?;
        crate::review::patch(request, path).await
    }

    async fn review_file(
        &self,
        root: &str,
        branch: &str,
        path: &str,
    ) -> Result<BranchFile, AppError> {
        self.refuse_networks(&[root])?;
        crate::review::file(root, branch, path).await
    }
}

/// A screenshot file name is one plain segment, so it cannot leave the run folder.
fn plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
}

#[async_trait]
impl RunFilesPort for LocalRunner {
    async fn read_screenshot(&self, run: Id<Run>, file: &str) -> Result<Option<Vec<u8>>, AppError> {
        if !plain_file_name(file) {
            return Ok(None);
        }
        let path = self.files(run).artifacts().join(file);
        blocking(move || match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nullradix_app::ErrorKind;
    use nullradix_app::runs::RunEnv;

    use super::*;

    fn runner(dir: &std::path::Path) -> LocalRunner {
        LocalRunner::new(
            dir.join("runs"),
            SupervisorCommand {
                program: PathBuf::from("nullradix-missing-supervisor"),
                args: Vec::new(),
            },
            None,
        )
    }

    fn spec(cwd: PathBuf) -> RunSpec {
        RunSpec {
            program: String::from("echo"),
            args: Vec::new(),
            cwd,
            timeout: Some(Duration::from_secs(1)),
            env: RunEnv::default(),
            stdin: None,
            claude: false,
        }
    }

    #[tokio::test]
    async fn rejects_bad_specs_before_touching_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let runner = runner(dir.path());
        let id = Id::from_uuid(uuid::Uuid::from_u128(1));
        let mut empty = spec(dir.path().to_path_buf());
        empty.program = String::from(" ");
        assert_eq!(
            runner.start(id, &empty).await.unwrap_err().code(),
            "run-program-invalid"
        );
        let missing = spec(dir.path().join("nope"));
        assert_eq!(
            runner.check(&missing).await.unwrap_err().code(),
            "run-cwd-invalid"
        );
        assert_eq!(
            runner.start(id, &missing).await.unwrap_err().code(),
            "run-cwd-invalid"
        );
        assert!(!dir.path().join("runs").exists());
    }

    #[tokio::test]
    async fn a_supervisor_that_cannot_start_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let runner = runner(dir.path());
        let error = runner
            .start(
                Id::from_uuid(uuid::Uuid::from_u128(2)),
                &spec(dir.path().to_path_buf()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Unavailable);
    }

    #[tokio::test]
    async fn progress_of_an_unknown_run_is_empty_and_dead() {
        let dir = tempfile::tempdir().unwrap();
        let progress = runner(dir.path())
            .progress(Id::from_uuid(uuid::Uuid::from_u128(3)), 0, 10)
            .await
            .unwrap();
        assert_eq!(progress.lines, Vec::new());
        assert_eq!(progress.report, None);
        assert!(!progress.alive);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_folders_are_private_and_an_older_open_runs_folder_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let mode_of =
            |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("runs");
        fs::create_dir(&runs).unwrap();
        fs::set_permissions(&runs, fs::Permissions::from_mode(0o755)).unwrap();
        let id = Id::from_uuid(uuid::Uuid::from_u128(4));
        // the run folder is made before the missing supervisor fails
        let _ = runner(dir.path())
            .start(id, &spec(dir.path().to_path_buf()))
            .await;
        assert_eq!(mode_of(&runs), 0o700);
        assert_eq!(mode_of(&runs.join(id.to_string())), 0o700);
    }

    #[tokio::test]
    async fn network_working_folders_are_refused_before_any_check() {
        let dir = tempfile::tempdir().unwrap();
        let runner = runner(dir.path());
        let id = Id::from_uuid(uuid::Uuid::from_u128(5));
        for cwd in ["//server/share", "\\\\server\\share", "\\\\?\\C:\\work"] {
            let spec = spec(PathBuf::from(cwd));
            assert_eq!(
                runner.check(&spec).await.unwrap_err().code(),
                "network-path-refused",
                "{cwd}"
            );
            assert_eq!(
                runner.start(id, &spec).await.unwrap_err().code(),
                "network-path-refused",
                "{cwd}"
            );
        }
        assert!(!dir.path().join("runs").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_listed_share_is_not_refused_as_a_working_folder() {
        // two leading slashes name a host to the check and the first folder to this system
        let doubled = PathBuf::from(format!("/{}", std::env::temp_dir().display()));
        let host = doubled
            .to_string_lossy()
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap()
            .to_owned();
        let dir = tempfile::tempdir().unwrap();
        let spec = spec(doubled);
        assert_eq!(
            runner(dir.path()).check(&spec).await.unwrap_err().code(),
            "network-path-refused"
        );
        let listed = runner(dir.path()).with_network_shares(NetworkShares::new([host]));
        assert!(listed.check(&spec).await.is_ok());
    }

    #[tokio::test]
    async fn stored_network_paths_are_refused_at_every_git_boundary() {
        use nullradix_app::review::ReviewRequest;
        use nullradix_app::session_detail::{ChangeRequest, DiffRequest};
        use nullradix_app::worktrees::{EnsureWorktree, NewWorktree, RemoveWorktree};
        let dir = tempfile::tempdir().unwrap();
        let runner = runner(dir.path());
        let share = String::from("//srv/share/arches");
        let local = text_of(dir.path());
        let review = |root: &str| ReviewRequest {
            root: root.to_owned(),
            branch: String::from("ticket/a"),
            target: None,
        };
        let changes = |cwd: &str| ChangeRequest {
            cwd: cwd.to_owned(),
            transcript: None,
        };
        let refused = |code: &str| assert_eq!(code, "network-path-refused");
        for (root, path) in [(&share, &local), (&local, &share)] {
            let new = NewWorktree {
                cwd: local.clone(),
                root: root.clone(),
                branch: String::from("ticket/a"),
                path: path.clone(),
                target: None,
            };
            refused(runner.create_worktree(&new).await.unwrap_err().code());
            let ensure = EnsureWorktree {
                root: root.clone(),
                branch: String::from("ticket/a"),
                path: path.clone(),
            };
            refused(runner.ensure_worktree(&ensure).await.unwrap_err().code());
            let remove = RemoveWorktree {
                root: root.clone(),
                path: path.clone(),
                cwd: local.clone(),
            };
            refused(runner.remove_worktree(&remove).await.unwrap_err().code());
        }
        let new = NewWorktree {
            cwd: share.clone(),
            root: local.clone(),
            branch: String::from("ticket/a"),
            path: local.clone(),
            target: None,
        };
        refused(runner.create_worktree(&new).await.unwrap_err().code());
        refused(
            runner
                .review_changes(&review(&share))
                .await
                .unwrap_err()
                .code(),
        );
        refused(
            runner
                .review_patch(&review(&share), "a.txt")
                .await
                .unwrap_err()
                .code(),
        );
        refused(
            runner
                .review_file(&share, "ticket/a", "a.txt")
                .await
                .unwrap_err()
                .code(),
        );
        refused(
            runner
                .session_changes(&changes(&share))
                .await
                .unwrap_err()
                .code(),
        );
        let diff = DiffRequest {
            changes: changes(&share),
            path: String::from("a.txt"),
        };
        refused(runner.session_diff(&diff).await.unwrap_err().code());
    }

    fn text_of(path: &std::path::Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stored_share_is_reported_missing_without_looking_unless_its_host_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        // two leading slashes name a host to the check and the first folder to this system
        let doubled = format!("/{}", dir.path().display());
        let host = doubled
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap()
            .to_owned();
        let paths = [doubled, text_of(dir.path())];
        let closed = runner(dir.path());
        assert_eq!(closed.folders_exist(&paths).await.unwrap(), [false, true]);
        let listed = runner(dir.path()).with_network_shares(NetworkShares::new([host]));
        assert_eq!(listed.folders_exist(&paths).await.unwrap(), [true, true]);
    }
}
