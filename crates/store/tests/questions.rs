#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

mod common;

use std::sync::Arc;

use nullradix_app::boards::{BoardService, local_person};
use nullradix_app::comments::{Author, CommentService};
use nullradix_app::projects::ProjectService;
use nullradix_app::questions::{Answer, NewQuestion, QuestionFilter, QuestionService};
use nullradix_app::tickets::{NewTicket, TicketScope, TicketService, WriteOptions};
use nullradix_app::{ChangeFeed, ChangeSignal, Entity, SystemClock, UuidV7Generator};
use nullradix_domain::{
    Actor, Board, Choice, OptionDraft, Project, QuestionOption, QuestionRef, QuestionState, Version,
};
use nullradix_store::Store;

struct Fixture {
    questions: QuestionService,
    tickets: TicketService,
    comments: CommentService,
    person: Actor,
    store: Arc<Store>,
    board: Board,
    other: Board,
}

const PERSON: Author<'static> = Author {
    by_person: true,
    name: None,
    session: None,
};

fn agent(name: &str) -> Author<'_> {
    Author {
        by_person: false,
        name: Some(name),
        session: Some("s-1"),
    }
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
    let mut made: Vec<Board> = Vec::new();
    for (key, name) in [("ARC", "Arches"), ("ZED", "Zed")] {
        let project: Project = projects.create(key, name).await.unwrap();
        made.push(
            boards
                .create(project.id(), "Main", &person)
                .await
                .unwrap()
                .board,
        );
    }
    let other = made.pop().unwrap();
    let board = made.pop().unwrap();
    let tickets = TicketService::new(
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let comments = CommentService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        clock.clone(),
        ids.clone(),
        signal.clone(),
    );
    let questions = QuestionService::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        clock,
        ids,
        signal,
    );
    Fixture {
        questions,
        tickets,
        comments,
        person,
        store,
        board,
        other,
    }
}

fn option(title: &str, recommended: bool) -> OptionDraft {
    OptionDraft {
        title: title.to_owned(),
        text: format!("{title} text"),
        pros: vec![String::from("simple")],
        cons: Vec::new(),
        recommended,
    }
}

impl Fixture {
    async fn ticket(&self, board: &Board, title: &str) -> String {
        let input = NewTicket {
            title: title.to_owned(),
            acceptance: Some(String::from("works")),
            ..NewTicket::default()
        };
        self.tickets
            .create(
                board.project(),
                board.id(),
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

    async fn ask(&self, input: &NewQuestion) -> Result<String, String> {
        self.questions
            .create(self.board.project(), input, agent("claude"), &self.person)
            .await
            .map(|detail| detail.view.row.question.key().to_string())
            .map_err(|error| error.code().to_owned())
    }

    async fn answer(&self, key: &str, answer: &Answer, author: Author<'_>) -> Result<(), String> {
        self.questions
            .answer(key, answer, author, &self.person, None)
            .await
            .map(|_| ())
            .map_err(|error| error.code().to_owned())
    }

    async fn start(&self, key: &str, force: bool) -> Result<(), String> {
        let options = WriteOptions {
            force,
            ..WriteOptions::default()
        };
        self.tickets
            .move_to(key, Some("in-progress"), None, &self.person, None, options)
            .await
            .map(|_| ())
            .map_err(|error| error.code().to_owned())
    }
}

fn titled(title: &str) -> NewQuestion {
    NewQuestion {
        title: title.to_owned(),
        ..NewQuestion::default()
    }
}

fn pick(option: &str) -> Answer {
    Answer {
        option: Some(option.to_owned()),
        ..Answer::default()
    }
}

#[tokio::test]
async fn a_question_keeps_its_options_tags_tickets_and_dependencies() {
    let f = fixture().await;
    let first = f.ticket(&f.board, "Login").await;
    let second = f.ticket(&f.board, "Logout").await;
    let input = NewQuestion {
        title: String::from(" Which store? "),
        description: Some(String::from("Files or a database")),
        occasion: Some(String::from(" planning ARC-0001 ")),
        tags: vec![String::from("storage, data"), String::from("Storage")],
        tickets: vec![second.to_lowercase(), first.clone(), first.clone()],
        depends_on: vec![String::from("arc-2")],
        options: vec![option("File", false), option("Db", true)],
    };
    let key = f.ask(&input).await.unwrap();
    assert_eq!(key, "ARC-Q0001");

    let detail = f.questions.get("arc-q1").await.unwrap();
    let row = &detail.view.row;
    let record = row.question.record();
    assert_eq!(record.title.as_str(), "Which store?");
    assert_eq!(record.occasion.as_deref(), Some("planning ARC-0001"));
    assert_eq!(record.tags, ["storage", "data"]);
    let tickets: Vec<String> = record.tickets.iter().map(ToString::to_string).collect();
    assert_eq!(tickets, ["ARC-0001", "ARC-0002"]);
    assert_eq!(row.tickets[0].title, "Login");
    assert_eq!(record.origin_session.as_deref(), Some("s-1"));
    assert_eq!(
        (row.origin_name.as_str(), row.origin_kind.as_str()),
        ("claude", "session")
    );
    assert_eq!(record.created_by, f.person.id());
    assert_eq!(record.depends_on, [QuestionRef::parse("ARC-0002").unwrap()]);
    // the open ticket it depends on keeps the question from being ready
    assert!(!detail.view.ready);
    assert_eq!(detail.view.waiting_on.len(), 1);
    let options = detail.newest_options();
    let keys: Vec<String> = options.iter().map(QuestionOption::key).collect();
    assert_eq!(keys, ["O1", "O2"]);
    assert!(options[1].recommended && !options[0].recommended);
    assert_eq!(options[0].pros, ["simple"]);
    assert_eq!((row.research_rounds, row.option_count), (1, 2));
    let head = f.store.changes_since(0, 1000).await.unwrap();
    assert_eq!(
        head.last().map(|change| change.entity),
        Some(Entity::Question)
    );
}

#[tokio::test]
async fn asking_refuses_bad_input_without_using_up_a_number() {
    let f = fixture().await;
    f.ticket(&f.board, "Here").await;
    let elsewhere = f.ticket(&f.other, "There").await;
    let cases = [
        (titled(" "), "title-empty"),
        (
            NewQuestion {
                tickets: vec![String::from("ARC-9")],
                ..titled("x")
            },
            "question-ticket-unknown",
        ),
        (
            NewQuestion {
                tickets: vec![elsewhere],
                ..titled("x")
            },
            "question-ticket-other-project",
        ),
        (
            NewQuestion {
                depends_on: vec![String::from("ARC-Q7")],
                ..titled("x")
            },
            "dependency-unknown",
        ),
        (
            NewQuestion {
                depends_on: vec![String::from("what")],
                ..titled("x")
            },
            "reference-invalid",
        ),
        (
            NewQuestion {
                options: vec![option("A", true), option("B", true)],
                ..titled("x")
            },
            "research-recommended-multiple",
        ),
        (
            NewQuestion {
                options: vec![OptionDraft::default()],
                ..titled("x")
            },
            "research-invalid",
        ),
    ];
    for (input, code) in cases {
        assert_eq!(f.ask(&input).await, Err(code.to_owned()), "{code}");
    }
    let missing = f
        .questions
        .create(
            nullradix_domain::Id::from_uuid(uuid::Uuid::from_u128(1)),
            &titled("x"),
            PERSON,
            &f.person,
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "project-not-found");
    assert_eq!(f.ask(&titled("Finally")).await.unwrap(), "ARC-Q0001");
}

#[tokio::test]
async fn an_open_question_holds_its_tickets_back_until_a_person_answers() {
    let f = fixture().await;
    let ticket = f.ticket(&f.board, "Login").await;
    let input = NewQuestion {
        tickets: vec![ticket.clone()],
        options: vec![option("File", false), option("Db", true)],
        ..titled("Which store?")
    };
    let key = f.ask(&input).await.unwrap();
    let view = f.tickets.get(&ticket).await.unwrap().view;
    assert_eq!(view.open_questions[0].key.to_string(), key);
    assert_eq!(
        f.start(&ticket, false).await,
        Err(String::from("question-open"))
    );
    let claimed = f
        .tickets
        .claim(&ticket, "claude", None, &f.person, None, false)
        .await
        .unwrap_err();
    assert_eq!(claimed.code(), "question-open");
    let planning = f
        .tickets
        .next_planning(TicketScope::default(), None, &f.person)
        .await
        .unwrap();
    assert!(planning.is_none());

    assert_eq!(
        f.answer(&key, &pick("O2"), agent("claude")).await,
        Err(String::from("human-only"))
    );
    let session_only = Author {
        session: Some("s-1"),
        ..Author::default()
    };
    assert_eq!(
        f.answer(&key, &pick("O2"), session_only).await,
        Err(String::from("human-only"))
    );
    assert_eq!(
        f.answer(&key, &pick("O3"), PERSON).await,
        Err(String::from("decision-option-unknown"))
    );
    assert_eq!(
        f.answer(&key, &Answer::default(), PERSON).await,
        Err(String::from("decision-ambiguous"))
    );
    let stale = f
        .questions
        .answer(
            &key,
            &pick("O2"),
            PERSON,
            &f.person,
            Some(Version::from_stored(7)),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "version-conflict");
    let answer = Answer {
        option: Some(String::from("o2")),
        reason: Some(String::from("fits")),
        text: None,
    };
    let answered = f
        .questions
        .answer(
            &key,
            &answer,
            Author::default(),
            &f.person,
            Some(Version::INITIAL),
        )
        .await
        .unwrap();
    let record = answered.view.row.question.record();
    assert_eq!(record.state, QuestionState::Answered);
    let decision = record.decision.as_ref().unwrap();
    assert_eq!(decision.choice, Some(Choice::Option(2)));
    assert_eq!(answered.view.row.decided_by_name.as_deref(), Some("anna"));
    assert_eq!(
        f.answer(&key, &pick("O1"), PERSON).await,
        Err(String::from("question-state-invalid"))
    );

    let detail = f.tickets.get(&ticket).await.unwrap();
    assert_eq!(detail.view.open_questions, Vec::new());
    let line = detail.history.last().unwrap();
    assert_eq!(line.field, "question");
    assert_eq!(
        (line.from.as_deref(), line.to.as_deref()),
        (Some("open"), Some("answered"))
    );
    assert_eq!(
        line.note.as_deref(),
        Some("question answered: ARC-Q0001 \"Which store?\"; reason: fits")
    );
    assert_eq!(f.start(&ticket, false).await, Ok(()));
}

#[tokio::test]
async fn force_lifts_the_question_lock_and_reopening_restores_it() {
    let f = fixture().await;
    let ticket = f.ticket(&f.board, "Login").await;
    let key = f
        .ask(&NewQuestion {
            tickets: vec![ticket.clone()],
            ..titled("Why?")
        })
        .await
        .unwrap();
    let discard =
        |reason: Option<&'static str>| f.questions.discard(&key, reason, PERSON, &f.person, None);
    assert_eq!(
        discard(Some(" ")).await.unwrap_err().code(),
        "decision-reason-missing"
    );
    let discarded = discard(Some("moot")).await.unwrap();
    assert_eq!(
        discarded.view.row.question.state(),
        QuestionState::Discarded
    );
    assert_eq!(
        discard(Some("again")).await.unwrap_err().code(),
        "question-state-invalid"
    );
    let reopened = f
        .questions
        .reopen(&key, Some(" new facts "), PERSON, &f.person, None)
        .await
        .unwrap();
    assert_eq!(reopened.view.row.question.record().decision, None);
    let kept: Vec<&str> = reopened
        .comments
        .iter()
        .map(|line| line.comment.body())
        .collect();
    assert_eq!(
        kept,
        ["Reopened. Reason: new facts. Earlier decision: discarded. Reason: moot (anna)"]
    );
    let note = f.tickets.get(&ticket).await.unwrap().history.pop().unwrap();
    assert_eq!(
        (
            note.from.as_deref(),
            note.to.as_deref(),
            note.note.as_deref()
        ),
        (
            Some("discarded"),
            Some("open"),
            Some("question reopened: ARC-Q0001 \"Why?\"; reason: new facts")
        )
    );
    assert_eq!(
        f.questions
            .reopen(&key, None, PERSON, &f.person, None)
            .await
            .unwrap_err()
            .code(),
        "question-state-invalid"
    );
    assert_eq!(
        f.start(&ticket, false).await,
        Err(String::from("question-open"))
    );
    assert_eq!(f.start(&ticket, true).await, Ok(()));
}

#[tokio::test]
async fn comments_bump_the_question_and_list_oldest_first() {
    let f = fixture().await;
    let key = f.ask(&titled("Why?")).await.unwrap();
    let comment = |body: &'static str, author| f.questions.comment(&key, body, author, &f.person);
    assert_eq!(
        comment(" \n", PERSON).await.unwrap_err().code(),
        "comment-empty"
    );
    let first = comment(" New finding ", agent("claude")).await.unwrap();
    assert_eq!(first.comment.body(), "New finding");
    assert_eq!(first.author_name, "claude");
    comment("Thanks", PERSON).await.unwrap();
    let missing = f
        .questions
        .comment("ARC-Q9", "x", PERSON, &f.person)
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "question-not-found");

    let detail = f.questions.get(&key).await.unwrap();
    let bodies: Vec<&str> = detail
        .comments
        .iter()
        .map(|line| line.comment.body())
        .collect();
    assert_eq!(bodies, ["New finding", "Thanks"]);
    assert_eq!(detail.view.row.question.record().version.value(), 3);
    // ticket comments still live next to question comments
    let ticket = f.ticket(&f.board, "Login").await;
    f.comments
        .add(&ticket, "ticket note", PERSON, &f.person)
        .await
        .unwrap();
    assert_eq!(f.comments.list(&ticket).await.unwrap().len(), 1);
    assert_eq!(f.questions.get(&key).await.unwrap().comments.len(), 2);
}

#[tokio::test]
async fn lists_filter_by_state_and_ticket_and_put_ready_questions_first() {
    let f = fixture().await;
    let ticket = f.ticket(&f.board, "Login").await;
    let waiting = f
        .ask(&NewQuestion {
            depends_on: vec![ticket.clone()],
            ..titled("Waits")
        })
        .await
        .unwrap();
    let ready = f
        .ask(&NewQuestion {
            tickets: vec![ticket.clone()],
            ..titled("Ready")
        })
        .await
        .unwrap();
    let done = f.ask(&titled("Done")).await.unwrap();
    f.questions
        .discard(&done, Some("moot"), PERSON, &f.person, None)
        .await
        .unwrap();
    let f = &f;
    let keys = |filter: QuestionFilter| async move {
        let list = f.questions.list(f.board.project(), &filter).await.unwrap();
        let keys: Vec<String> = list
            .questions
            .iter()
            .map(|view| view.row.question.key().to_string())
            .collect();
        (keys, list.counts)
    };
    let (all, counts) = keys(QuestionFilter::default()).await;
    assert_eq!(all, [ready.clone(), waiting.clone(), done.clone()]);
    assert_eq!((counts.open, counts.answered, counts.discarded), (2, 0, 1));
    let open = QuestionFilter::parse(Some("open"), None, None, None).unwrap();
    assert_eq!(keys(open).await.0, [ready.clone(), waiting]);
    let about = QuestionFilter::parse(Some("all"), Some(&ticket), None, None).unwrap();
    assert_eq!(keys(about).await.0, [ready]);
    let text = QuestionFilter::parse(None, None, None, Some("DON")).unwrap();
    assert_eq!(keys(text).await.0, [done]);
}

#[tokio::test]
async fn deleting_a_ticket_drops_it_from_its_questions() {
    let f = fixture().await;
    let ticket = f.ticket(&f.board, "Gone").await;
    let key = f
        .ask(&NewQuestion {
            tickets: vec![ticket.clone()],
            depends_on: vec![ticket.clone()],
            ..titled("About it")
        })
        .await
        .unwrap();
    f.tickets.delete(&ticket, &f.person, None).await.unwrap();
    let detail = f.questions.get(&key).await.unwrap();
    let record = detail.view.row.question.record();
    assert_eq!(record.tickets.len(), 0);
    assert_eq!(record.depends_on.len(), 0);
    assert_eq!(record.version.value(), 2);
    assert!(detail.view.ready);
}

#[tokio::test]
async fn a_session_caller_without_a_name_asks_as_a_session_and_never_decides() {
    let f = fixture().await;
    let session_only = Author {
        session: Some("s-1"),
        ..Author::default()
    };
    let asked = f
        .questions
        .create(f.board.project(), &titled("Why?"), session_only, &f.person)
        .await
        .unwrap();
    let row = &asked.view.row;
    assert_eq!(
        (
            row.origin_name.as_str(),
            row.origin_kind.as_str(),
            row.question.record().origin_session.as_deref()
        ),
        ("anna", "session", Some("s-1"))
    );
    let key = row.question.key().to_string();
    let in_session = f
        .questions
        .comment(&key, "why", session_only, &f.person)
        .await
        .unwrap();
    assert_eq!(
        (
            in_session.author_name.as_str(),
            in_session.author_kind.as_str()
        ),
        ("anna", "session")
    );
    assert_eq!(
        f.answer(&key, &pick("O1"), session_only).await,
        Err(String::from("human-only"))
    );
    let text = Answer {
        text: Some(String::from("Because")),
        ..Answer::default()
    };
    assert_eq!(f.answer(&key, &text, Author::default()).await, Ok(()));
}
