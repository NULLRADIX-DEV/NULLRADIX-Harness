use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, anyhow, bail};
use nullradix_api::BoardDto;
use nullradix_client::Client;
use uuid::Uuid;

use crate::board::local_runner;

const NOT_SELECTED: &str =
    "no board selected: pick one with `nullradix board use <board>` or pass --board <board>";
const ACTIVE_BOARD: &str = "active_board";

/// `--board` of commands that work on one board or on all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoardOption {
    One(Uuid),
    All,
}

impl FromStr for BoardOption {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text == "all" {
            return Ok(Self::All);
        }
        Uuid::parse_str(text)
            .map(Self::One)
            .map_err(|_| format!("{text} is neither a board id nor `all`"))
    }
}

/// What the defaults found: a board of the repository, or only the id of the active board.
enum Picked {
    Repository(BoardDto),
    Active(Uuid),
}

/// The board a command works on; the active board is this CLI's own state, as in legacy.
#[derive(Debug, Clone)]
pub(crate) struct BoardChoice {
    file: PathBuf,
}

impl BoardChoice {
    pub(crate) fn in_dir(dir: &Path) -> Self {
        Self {
            file: dir.join("client.toml"),
        }
    }

    pub(crate) fn active(&self) -> anyhow::Result<Option<Uuid>> {
        let Some(value) = self.read()?.remove(ACTIVE_BOARD) else {
            return Ok(None);
        };
        let board = value
            .as_str()
            .and_then(|text| Uuid::parse_str(text).ok())
            .with_context(|| format!("{ACTIVE_BOARD} in {} is no id", self.file.display()))?;
        Ok(Some(board))
    }

    /// Keeps other keys, so CLIs of other versions can share the file.
    pub(crate) fn set_active(&self, board: Option<Uuid>) -> anyhow::Result<()> {
        // a broken file is replaced rather than blocking the fix
        let mut state = self.read().unwrap_or_default();
        match board {
            Some(board) => state.insert(ACTIVE_BOARD.to_owned(), board.to_string().into()),
            None => state.remove(ACTIVE_BOARD),
        };
        if let Some(dir) = self.file.parent() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        // a temp file per call, so parallel calls never write into each other's file
        let temp = self
            .file
            .with_extension(format!("toml.{}.tmp", Uuid::now_v7().simple()));
        fs::write(&temp, toml::to_string(&state)?)
            .with_context(|| format!("cannot write {}", temp.display()))?;
        fs::rename(&temp, &self.file).or_else(|error| {
            let _ = fs::remove_file(&temp);
            Err(error).with_context(|| format!("cannot write {}", self.file.display()))
        })
    }

    fn read(&self) -> anyhow::Result<toml::Table> {
        match fs::read_to_string(&self.file) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("cannot parse {}", self.file.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(toml::Table::new()),
            Err(error) => {
                Err(error).with_context(|| format!("cannot read {}", self.file.display()))
            }
        }
    }

    /// `--board`, else the board of the current folder's repository, else the active board.
    pub(crate) fn one(&self, client: &Client, option: Option<Uuid>) -> anyhow::Result<Uuid> {
        if let Some(board) = option {
            return Ok(board);
        }
        Ok(match self.default(client)? {
            Picked::Repository(board) => board.id,
            Picked::Active(board) => board,
        })
    }

    /// The board [`Self::one`] picks, with its details.
    pub(crate) fn board(&self, client: &Client, option: Option<Uuid>) -> anyhow::Result<BoardDto> {
        let id = match option {
            Some(board) => board,
            None => match self.default(client)? {
                Picked::Repository(board) => return Ok(board),
                Picked::Active(board) => board,
            },
        };
        match client
            .list_all_boards()?
            .items
            .into_iter()
            .find(|board| board.id == id)
        {
            Some(board) => Ok(board),
            None => bail!("no board has the id {id}"),
        }
    }

    /// One board as [`Self::one`] picks it, or `None` for all boards with `--board all`.
    pub(crate) fn many(
        &self,
        client: &Client,
        option: Option<BoardOption>,
    ) -> anyhow::Result<Option<Uuid>> {
        match option {
            Some(BoardOption::All) => Ok(None),
            Some(BoardOption::One(board)) => Ok(Some(board)),
            None => self.one(client, None).map(Some),
        }
    }

    fn default(&self, client: &Client) -> anyhow::Result<Picked> {
        let active = self.active()?;
        if let Some(board) = repository_board(client, active)? {
            return Ok(Picked::Repository(board));
        }
        active
            .map(Picked::Active)
            .ok_or_else(|| anyhow!(NOT_SELECTED))
    }
}

fn repository_board(client: &Client, active: Option<Uuid>) -> anyhow::Result<Option<BoardDto>> {
    let folder = std::env::current_dir().context("cannot read the current folder")?;
    let Some(path) = folder.to_str() else {
        return Ok(None);
    };
    let runner = local_runner(client)?;
    let boards = client.list_boards_at(runner.id, path)?.items;
    Ok(pick(boards, active))
}

/// The active board if it is among `boards`, else the oldest.
fn pick(boards: Vec<BoardDto>, active: Option<Uuid>) -> Option<BoardDto> {
    let mut oldest: Option<BoardDto> = None;
    for board in boards {
        if Some(board.id) == active {
            return Some(board);
        }
        if oldest
            .as_ref()
            .is_none_or(|known| (board.created_at, board.id) < (known.created_at, known.id))
        {
            oldest = Some(board);
        }
    }
    oldest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(id: u128, created_at: i64) -> BoardDto {
        let json = serde_json::json!({
            "id": Uuid::from_u128(id),
            "projectId": Uuid::from_u128(100),
            "projectKey": "ARC",
            "name": "Main",
            "ownerId": Uuid::from_u128(200),
            "ownerName": "Ben",
            "dispatchEnabled": true,
            "repositoryId": null,
            "repositoryName": null,
            "openTickets": 0,
            "createdAt": created_at,
            "updatedAt": created_at,
            "version": 1,
        });
        serde_json::from_value(json).unwrap()
    }

    fn picked(active: Option<u128>) -> Option<Uuid> {
        let boards = vec![board(3, 20), board(1, 30), board(2, 10)];
        pick(boards, active.map(Uuid::from_u128)).map(|board| board.id)
    }

    #[test]
    fn picks_the_active_board_of_the_repository_else_the_oldest() {
        assert_eq!(picked(Some(1)), Some(Uuid::from_u128(1)));
        assert_eq!(picked(Some(9)), Some(Uuid::from_u128(2)));
        assert_eq!(picked(None), Some(Uuid::from_u128(2)));
        assert!(pick(Vec::new(), Some(Uuid::from_u128(1))).is_none());
    }

    #[test]
    fn the_active_board_survives_a_round_trip_and_can_be_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let choice = BoardChoice::in_dir(&dir.path().join("config"));
        assert_eq!(choice.active().unwrap(), None);
        choice.set_active(Some(Uuid::from_u128(7))).unwrap();
        assert_eq!(choice.active().unwrap(), Some(Uuid::from_u128(7)));
        choice.set_active(None).unwrap();
        assert_eq!(choice.active().unwrap(), None);
        let leftovers = fs::read_dir(dir.path().join("config")).unwrap().count();
        assert_eq!(leftovers, 1);
    }

    #[test]
    fn keys_of_other_versions_are_kept_and_a_broken_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let choice = BoardChoice::in_dir(dir.path());
        fs::write(dir.path().join("client.toml"), "theme = \"dark\"\n").unwrap();
        assert_eq!(choice.active().unwrap(), None);
        choice.set_active(Some(Uuid::from_u128(7))).unwrap();
        let text = fs::read_to_string(dir.path().join("client.toml")).unwrap();
        assert!(text.contains("theme = \"dark\""), "{text}");
        fs::write(dir.path().join("client.toml"), "active_board = \"0190").unwrap();
        assert!(choice.active().is_err());
        choice.set_active(Some(Uuid::from_u128(8))).unwrap();
        assert_eq!(choice.active().unwrap(), Some(Uuid::from_u128(8)));
    }

    #[test]
    fn board_options_are_an_id_or_all() {
        assert_eq!("all".parse::<BoardOption>(), Ok(BoardOption::All));
        assert_eq!(
            Uuid::from_u128(5).to_string().parse::<BoardOption>(),
            Ok(BoardOption::One(Uuid::from_u128(5)))
        );
        assert!("main".parse::<BoardOption>().is_err());
    }
}
