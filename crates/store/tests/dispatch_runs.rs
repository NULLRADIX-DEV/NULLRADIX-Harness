#![allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::panic,
    reason = "integration tests fail loudly"
)]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nullradix_app::accounts::{AccountFolders, AccountService};
use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::dispatch::runs::{DispatchRuns, RunFilesPort, RunSubject, StartDispatchRun};
use nullradix_app::dispatch::stream::{ClaudeStream, Pipe};
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::projects::ProjectService;
use nullradix_app::repositories::Located;
use nullradix_app::runs::{
    ClaudeReport, OutputLine, OutputStream, RunLink, RunProgress, RunReport, RunRepository,
    RunService, RunSpec, RunnerPort,
};
use nullradix_app::secrets::MemorySecretStore;
use nullradix_app::session_detail::{
    ChangeRequest, DiffRequest, FileDiff, SessionChanges, TranscriptChunk, TranscriptRead,
    TranscriptSource, TranscriptSummary,
};
use nullradix_app::tickets::{NewTicket, TicketService, WriteOptions};
use nullradix_app::worktrees::{
    CreatedWorktree, EnsureWorktree, EnsuredWorktree, NewWorktree, RemoveWorktree, WorktreeRemoval,
};
use nullradix_app::{AppError, ChangeSignal, SystemClock, UuidV7Generator};
use nullradix_domain::{
    AbortCategory, AccountSlug, ConfigDir, Id, Run, RunEnd, RunKind, RunOutcome, RunStatus,
    Screenshot,
};
use nullradix_store::Store;

/// What the test decides about one run.
#[derive(Default)]
struct Script {
    lines: Vec<OutputLine>,
    report: Option<RunReport>,
    alive: bool,
}

/// Runner whose processes the test ends by hand; everything but runs is answered by `FolderRunner`.
#[derive(Default)]
struct ScriptedRunner {
    scripts: Mutex<HashMap<Id<Run>, Script>>,
    specs: Mutex<Vec<RunSpec>>,
    files: Mutex<HashMap<String, Vec<u8>>>,
}

impl ScriptedRunner {
    fn script(&self, id: Id<Run>, script: Script) {
        self.scripts.lock().unwrap().insert(id, script);
    }
}

#[async_trait]
impl RunnerPort for ScriptedRunner {
    async fn check(&self, _spec: &RunSpec) -> Result<(), AppError> {
        Ok(())
    }

    async fn start(&self, run: Id<Run>, spec: &RunSpec) -> Result<u32, AppError> {
        self.specs.lock().unwrap().push(spec.clone());
        self.scripts.lock().unwrap().insert(
            run,
            Script {
                alive: true,
                ..Script::default()
            },
        );
        Ok(4242)
    }

    async fn cancel(&self, _run: Id<Run>) -> Result<(), AppError> {
        Ok(())
    }

    async fn progress(
        &self,
        run: Id<Run>,
        after: i64,
        limit: u32,
    ) -> Result<RunProgress, AppError> {
        let scripts = self.scripts.lock().unwrap();
        let script = scripts.get(&run).unwrap();
        let lines = script
            .lines
            .iter()
            .filter(|line| line.seq > after)
            .take(limit as usize)
            .cloned()
            .collect();
        Ok(RunProgress {
            lines,
            report: script.report.clone(),
            alive: script.alive,
        })
    }

    async fn locate(&self, path: &str) -> Result<Located, AppError> {
        common::FolderRunner.locate(path).await
    }

    async fn create_worktree(&self, request: &NewWorktree) -> Result<CreatedWorktree, AppError> {
        common::FolderRunner.create_worktree(request).await
    }

    async fn folders_exist(&self, paths: &[String]) -> Result<Vec<bool>, AppError> {
        common::FolderRunner.folders_exist(paths).await
    }

    async fn ensure_worktree(&self, request: &EnsureWorktree) -> Result<EnsuredWorktree, AppError> {
        common::FolderRunner.ensure_worktree(request).await
    }

    async fn remove_worktree(&self, request: &RemoveWorktree) -> Result<WorktreeRemoval, AppError> {
        common::FolderRunner.remove_worktree(request).await
    }

    async fn read_transcript(
        &self,
        request: &TranscriptRead,
    ) -> Result<Option<TranscriptChunk>, AppError> {
        common::FolderRunner.read_transcript(request).await
    }

    async fn transcript_facts(
        &self,
        source: &TranscriptSource,
    ) -> Result<Option<TranscriptSummary>, AppError> {
        common::FolderRunner.transcript_facts(source).await
    }

    async fn session_changes(&self, request: &ChangeRequest) -> Result<SessionChanges, AppError> {
        common::FolderRunner.session_changes(request).await
    }

    async fn session_diff(&self, request: &DiffRequest) -> Result<FileDiff, AppError> {
        common::FolderRunner.session_diff(request).await
    }
}

#[async_trait]
impl RunFilesPort for ScriptedRunner {
    async fn read_screenshot(
        &self,
        _run: Id<Run>,
        file: &str,
    ) -> Result<Option<Vec<u8>>, AppError> {
        Ok(self.files.lock().unwrap().get(file).cloned())
    }
}

struct NoFolders;

#[async_trait]
impl AccountFolders for NoFolders {
    fn root(&self) -> String {
        String::from("/accounts")
    }

    async fn create(&self, slug: &AccountSlug) -> Result<ConfigDir, AppError> {
        Ok(ConfigDir::from_stored(format!(
            "/accounts/{}",
            slug.as_str()
        )))
    }

    async fn check(&self, _dir: &ConfigDir) -> Result<(), AppError> {
        Ok(())
    }

    fn key(&self, dir: &ConfigDir) -> String {
        dir.as_str().to_owned()
    }

    fn slug_key(&self, slug: &AccountSlug) -> String {
        format!("/accounts/{}", slug.as_str())
    }

    fn owns(&self, _dir: &ConfigDir) -> bool {
        true
    }

    async fn remove(&self, _dir: &ConfigDir) -> Result<(), AppError> {
        Ok(())
    }
}

struct Fixture {
    store: Arc<Store>,
    runner: Arc<ScriptedRunner>,
    runs: Arc<RunService>,
    dispatch: DispatchRuns,
    config: Arc<DispatcherConfigService>,
}

fn run_service(
    store: &Arc<Store>,
    runner: &Arc<ScriptedRunner>,
    signal: &ChangeSignal,
) -> Arc<RunService> {
    RunService::new(
        store.clone(),
        runner.clone(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        signal.clone(),
        Duration::from_millis(20),
    )
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let clock = Arc::new(SystemClock);
    let ids = Arc::new(UuidV7Generator);
    let runner = Arc::new(ScriptedRunner::default());
    let runners = common::runners(&store, &signal).await;
    let person = local_person(store.as_ref(), "anna", ids.as_ref(), clock.as_ref())
        .await
        .unwrap();
    let projects = ProjectService::new(store.clone(), clock.clone(), ids.clone(), signal.clone());
    let boards = BoardService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        runners,
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let project = projects.create("ARC", "Arches").await.unwrap();
    let board = boards
        .create(project.id(), "Main", &person)
        .await
        .unwrap()
        .board;
    let tickets = TicketService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    for title in ["Build the login", "Fix the logout"] {
        let input = NewTicket {
            title: title.to_owned(),
            ..NewTicket::default()
        };
        let created = tickets
            .create(
                project.id(),
                board.id(),
                &input,
                &person,
                WriteOptions::default(),
            )
            .await
            .unwrap();
        assert!(created.ticket.key().to_string().starts_with("ARC-"));
    }
    let config = Arc::new(DispatcherConfigService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let accounts = Arc::new(AccountService::new(
        store.clone(),
        Arc::new(MemorySecretStore::new()),
        Arc::new(NoFolders),
        clock,
        ids,
        signal.clone(),
    ));
    let runs = run_service(&store, &runner, &signal);
    let dispatch = DispatchRuns::new(
        runs.clone(),
        config.clone(),
        accounts,
        store.clone(),
        store.clone(),
        runner.clone(),
    );
    Fixture {
        store,
        runner,
        runs,
        dispatch,
        config,
    }
}

fn request(subject: &str) -> StartDispatchRun {
    StartDispatchRun {
        kind: RunKind::Work,
        trigger: nullradix_domain::RunTrigger::Dispatcher,
        subject: subject.to_owned(),
        prompt: String::from("Implement the ticket."),
        cwd: std::env::temp_dir(),
        profile: None,
        account: None,
        extra_tools: Vec::new(),
        agent: None,
    }
}

fn line(seq: i64, stream: OutputStream, text: &str) -> OutputLine {
    OutputLine {
        seq,
        stream,
        text: text.to_owned(),
        continues: false,
    }
}

/// Output of a clean run as the fake Claude writes it.
fn success_lines() -> Vec<OutputLine> {
    let events = [
        r#"{"type":"system","subtype":"init","model":"fake-opus","session_id":"fake-session-1"}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Working."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"a.rs"}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"num_turns":7,"duration_ms":61000,"total_cost_usd":0.4321,"session_id":"fake-session-1","usage":{"input_tokens":1200,"output_tokens":340,"cache_read_input_tokens":9000,"cache_creation_input_tokens":150},"modelUsage":{"fake-haiku":{"costUSD":0.01},"fake-opus":{"inputTokens":1190,"costUSD":0.4221}},"permission_denials":[{"tool_name":"Bash","tool_input":{"command":"git push"}},{"tool_name":"Bash","tool_input":{"command":"git push"}}]}"#,
    ];
    events
        .iter()
        .zip(1..)
        .map(|(text, seq)| line(seq, OutputStream::Stdout, text))
        .collect()
}

fn claude_report(lines: &[OutputLine], screenshots: Vec<Screenshot>) -> ClaudeReport {
    let mut stream = ClaudeStream::new();
    for line in lines {
        let pipe = match line.stream {
            OutputStream::Stdout => Pipe::Stdout,
            OutputStream::Stderr => Pipe::Stderr,
        };
        stream.feed(pipe, &line.text);
    }
    ClaudeReport {
        output: stream.finish(),
        screenshots,
    }
}

fn report(outcome: RunOutcome, code: i32, claude: ClaudeReport) -> RunReport {
    RunReport {
        outcome,
        end: RunEnd {
            exit_code: Some(code),
            error: None,
        },
        claude: Some(claude),
    }
}

async fn wait_until_finished(runs: &RunService, id: Id<Run>) -> Run {
    for _ in 0..200 {
        let run = runs.get(id).await.unwrap();
        if !run.status().is_active() {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not finish");
}

#[tokio::test]
async fn a_started_run_records_its_settings_and_hands_the_prompt_over() {
    let fx = fixture().await;
    let run = fx.dispatch.start(request("arc-1")).await.unwrap();
    assert_eq!(run.status(), RunStatus::Running);
    let dispatch = run.dispatch().unwrap();
    assert_eq!(dispatch.kind, RunKind::Work);
    assert_eq!(dispatch.subject_key.as_deref(), Some("ARC-0001"));
    assert_eq!(dispatch.profile.name, "medium");
    assert_eq!(dispatch.profile.model.as_deref(), Some("opus"));
    assert_eq!(dispatch.profile.max_turns, 80);
    assert_eq!(dispatch.prompt, "Implement the ticket.");
    assert_eq!(run.label().as_str(), "work ARC-0001");
    assert_eq!(dispatch.config_hash, fx.config.get().await.unwrap().hash);

    let stored = fx.runs.get(run.id()).await.unwrap();
    assert_eq!(stored.dispatch(), run.dispatch());
    let spec = fx.runner.specs.lock().unwrap().pop().unwrap();
    assert!(spec.claude);
    assert_eq!(spec.stdin.as_deref(), Some("Implement the ticket."));
    assert_eq!(spec.timeout, Some(Duration::from_mins(90)));
    assert_eq!(spec.env.set["NULLRADIX_TICKET"], "ARC-0001");
    assert_eq!(spec.env.set["NULLRADIX_RUN"], run.id().to_string());
    assert_eq!(spec.args, dispatch.args);
    assert_eq!(spec.args[0], "-p");
}

#[tokio::test]
async fn usage_denials_and_screenshots_are_recorded_when_the_run_ends() {
    let fx = fixture().await;
    let run = fx.dispatch.start(request("ARC-0001")).await.unwrap();
    let lines = success_lines();
    let shot = Screenshot {
        tool_use_id: Some(String::from("t1")),
        media_type: String::from("image/png"),
        file: String::from("shot-1.png"),
        bytes: 3,
    };
    fx.runner
        .files
        .lock()
        .unwrap()
        .insert(String::from("shot-1.png"), vec![1, 2, 3]);
    fx.runner.script(
        run.id(),
        Script {
            report: Some(report(
                RunOutcome::Succeeded,
                0,
                claude_report(&lines, vec![shot]),
            )),
            lines,
            alive: false,
        },
    );
    let done = wait_until_finished(&fx.runs, run.id()).await;
    assert_eq!(done.status(), RunStatus::Succeeded);
    let result = done.result().unwrap();
    assert_eq!(result.session_id.as_deref(), Some("fake-session-1"));
    assert_eq!(result.abort, None);
    assert_eq!(result.denied_count, Some(2));
    let usage = result.usage.as_ref().unwrap();
    assert_eq!(usage.cost_usd, Some(0.4321));
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.num_turns),
        (Some(1200), Some(340), Some(7))
    );
    assert_eq!(
        (usage.cache_read_tokens, usage.cache_write_tokens),
        (Some(9000), Some(150))
    );
    assert_eq!(usage.model.as_deref(), Some("fake-opus"));
    assert_eq!(usage.models.len(), 2);

    let detail = fx.dispatch.detail(run.id()).await.unwrap();
    assert_eq!(detail.run, done);
    assert_eq!(detail.denied.len(), 1);
    assert_eq!(detail.denied[0].tool, "Bash");
    assert_eq!(detail.denied[0].count, 2);
    assert_eq!(detail.artifacts.len(), 1);
    let (artifact, bytes) = fx
        .dispatch
        .screenshot(run.id(), detail.artifacts[0].id)
        .await
        .unwrap();
    assert_eq!(artifact.screenshot.media_type, "image/png");
    assert_eq!(bytes, vec![1, 2, 3]);
    assert_eq!(
        fx.dispatch
            .screenshot(run.id(), uuid::Uuid::now_v7())
            .await
            .unwrap_err()
            .code(),
        "run-artifact-not-found"
    );
}

#[tokio::test]
async fn failed_runs_record_why_they_aborted() {
    let fx = fixture().await;
    let limit = fx.dispatch.start(request("ARC-0001")).await.unwrap();
    let lines = vec![
        line(
            1,
            OutputStream::Stderr,
            "Claude AI usage limit reached|1760000000",
        ),
        line(
            2,
            OutputStream::Stdout,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"You've hit your session limit"}"#,
        ),
    ];
    fx.runner.script(
        limit.id(),
        Script {
            report: Some(report(
                RunOutcome::Failed,
                1,
                claude_report(&lines, Vec::new()),
            )),
            lines,
            alive: false,
        },
    );
    let done = wait_until_finished(&fx.runs, limit.id()).await;
    assert_eq!(done.status(), RunStatus::Failed);
    let abort = done.result().unwrap().abort.clone().unwrap();
    assert_eq!(abort.category, AbortCategory::UsageLimit);
    assert!(abort.reason.starts_with("usage limit:"));

    let slow = fx.dispatch.start(request("ARC-0002")).await.unwrap();
    fx.runner.script(
        slow.id(),
        Script {
            report: Some(report(
                RunOutcome::TimedOut,
                0,
                claude_report(&[], Vec::new()),
            )),
            ..Script::default()
        },
    );
    let timed_out = wait_until_finished(&fx.runs, slow.id()).await;
    assert_eq!(timed_out.status(), RunStatus::TimedOut);
    let abort = timed_out.result().unwrap().abort.clone().unwrap();
    assert_eq!(abort.category, AbortCategory::Timeout);
    assert_eq!(abort.reason, "time limit of 90 min reached, run stopped");
    assert_eq!(timed_out.result().unwrap().usage, None);
    assert_eq!(timed_out.result().unwrap().denied_count, None);

    let vanished = fx.dispatch.start(request("ARC-0001")).await.unwrap();
    fx.runner.script(vanished.id(), Script::default());
    let lost = wait_until_finished(&fx.runs, vanished.id()).await;
    assert_eq!(lost.status(), RunStatus::Lost);
    assert_eq!(
        lost.result().unwrap().abort.as_ref().unwrap().category,
        AbortCategory::Vanished
    );
}

#[tokio::test]
async fn a_cancelled_run_is_no_abort() {
    let fx = fixture().await;
    let run = fx.dispatch.start(request("ARC-0001")).await.unwrap();
    fx.runner.script(
        run.id(),
        Script {
            report: Some(report(
                RunOutcome::Cancelled,
                0,
                claude_report(&[], Vec::new()),
            )),
            ..Script::default()
        },
    );
    let done = wait_until_finished(&fx.runs, run.id()).await;
    assert_eq!(done.status(), RunStatus::Cancelled);
    assert_eq!(done.result().and_then(|result| result.abort.as_ref()), None);
}

#[tokio::test]
async fn runs_are_listed_by_ticket_kind_and_session() {
    let fx = fixture().await;
    let first = fx.dispatch.start(request("ARC-0001")).await.unwrap();
    let second = fx.dispatch.start(request("ARC-0002")).await.unwrap();
    let merge = fx
        .dispatch
        .start(StartDispatchRun {
            kind: RunKind::Merge,
            ..request("ARC-0001")
        })
        .await
        .unwrap();
    let plain = fx.runs.start("plain", &plain_spec()).await.unwrap();

    let ids = |runs: Vec<Run>| runs.iter().map(Run::id).collect::<Vec<_>>();
    let of = |subject, kind, link| fx.dispatch.list(subject, kind, link, 50);
    assert_eq!(
        ids(of(Some(RunSubject::Ticket("arc-1")), None, None)
            .await
            .unwrap()),
        vec![merge.id(), first.id()]
    );
    assert_eq!(
        ids(of(
            Some(RunSubject::Ticket("ARC-0001")),
            Some(RunKind::Work),
            None
        )
        .await
        .unwrap()),
        vec![first.id()]
    );
    assert_eq!(
        ids(of(Some(RunSubject::Ticket("ARC-0099")), None, None)
            .await
            .unwrap()),
        Vec::new()
    );
    assert_eq!(
        of(Some(RunSubject::Ticket("../x")), None, None)
            .await
            .unwrap_err()
            .code(),
        "ticket-id-invalid"
    );
    assert_eq!(of(None, None, None).await.unwrap().len(), 4);
    assert_eq!(
        ids(of(None, Some(RunKind::Merge), None).await.unwrap()),
        vec![merge.id()]
    );

    // a run is found through its Claude session once the output named it, or through its run id
    let lines = success_lines();
    fx.runner.script(
        second.id(),
        Script {
            report: Some(report(
                RunOutcome::Succeeded,
                0,
                claude_report(&lines, Vec::new()),
            )),
            lines,
            alive: false,
        },
    );
    wait_until_finished(&fx.runs, second.id()).await;
    let by_session = RunLink {
        claude_session: String::from("fake-session-1"),
        run: None,
    };
    assert_eq!(
        ids(of(None, None, Some(by_session)).await.unwrap()),
        vec![second.id()]
    );
    let by_run = RunLink {
        claude_session: String::from("unknown"),
        run: Some(first.id()),
    };
    assert_eq!(
        ids(of(None, None, Some(by_run)).await.unwrap()),
        vec![first.id()]
    );
    assert!(plain.dispatch().is_none());
}

fn plain_spec() -> RunSpec {
    RunSpec {
        program: String::from("echo"),
        args: Vec::new(),
        cwd: std::env::temp_dir(),
        timeout: None,
        env: nullradix_app::runs::RunEnv::default(),
        stdin: None,
        claude: false,
    }
}

#[tokio::test]
async fn bad_requests_leave_no_run_behind() {
    let fx = fixture().await;
    let missing = fx.dispatch.start(request("ARC-0099")).await.unwrap_err();
    assert_eq!(missing.code(), "ticket-not-found");
    let invalid = fx.dispatch.start(request("nonsense")).await.unwrap_err();
    assert_eq!(invalid.code(), "ticket-id-invalid");
    let research = fx
        .dispatch
        .start(StartDispatchRun {
            kind: RunKind::Research,
            ..request("ARC-0001")
        })
        .await
        .unwrap_err();
    assert_eq!(research.code(), "question-id-invalid");
    let question = fx
        .dispatch
        .start(StartDispatchRun {
            kind: RunKind::Research,
            ..request("ARC-Q0001")
        })
        .await
        .unwrap_err();
    assert_eq!(question.code(), "question-not-found");
    let empty = fx
        .dispatch
        .start(StartDispatchRun {
            prompt: String::from("  "),
            ..request("ARC-0001")
        })
        .await
        .unwrap_err();
    assert_eq!(empty.code(), "run-prompt-invalid");
    let profile = fx
        .dispatch
        .start(StartDispatchRun {
            profile: Some(String::from("giant")),
            ..request("ARC-0001")
        })
        .await
        .unwrap_err();
    assert_eq!(profile.code(), "dispatch-profile-unknown");
    let named = fx
        .dispatch
        .start(StartDispatchRun {
            profile: Some(String::from("small")),
            ..request("ARC-0001")
        })
        .await
        .unwrap();
    assert_eq!(named.dispatch().unwrap().profile.name, "small");
    assert_eq!(fx.runs.list(10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_transcript_formats_the_output_in_pages() {
    let fx = fixture().await;
    let run = fx.dispatch.start(request("ARC-0001")).await.unwrap();
    let lines = success_lines();
    fx.runner.script(
        run.id(),
        Script {
            lines,
            alive: true,
            report: None,
        },
    );
    let page = fx.dispatch.transcript(run.id(), 0, 2).await.unwrap();
    let kinds: Vec<&str> = page
        .entries
        .iter()
        .map(|entry| entry.line.kind.as_str())
        .collect();
    assert_eq!(kinds, ["start", "say", "read"]);
    assert_eq!((page.last_seq, page.finished), (2, false));
    let rest = fx
        .dispatch
        .transcript(run.id(), page.last_seq, 10)
        .await
        .unwrap();
    assert_eq!(rest.entries.len(), 2);
    assert_eq!(rest.last_seq, 4);
    // still running, so a reader must come back
    assert!(!rest.finished);
    fx.runner
        .scripts
        .lock()
        .unwrap()
        .get_mut(&run.id())
        .unwrap()
        .report = Some(report(
        RunOutcome::Succeeded,
        0,
        claude_report(&[], Vec::new()),
    ));
    let end = fx.dispatch.transcript(run.id(), 4, 10).await.unwrap();
    assert!(end.entries.is_empty() && end.finished);
}

#[tokio::test]
async fn a_dispatch_run_that_outlives_the_daemon_is_settled_by_the_next_one() {
    let fx = fixture().await;
    let run = fx.dispatch.start(request("ARC-0001")).await.unwrap();
    // the first service stops watching only with its process, so a second one picks the run up
    let signal = ChangeSignal::new();
    let restarted = run_service(&fx.store, &fx.runner, &signal);
    assert_eq!(restarted.recover().await.unwrap(), 1);
    let lines = success_lines();
    fx.runner.script(
        run.id(),
        Script {
            report: Some(report(
                RunOutcome::Succeeded,
                0,
                claude_report(&lines, Vec::new()),
            )),
            lines,
            alive: false,
        },
    );
    let done = wait_until_finished(&restarted, run.id()).await;
    assert_eq!(done.status(), RunStatus::Succeeded);
    assert_eq!(
        done.result().unwrap().usage.as_ref().unwrap().cost_usd,
        Some(0.4321)
    );
    let stored = RunRepository::get(fx.store.as_ref(), run.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.result(), done.result());
}
