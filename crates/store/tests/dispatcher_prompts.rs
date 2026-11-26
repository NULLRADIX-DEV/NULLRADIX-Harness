#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::Author;
use nullradix_app::dispatcher_config::DispatcherConfigService;
use nullradix_app::dispatcher_prompts::{DispatcherPromptService, PreviewRequest};
use nullradix_app::projects::ProjectService;
use nullradix_app::questions::{Answer, NewQuestion, QuestionService};
use nullradix_app::tickets::{NewTicket, TicketService, WriteOptions};
use nullradix_app::{AppError, ChangeSignal, ErrorKind, SystemClock, UuidV7Generator};
use nullradix_domain::{
    Actor, Board, OptionDraft, PromptKind, Version, WorktreeSetupStep, default_template,
};
use nullradix_store::Store;

struct Fixture {
    prompts: DispatcherPromptService,
    config: Arc<DispatcherConfigService>,
    tickets: Arc<TicketService>,
    questions: Arc<QuestionService>,
    person: Actor,
    board: Board,
}

async fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let signal = ChangeSignal::new();
    let person = local_person(store.as_ref(), "anna", &UuidV7Generator, &SystemClock)
        .await
        .unwrap();
    let clock = Arc::new(SystemClock);
    let ids = Arc::new(UuidV7Generator);
    let projects = ProjectService::new(store.clone(), clock.clone(), ids.clone(), signal.clone());
    let boards = BoardService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        common::runners(&store, &signal).await,
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
    let tickets = Arc::new(TicketService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let questions = Arc::new(QuestionService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    let config = Arc::new(DispatcherConfigService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    ));
    Fixture {
        prompts: DispatcherPromptService::new(config.clone(), tickets.clone(), questions.clone()),
        config,
        tickets,
        questions,
        person,
        board,
    }
}

impl Fixture {
    async fn ticket(&self, title: &str, labels: &[&str], target: Option<&str>) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            acceptance: Some(String::from("works")),
            labels: labels.iter().map(|label| (*label).to_owned()).collect(),
            target_branch: target.map(str::to_owned),
            ..NewTicket::default()
        };
        self.tickets
            .create(
                self.board.project(),
                self.board.id(),
                &input,
                &self.person,
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .ticket
            .key()
            .to_string()
    }

    async fn decided_question(&self, ticket: &str) -> String {
        let input = NewQuestion {
            title: String::from("Which store?"),
            tickets: vec![ticket.to_owned()],
            options: vec![
                OptionDraft {
                    title: String::from("File"),
                    text: String::from("File text"),
                    ..OptionDraft::default()
                },
                OptionDraft {
                    title: String::from("Db"),
                    text: String::from("Db text"),
                    ..OptionDraft::default()
                },
            ],
            ..NewQuestion::default()
        };
        let agent = Author {
            by_person: false,
            name: Some("claude"),
            session: Some("s-1"),
        };
        let key = self
            .questions
            .create(self.board.project(), &input, agent, &self.person)
            .await
            .unwrap()
            .view
            .row
            .question
            .key()
            .to_string();
        let answer = Answer {
            option: Some(String::from("O2")),
            reason: Some(String::from("fits")),
            ..Answer::default()
        };
        let by_person = Author {
            by_person: true,
            name: None,
            session: None,
        };
        self.questions
            .answer(&key, &answer, by_person, &self.person, None)
            .await
            .unwrap();
        key
    }
}

fn code(error: &AppError) -> (ErrorKind, &'static str) {
    (error.kind(), error.code())
}

fn request(kind: PromptKind, key: Option<&str>, template: Option<&str>) -> PreviewRequest {
    PreviewRequest {
        kind,
        key: key.map(str::to_owned),
        template: template.map(str::to_owned),
    }
}

const OWN_PLAN: &str = "Plan {{ticketId}} as {{agent}}.\nSave plan.json with: nullradix plan import {{ticketId}} --file plan.json";

#[tokio::test]
async fn lists_the_four_kinds_with_their_defaults() {
    let f = fixture().await;
    let list = f.prompts.list().await.unwrap();
    assert_eq!(list.version, Version::from_stored(1));
    let kinds: Vec<PromptKind> = list.items.iter().map(|item| item.kind).collect();
    assert_eq!(kinds, PromptKind::ALL);
    for item in &list.items {
        assert_eq!(item.default, default_template(item.kind));
        assert_eq!(item.template, None);
        assert!(item.placeholders.len() > 3);
        assert!(item.required.len() > 3);
    }
}

#[tokio::test]
async fn an_own_template_is_stored_per_kind_and_reset_goes_back_to_the_default() {
    let f = fixture().await;
    let before = f.config.get().await.unwrap();
    let saved = f
        .prompts
        .set(
            PromptKind::Plan,
            &OWN_PLAN.replace('\n', "\r\n"),
            before.version,
        )
        .await
        .unwrap();
    assert_eq!(saved.version, before.version.next());
    assert_eq!(saved.prompt.template.as_deref(), Some(OWN_PLAN));
    let work = f.prompts.get(PromptKind::Work).await.unwrap();
    assert_eq!(work.prompt.template, None);
    let stored = f.config.get().await.unwrap();
    assert_eq!(stored.config.prompts.plan.as_deref(), Some(OWN_PLAN));
    assert_ne!(stored.hash, before.hash);

    let reset = f
        .prompts
        .reset(PromptKind::Plan, saved.version)
        .await
        .unwrap();
    assert_eq!(reset.prompt.template, None);
    assert_eq!(reset.version, saved.version.next());
    let again = f
        .prompts
        .reset(PromptKind::Plan, reset.version)
        .await
        .unwrap();
    assert_eq!(again.version, reset.version);
}

#[tokio::test]
async fn a_blank_template_resets_and_the_rest_of_the_configuration_stays() {
    let f = fixture().await;
    let mut config = f.config.get().await.unwrap();
    config.config.worktree_setup = vec![WorktreeSetupStep {
        cwd: String::from("web"),
        run: String::from("npm ci"),
    }];
    let config = f
        .config
        .update(config.config, config.version)
        .await
        .unwrap();
    let saved = f
        .prompts
        .set(PromptKind::Plan, OWN_PLAN, config.version)
        .await
        .unwrap();
    let blank = f
        .prompts
        .set(PromptKind::Plan, "  \r\n ", saved.version)
        .await
        .unwrap();
    assert_eq!(blank.prompt.template, None);
    let stored = f.config.get().await.unwrap().config;
    assert_eq!(stored.worktree_setup.len(), 1);
}

#[tokio::test]
async fn a_template_with_unknown_placeholders_or_missing_parts_is_refused() {
    let f = fixture().await;
    let version = f.config.get().await.unwrap().version;
    let error = f
        .prompts
        .set(PromptKind::Plan, "Plan {{ticketId}} {{nope}}", version)
        .await
        .unwrap_err();
    assert_eq!(
        code(&error),
        (ErrorKind::Invalid, "dispatcher-prompt-invalid")
    );
    let messages: Vec<&str> = error
        .fields()
        .iter()
        .map(|issue| issue.message.as_str())
        .collect();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], "uses unknown placeholders {{nope}}");
    assert!(messages[1].starts_with("lacks required parts: Placeholder {{agent}}"));
    assert!(error.fields().iter().all(|issue| issue.field == "template"));
    assert_eq!(f.config.get().await.unwrap().version, version);
}

#[tokio::test]
async fn a_huge_template_is_refused_fast_when_set_or_previewed() {
    let f = fixture().await;
    let version = f.config.get().await.unwrap().version;
    let huge = "--author ".repeat(2 * 1024 * 1024 / 9);
    let started = std::time::Instant::now();
    let error = f
        .prompts
        .set(PromptKind::Work, &huge, version)
        .await
        .unwrap_err();
    assert_eq!(
        code(&error),
        (ErrorKind::Invalid, "dispatcher-prompt-invalid")
    );
    assert_eq!(error.fields()[0].field, "template");
    assert_eq!(error.fields()[0].message, "is longer than 64 KiB");
    let error = f
        .prompts
        .preview(&PreviewRequest {
            kind: PromptKind::Work,
            key: None,
            template: Some(huge),
        })
        .await
        .unwrap_err();
    assert_eq!(error.fields()[0].message, "is longer than 64 KiB");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(f.config.get().await.unwrap().version, version);
}

#[tokio::test]
async fn a_stale_version_is_a_conflict() {
    let f = fixture().await;
    let version = f.config.get().await.unwrap().version;
    f.prompts
        .set(PromptKind::Plan, OWN_PLAN, version)
        .await
        .unwrap();
    let error = f
        .prompts
        .set(PromptKind::Plan, OWN_PLAN, version)
        .await
        .unwrap_err();
    assert_eq!(code(&error), (ErrorKind::Conflict, "version-conflict"));
    let error = f
        .prompts
        .reset(PromptKind::Plan, version)
        .await
        .unwrap_err();
    assert_eq!(code(&error), (ErrorKind::Conflict, "version-conflict"));
}

#[tokio::test]
async fn a_preview_without_a_key_uses_sample_values() {
    let f = fixture().await;
    for kind in PromptKind::ALL {
        let preview = f.prompts.preview(&request(kind, None, None)).await.unwrap();
        assert!(preview.sample, "{kind:?}");
        assert_eq!(preview.key, None);
        assert_eq!(preview.stand_ins, Vec::<&str>::new());
        assert!(!preview.prompt.contains("{{"), "{kind:?}");
    }
    let work = f
        .prompts
        .preview(&request(PromptKind::Work, None, None))
        .await
        .unwrap();
    assert!(work.prompt.contains("You are worker-ARC-0042"));
    assert!(work.prompt.contains("Sample feedback from the review"));
}

#[tokio::test]
async fn a_work_preview_for_a_ticket_uses_its_facts() {
    let f = fixture().await;
    let key = f.ticket("Fix the login", &["interactive"], None).await;
    let question = f.decided_question(&key).await;
    let mut config = f.config.get().await.unwrap();
    config.config.autopilot.enabled = true;
    config.config.prompt_extra = String::from("House rule: no any.");
    config.config.worktree_setup = vec![WorktreeSetupStep {
        cwd: String::from("."),
        run: String::from("npm ci"),
    }];
    f.config
        .update(config.config, config.version)
        .await
        .unwrap();

    let preview = f
        .prompts
        .preview(&request(PromptKind::Work, Some(&key.to_lowercase()), None))
        .await
        .unwrap();
    assert!(!preview.sample);
    assert_eq!(preview.key.as_deref(), Some(key.as_str()));
    assert_eq!(preview.stand_ins, Vec::<&str>::new());
    let text = preview.prompt;
    assert!(text.contains(&format!("You are worker-{key}, a junior developer agent.")));
    assert!(text.contains("branch ticket/ARC-0001-fix-the-login"));
    assert!(text.contains(&format!(
        "- {question} \"Which store?\": option O2 \"Db\" chosen. Reason: fits"
    )));
    assert!(text.contains("(.$ npm ci)"));
    assert!(text.contains("Human Interaction: <what>"));
    assert!(!text.contains("Feedback from the review"));
    assert!(text.ends_with("\n\nHouse rule: no any."));
}

#[tokio::test]
async fn autopilot_follows_the_ticket_labels_in_the_preview() {
    let f = fixture().await;
    let plain = f.ticket("Plain", &[], None).await;
    let mut config = f.config.get().await.unwrap();
    config.config.autopilot.enabled = true;
    f.config
        .update(config.config, config.version)
        .await
        .unwrap();
    let preview = f
        .prompts
        .preview(&request(PromptKind::Work, Some(&plain), None))
        .await
        .unwrap();
    assert!(!preview.prompt.contains("Human Interaction: <what>"));
    assert!(preview.prompt.contains("exactly one of three outcomes"));
}

#[tokio::test]
async fn plan_merge_and_research_previews_use_their_own_facts() {
    let f = fixture().await;
    let key = f.ticket("Fix the login", &[], Some("develop")).await;
    let plan = f
        .prompts
        .preview(&request(PromptKind::Plan, Some(&key), None))
        .await
        .unwrap();
    assert_eq!(plan.stand_ins, Vec::<&str>::new());
    assert!(
        plan.prompt
            .contains(&format!("nullradix plan import {key} --file plan.json"))
    );
    assert!(
        plan.prompt
            .contains(&format!("You are planner-{key}, a planning agent."))
    );

    let merge = f
        .prompts
        .preview(&request(PromptKind::Merge, Some(&key), None))
        .await
        .unwrap();
    assert_eq!(merge.stand_ins, ["previousAttempts"]);
    assert!(
        merge
            .prompt
            .contains("cannot be merged into develop without conflicts")
    );
    assert!(
        merge
            .prompt
            .contains("(preview) The report of earlier failed merge sessions")
    );

    let question = f.decided_question(&key).await;
    let research = f
        .prompts
        .preview(&request(PromptKind::Research, Some(&question), None))
        .await
        .unwrap();
    assert_eq!(research.key.as_deref(), Some(question.as_str()));
    assert!(
        research
            .prompt
            .contains(&format!("You are research-{question}, a research agent."))
    );
    assert!(research.prompt.contains(&format!(
        "nullradix research submit {question} --file research.json"
    )));
}

#[tokio::test]
async fn a_merge_preview_targets_main_when_the_ticket_names_none() {
    let f = fixture().await;
    let key = f.ticket("Fix the login", &[], None).await;
    let merge = f
        .prompts
        .preview(&request(PromptKind::Merge, Some(&key), None))
        .await
        .unwrap();
    assert!(
        merge
            .prompt
            .contains("cannot be merged into main without conflicts")
    );
}

#[tokio::test]
async fn a_draft_template_is_rendered_but_not_saved_and_checked_first() {
    let f = fixture().await;
    let key = f.ticket("Fix the login", &[], None).await;
    let version = f.config.get().await.unwrap().version;
    let preview = f
        .prompts
        .preview(&request(PromptKind::Plan, Some(&key), Some(OWN_PLAN)))
        .await
        .unwrap();
    assert_eq!(
        preview.prompt,
        format!(
            "Plan {key} as planner-{key}.\nSave plan.json with: nullradix plan import {key} --file plan.json"
        )
    );
    assert_eq!(f.config.get().await.unwrap().version, version);
    assert_eq!(
        f.prompts
            .get(PromptKind::Plan)
            .await
            .unwrap()
            .prompt
            .template,
        None
    );

    let error = f
        .prompts
        .preview(&request(
            PromptKind::Plan,
            Some(&key),
            Some("Plan {{ticketId}}"),
        ))
        .await
        .unwrap_err();
    assert_eq!(
        code(&error),
        (ErrorKind::Invalid, "dispatcher-prompt-invalid")
    );
}

#[tokio::test]
async fn a_stored_template_is_what_the_preview_renders() {
    let f = fixture().await;
    let key = f.ticket("Fix the login", &[], None).await;
    let version = f.config.get().await.unwrap().version;
    f.prompts
        .set(PromptKind::Plan, OWN_PLAN, version)
        .await
        .unwrap();
    let preview = f
        .prompts
        .preview(&request(PromptKind::Plan, Some(&key), None))
        .await
        .unwrap();
    assert!(
        preview
            .prompt
            .starts_with(&format!("Plan {key} as planner-{key}."))
    );
}

#[tokio::test]
async fn unknown_tickets_and_questions_are_not_found() {
    let f = fixture().await;
    let error = f
        .prompts
        .preview(&request(PromptKind::Work, Some("ARC-0099"), None))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
    let error = f
        .prompts
        .preview(&request(PromptKind::Research, Some("ARC-Q0099"), None))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
    let error = f
        .prompts
        .preview(&request(PromptKind::Work, Some("nonsense"), None))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Invalid);
}
