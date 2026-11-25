use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::{
    Id, Repository, Runner, TargetBranch, TextError, Ticket, TicketKey, Timestamp, Version,
    required_text,
};

pub const WORKTREE_BRANCH_MAX: usize = 400;
pub const WORKTREE_PATH_MAX: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorktreeError {
    #[error("the branch {0}")]
    Branch(TextError),
    #[error("the worktree path {0}")]
    Path(TextError),
}

/// The branch a worktree has checked out, such as `ticket/ARC-0007-fix-it`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorktreeBranch(String);

impl WorktreeBranch {
    pub fn parse(text: &str) -> Result<Self, WorktreeError> {
        required_text(text, WORKTREE_BRANCH_MAX)
            .map(|branch| Self(branch.to_owned()))
            .map_err(WorktreeError::Branch)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The folder of a worktree on its runner, with forward slashes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorktreePath(String);

impl WorktreePath {
    pub fn parse(text: &str) -> Result<Self, WorktreeError> {
        required_text(text, WORKTREE_PATH_MAX)
            .map(|path| Self(path.to_owned()))
            .map_err(WorktreeError::Path)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a worktree exists; research and plan worktrees are detached and temporary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum WorktreePurpose {
    Work,
    Research,
    Plan,
}

impl WorktreePurpose {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Whether the folder should exist; the runner brings the actual state there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum DesiredState {
    Present,
    Absent,
}

impl DesiredState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// What the runner last reported about the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum ActualState {
    Unknown,
    Creating,
    Present,
    Removing,
    Removed,
    Failed,
}

impl ActualState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// The setup commands of a new worktree; `none` when nothing is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumString, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum SetupState {
    #[strum(serialize = "none")]
    Unset,
    Pending,
    Done,
    Failed,
}

impl SetupState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// What a new worktree brings; the states start as nothing happened yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDraft {
    pub repository: Id<Repository>,
    pub runner: Id<Runner>,
    pub branch: WorktreeBranch,
    pub target_branch: Option<TargetBranch>,
    pub path: WorktreePath,
    pub purpose: WorktreePurpose,
}

/// Every stored field of a worktree, for persistence adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub id: Id<Worktree>,
    pub repository: Id<Repository>,
    pub runner: Id<Runner>,
    pub branch: WorktreeBranch,
    pub target_branch: Option<TargetBranch>,
    pub path: WorktreePath,
    pub purpose: WorktreePurpose,
    pub desired: DesiredState,
    pub actual: ActualState,
    pub setup: SetupState,
    pub ready_to_merge: bool,
    pub last_error: Option<String>,
    /// When the runner last reported the actual state.
    pub observed_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

/// A registry entry of legacy `worktrees.json`: it stays, even after its folder is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree(WorktreeRecord);

impl Worktree {
    #[must_use]
    pub fn new(id: Id<Self>, draft: WorktreeDraft, now: Timestamp) -> Self {
        Self(WorktreeRecord {
            id,
            repository: draft.repository,
            runner: draft.runner,
            branch: draft.branch,
            target_branch: draft.target_branch,
            path: draft.path,
            purpose: draft.purpose,
            desired: DesiredState::Present,
            actual: ActualState::Unknown,
            setup: SetupState::Unset,
            ready_to_merge: false,
            last_error: None,
            observed_at: None,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    #[must_use]
    pub const fn restore(record: WorktreeRecord) -> Self {
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &WorktreeRecord {
        &self.0
    }

    #[must_use]
    pub const fn id(&self) -> Id<Self> {
        self.0.id
    }

    #[must_use]
    pub const fn branch(&self) -> &WorktreeBranch {
        &self.0.branch
    }

    #[must_use]
    pub const fn path(&self) -> &WorktreePath {
        &self.0.path
    }

    /// The worktree as the runner reported it after creating the folder.
    #[must_use]
    pub fn seen_present(mut self, at: Timestamp) -> Self {
        self.0.actual = ActualState::Present;
        self.0.observed_at = Some(at);
        self
    }

    /// Legacy `removed`: its folder is gone or on its way out.
    #[must_use]
    pub fn removed(&self) -> bool {
        self.0.desired == DesiredState::Absent || self.0.actual == ActualState::Removed
    }
}

const SLUG_MAX: usize = 40;

/// Short title for branch names: lowercase, umlauts spelled out, other runs one dash.
#[must_use]
pub fn slug(title: &str) -> String {
    let mut dashed = String::with_capacity(title.len());
    for letter in title.to_lowercase().chars() {
        match letter {
            '\u{e4}' => dashed.push_str("ae"),
            '\u{f6}' => dashed.push_str("oe"),
            '\u{fc}' => dashed.push_str("ue"),
            '\u{df}' => dashed.push_str("ss"),
            'a'..='z' | '0'..='9' => dashed.push(letter),
            _ if !dashed.ends_with('-') => dashed.push('-'),
            _ => {}
        }
    }
    let slug = shorten(dashed.trim_matches('-'));
    if slug.is_empty() {
        String::from("ticket")
    } else {
        slug
    }
}

/// Cuts at the last dash within the limit, so no word is split.
fn shorten(slug: &str) -> String {
    if slug.len() <= SLUG_MAX {
        return slug.to_owned();
    }
    let cut = &slug[..SLUG_MAX];
    let short = match cut.rfind('-') {
        Some(dash) if dash > 0 => &cut[..dash],
        _ => cut,
    };
    short.trim_end_matches('-').to_owned()
}

/// The branch of a ticket's own worktree, such as `ticket/ARC-0007-fix-login`.
#[must_use]
pub fn ticket_branch(key: &TicketKey, title: &str) -> WorktreeBranch {
    WorktreeBranch(format!("ticket/{key}-{}", slug(title)))
}

/// The ticket's worktree folder `<parent>/<name>-worktrees/<KEY>` next to the main checkout.
pub fn ticket_worktree_path(root: &str, key: &TicketKey) -> Result<WorktreePath, WorktreeError> {
    let root = root.replace('\\', "/");
    let root = root.trim_end_matches('/');
    let path = match root.rsplit_once('/') {
        Some((parent, name)) => format!("{parent}/{name}-worktrees/{key}"),
        None => format!("{root}-worktrees/{key}"),
    };
    WorktreePath::parse(&path)
}

/// A registry entry as auto-join sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinCandidate<'a> {
    pub worktree: &'a Worktree,
    pub folder_exists: bool,
    /// Targets of the tickets in its list that still exist.
    pub targets: Vec<Option<TargetBranch>>,
    /// An unfinished ticket of the list touches the same files.
    pub overlap: bool,
}

/// The first entry a ticket with `target` may join; without spec every ticket counts as the same.
#[must_use]
pub fn pick_join(candidates: &[JoinCandidate<'_>], target: Option<&TargetBranch>) -> Option<usize> {
    candidates.iter().position(|candidate| {
        !candidate.worktree.removed()
            && !candidate.worktree.record().ready_to_merge
            && candidate.folder_exists
            && candidate
                .targets
                .iter()
                .all(|other| other.as_ref() == target)
            && !candidate.overlap
    })
}

/// A ticket in a worktree's list; the list never shrinks, a link only ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeTicket {
    pub ticket: Id<Ticket>,
    pub key: TicketKey,
    /// Join order; 0 owns the worktree.
    pub position: u32,
    pub joined_at: Timestamp,
    /// When the ticket's branch and worktree were cleared.
    pub released_at: Option<Timestamp>,
}

impl WorktreeTicket {
    /// Whether the ticket still shows this worktree as its branch and folder.
    #[must_use]
    pub const fn live(&self) -> bool {
        self.released_at.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id<T>(value: u128) -> Id<T> {
        Id::from_uuid(uuid::Uuid::from_u128(value))
    }

    #[test]
    fn branches_and_paths_are_trimmed_and_limited() {
        assert_eq!(
            WorktreeBranch::parse(" ticket/ARC-0001-a ")
                .unwrap()
                .as_str(),
            "ticket/ARC-0001-a"
        );
        assert_eq!(
            WorktreeBranch::parse(&"b".repeat(401)),
            Err(WorktreeError::Branch(TextError::TooLong(400)))
        );
        assert_eq!(
            WorktreePath::parse(" "),
            Err(WorktreeError::Path(TextError::Empty))
        );
        assert_eq!(
            WorktreePath::parse(&"p".repeat(501)),
            Err(WorktreeError::Path(TextError::TooLong(500)))
        );
    }

    #[test]
    fn states_keep_their_stored_names() {
        assert_eq!(SetupState::Unset.as_str(), "none");
        assert_eq!("none".parse::<SetupState>(), Ok(SetupState::Unset));
        assert_eq!(ActualState::Removing.as_str(), "removing");
        assert_eq!("absent".parse::<DesiredState>(), Ok(DesiredState::Absent));
        assert_eq!(WorktreePurpose::Research.as_str(), "research");
    }

    #[test]
    fn new_worktrees_are_wanted_and_not_yet_seen() {
        let draft = WorktreeDraft {
            repository: id(1),
            runner: id(2),
            branch: WorktreeBranch::parse("ticket/ARC-0001-a").unwrap(),
            target_branch: None,
            path: WorktreePath::parse("/work/arches-worktrees/ARC-0001").unwrap(),
            purpose: WorktreePurpose::Work,
        };
        let worktree = Worktree::new(id(3), draft, Timestamp::from_millis(4));
        let record = worktree.record();
        assert_eq!(
            (record.desired, record.actual, record.setup),
            (
                DesiredState::Present,
                ActualState::Unknown,
                SetupState::Unset
            )
        );
        assert!(!record.ready_to_merge);
        assert_eq!(record.version, Version::INITIAL);
        assert_eq!(Worktree::restore(record.clone()), worktree);
        assert!(!worktree.removed());
        let seen = worktree.seen_present(Timestamp::from_millis(9));
        assert_eq!(seen.record().actual, ActualState::Present);
        assert_eq!(seen.record().observed_at, Some(Timestamp::from_millis(9)));
    }

    #[test]
    fn slugs_follow_the_legacy_rules() {
        assert_eq!(slug("Login Formular bauen"), "login-formular-bauen");
        assert_eq!(
            slug("Gr\u{f6}\u{df}e \u{e4}ndern f\u{fc}r \u{dc}bersicht"),
            "groesse-aendern-fuer-uebersicht"
        );
        assert_eq!(slug("  --Fix: the  (API)!! "), "fix-the-api");
        assert_eq!(slug("Caf\u{e9}"), "caf");
        assert_eq!(slug("!!!"), "ticket");
        assert_eq!(slug(""), "ticket");
        let long = slug("one two three four five six seven eight nine ten");
        assert_eq!(long, "one-two-three-four-five-six-seven-eight");
        assert!(long.len() <= 40);
        assert_eq!(slug(&"a".repeat(45)), "a".repeat(40));
    }

    #[test]
    fn ticket_branches_and_folders_derive_from_key_and_root() {
        let key = TicketKey::parse("ARC-7").unwrap();
        assert_eq!(
            ticket_branch(&key, "Fix the login").as_str(),
            "ticket/ARC-0007-fix-the-login"
        );
        let path = |root: &str| ticket_worktree_path(root, &key).unwrap();
        assert_eq!(
            path("C:/work/Arches").as_str(),
            "C:/work/Arches-worktrees/ARC-0007"
        );
        assert_eq!(
            path(r"C:\work\Arches\").as_str(),
            "C:/work/Arches-worktrees/ARC-0007"
        );
        assert_eq!(path("/arches").as_str(), "/arches-worktrees/ARC-0007");
        assert!(ticket_worktree_path(&format!("/{}", "x".repeat(500)), &key).is_err());
    }

    fn worktree(change: impl FnOnce(&mut WorktreeRecord)) -> Worktree {
        let draft = WorktreeDraft {
            repository: id(1),
            runner: id(2),
            branch: WorktreeBranch::parse("ticket/ARC-0001-a").unwrap(),
            target_branch: None,
            path: WorktreePath::parse("/work/arches-worktrees/ARC-0001").unwrap(),
            purpose: WorktreePurpose::Work,
        };
        let mut record = Worktree::new(id(3), draft, Timestamp::from_millis(4))
            .record()
            .clone();
        change(&mut record);
        Worktree::restore(record)
    }

    fn candidate<'a>(worktree: &'a Worktree, target: Option<&str>) -> JoinCandidate<'a> {
        JoinCandidate {
            worktree,
            folder_exists: true,
            targets: vec![target.and_then(|name| TargetBranch::parse(name).unwrap())],
            overlap: false,
        }
    }

    #[test]
    fn auto_join_takes_the_first_open_entry_with_the_same_target() {
        let release = TargetBranch::parse("release").unwrap();
        let open = worktree(|_| {});
        let absent = worktree(|record| record.desired = DesiredState::Absent);
        let removed = worktree(|record| record.actual = ActualState::Removed);
        let ready = worktree(|record| record.ready_to_merge = true);
        assert!(absent.removed() && removed.removed());
        let blocked = [
            candidate(&absent, None),
            candidate(&removed, None),
            candidate(&ready, None),
            JoinCandidate {
                folder_exists: false,
                ..candidate(&open, None)
            },
            JoinCandidate {
                overlap: true,
                ..candidate(&open, None)
            },
            candidate(&open, Some("release")),
        ];
        assert_eq!(pick_join(&blocked, None), None);
        assert_eq!(pick_join(&blocked, release.as_ref()), Some(5));
        let first = [
            candidate(&open, Some("release")),
            candidate(&open, None),
            candidate(&open, None),
        ];
        assert_eq!(pick_join(&first, None), Some(1));
        let mixed = JoinCandidate {
            targets: vec![None, release.clone()],
            ..candidate(&open, None)
        };
        assert_eq!(pick_join(&[mixed], None), None);
        let empty = JoinCandidate {
            targets: Vec::new(),
            ..candidate(&open, None)
        };
        assert_eq!(pick_join(&[empty], release.as_ref()), Some(0));
    }
}
