#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests fail loudly"
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::repositories::{Located, not_a_git_project};
use nullradix_app::review::{
    BranchChanges, BranchFile, FilePatch, ReviewRequest, review_not_available,
};
use nullradix_app::runs::{
    RunEnv, RunProgress, RunReport, RunRepository, RunService, RunSpec, RunnerPort,
};
use nullradix_app::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary, change_not_found,
};
use nullradix_app::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, WorktreeRemoval,
};
use nullradix_app::{AppError, ChangeSignal, SystemClock, UuidV7Generator};
use nullradix_domain::{Id, Run, RunEnd, RunLabel, RunOutcome, RunStatus, Timestamp};
use nullradix_store::Store;

/// Report and liveness of a scripted run.
type Script = (Option<RunReport>, bool);

/// Runner double whose processes the test finishes by hand.
#[derive(Default)]
struct ScriptedRunner {
    reject: Mutex<bool>,
    refuse: Mutex<Option<String>>,
    runs: Mutex<HashMap<Id<Run>, Script>>,
    cancelled: Mutex<Vec<Id<Run>>>,
}

impl ScriptedRunner {
    fn finish(&self, id: Id<Run>, outcome: RunOutcome, exit_code: i32) {
        let report = RunReport {
            outcome,
            end: RunEnd {
                exit_code: Some(exit_code),
                error: None,
            },
            claude: None,
        };
        self.runs.lock().unwrap().insert(id, (Some(report), false));
    }

    fn vanish(&self, id: Id<Run>) {
        self.runs.lock().unwrap().insert(id, (None, false));
    }

    fn keep_alive(&self, id: Id<Run>) {
        self.runs.lock().unwrap().insert(id, (None, true));
    }
}

#[async_trait]
impl RunnerPort for ScriptedRunner {
    async fn check(&self, _spec: &RunSpec) -> Result<(), AppError> {
        if *self.reject.lock().unwrap() {
            return Err(AppError::invalid("run-cwd-invalid", "no such directory"));
        }
        Ok(())
    }

    async fn start(&self, run: Id<Run>, _spec: &RunSpec) -> Result<u32, AppError> {
        if let Some(reason) = self.refuse.lock().unwrap().clone() {
            return Err(AppError::unavailable("runner-failed", reason));
        }
        self.runs.lock().unwrap().insert(run, (None, true));
        Ok(4242)
    }

    async fn cancel(&self, run: Id<Run>) -> Result<(), AppError> {
        self.cancelled.lock().unwrap().push(run);
        Ok(())
    }

    async fn progress(
        &self,
        run: Id<Run>,
        _after: i64,
        _limit: u32,
    ) -> Result<RunProgress, AppError> {
        let (report, alive) = self
            .runs
            .lock()
            .unwrap()
            .get(&run)
            .cloned()
            .unwrap_or((None, false));
        Ok(RunProgress {
            lines: Vec::new(),
            report,
            alive,
        })
    }

    async fn locate(&self, path: &str) -> Result<Located, AppError> {
        Err(not_a_git_project(path))
    }

    async fn create_worktree(&self, _request: &NewWorktree) -> Result<CreatedWorktree, AppError> {
        Err(AppError::unavailable("runner-failed", "no git here"))
    }

    async fn folders_exist(&self, paths: &[String]) -> Result<Vec<bool>, AppError> {
        Ok(vec![false; paths.len()])
    }

    async fn ensure_worktree(
        &self,
        _request: &EnsureWorktree,
    ) -> Result<EnsuredWorktree, AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }

    async fn remove_worktree(
        &self,
        _request: &RemoveWorktree,
    ) -> Result<WorktreeRemoval, AppError> {
        Err(AppError::unavailable("runner-failed", "runs nothing"))
    }
    async fn read_transcript(
        &self,
        _request: &TranscriptRead,
    ) -> Result<Option<TranscriptChunk>, AppError> {
        Ok(None)
    }

    async fn transcript_facts(
        &self,
        _source: &TranscriptSource,
    ) -> Result<Option<TranscriptSummary>, AppError> {
        Ok(None)
    }

    async fn session_changes(&self, _request: &ChangeRequest) -> Result<SessionChanges, AppError> {
        Ok(SessionChanges::none())
    }

    async fn session_diff(&self, request: &DiffRequest) -> Result<FileDiff, AppError> {
        Err(change_not_found(&request.path))
    }

    async fn review_changes(&self, _request: &ReviewRequest) -> Result<BranchChanges, AppError> {
        Err(review_not_available("this runner reads no branches"))
    }

    async fn review_patch(
        &self,
        _request: &ReviewRequest,
        _path: &str,
    ) -> Result<FilePatch, AppError> {
        Err(review_not_available("this runner reads no branches"))
    }

    async fn review_file(
        &self,
        _root: &str,
        _branch: &str,
        _path: &str,
    ) -> Result<BranchFile, AppError> {
        Err(review_not_available("this runner reads no branches"))
    }
}

struct Fixture {
    runs: Arc<RunService>,
    runner: Arc<ScriptedRunner>,
    store: Arc<Store>,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let runner = Arc::new(ScriptedRunner::default());
    let runs = RunService::new(
        store.clone(),
        runner.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
        Duration::from_millis(20),
    );
    Fixture {
        runs,
        runner,
        store,
    }
}

fn spec() -> RunSpec {
    RunSpec {
        program: String::from("echo"),
        args: vec![String::from("hi")],
        cwd: std::env::temp_dir(),
        timeout: None,
        env: RunEnv::default(),
        stdin: None,
        claude: false,
    }
}

async fn wait_for(runs: &RunService, id: Id<Run>, status: RunStatus) -> Run {
    for _ in 0..200 {
        let run = runs.get(id).await.unwrap();
        if run.status() == status {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run never reached {status:?}");
}

#[tokio::test]
async fn a_started_run_is_watched_until_it_finishes() {
    let fx = fixture().await;
    let run = fx.runs.start("echo hi", &spec()).await.unwrap();
    assert_eq!(run.status(), RunStatus::Running);
    assert_eq!(run.pid(), Some(4242));

    fx.runner.finish(run.id(), RunOutcome::Failed, 3);
    let done = wait_for(&fx.runs, run.id(), RunStatus::Failed).await;
    assert_eq!(done.exit_code(), Some(3));
    assert!(done.finished_at().is_some());
}

#[tokio::test]
async fn a_run_whose_supervisor_vanished_is_lost() {
    let fx = fixture().await;
    let run = fx.runs.start("echo hi", &spec()).await.unwrap();
    fx.runner.vanish(run.id());
    wait_for(&fx.runs, run.id(), RunStatus::Lost).await;
}

#[tokio::test]
async fn a_refused_start_is_recorded_as_failed() {
    let fx = fixture().await;
    *fx.runner.refuse.lock().unwrap() = Some(String::from("no supervisor"));
    let error = fx.runs.start("echo hi", &spec()).await.unwrap_err();
    assert_eq!(error.code(), "runner-failed");
    let recorded = fx.runs.list(10).await.unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].status(), RunStatus::Failed);
    assert_eq!(recorded[0].error(), Some("no supervisor"));
}

#[tokio::test]
async fn cancelling_asks_the_runner_and_finished_runs_refuse() {
    let fx = fixture().await;
    let run = fx.runs.start("echo hi", &spec()).await.unwrap();
    fx.runs.cancel(run.id()).await.unwrap();
    assert_eq!(*fx.runner.cancelled.lock().unwrap(), vec![run.id()]);

    fx.runner.finish(run.id(), RunOutcome::Cancelled, 1);
    wait_for(&fx.runs, run.id(), RunStatus::Cancelled).await;
    let again = fx.runs.cancel(run.id()).await.unwrap_err();
    assert_eq!(again.code(), "run-finished");
}

fn record(label: &str) -> Run {
    Run::new(
        Id::from_uuid(uuid::Uuid::now_v7()),
        RunLabel::parse(label).unwrap(),
        Timestamp::from_millis(1),
    )
}

#[tokio::test]
async fn recovery_settles_runs_from_before_the_restart() {
    let fx = fixture().await;
    let running = record("survivor")
        .started(4242, Timestamp::from_millis(2))
        .unwrap();
    // the daemon died between starting the supervisor and saving its pid
    let half_started = record("half started");
    let orphan = record("orphan");
    for run in [&running, &half_started, &orphan] {
        fx.store.insert(run).await.unwrap();
    }
    fx.runner.keep_alive(running.id());
    fx.runner.keep_alive(half_started.id());

    assert_eq!(fx.runs.recover().await.unwrap(), 3);
    wait_for(&fx.runs, orphan.id(), RunStatus::Lost).await;
    fx.runner.finish(running.id(), RunOutcome::Succeeded, 0);
    fx.runner.finish(half_started.id(), RunOutcome::Failed, 2);
    wait_for(&fx.runs, running.id(), RunStatus::Succeeded).await;
    let failed = wait_for(&fx.runs, half_started.id(), RunStatus::Failed).await;
    assert_eq!(failed.exit_code(), Some(2));
}

#[tokio::test]
async fn invalid_input_is_rejected_without_a_record() {
    let fx = fixture().await;
    let error = fx.runs.start("  ", &spec()).await.unwrap_err();
    assert_eq!(error.code(), "run-label-invalid");
    let relative = RunSpec {
        cwd: std::path::PathBuf::from("relative"),
        ..spec()
    };
    let error = fx.runs.start("echo hi", &relative).await.unwrap_err();
    assert_eq!(error.code(), "run-cwd-invalid");
    *fx.runner.reject.lock().unwrap() = true;
    let error = fx.runs.start("echo hi", &spec()).await.unwrap_err();
    assert_eq!(error.code(), "run-cwd-invalid");
    assert_eq!(fx.runs.list(10).await.unwrap(), Vec::new());
}
