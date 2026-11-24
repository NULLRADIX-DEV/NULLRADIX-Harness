use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, anyhow};
use nullradix_api::BoardDto;
use nullradix_client::Client;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::board::local_runner;

const NOT_SELECTED: &str =
    "no board selected: pick one with `nullradix board use <board>` or pass --board <board>";

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

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientState {
    active_board: Option<Uuid>,
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
        let text = match fs::read_to_string(&self.file) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", self.file.display()));
            }
        };
        let state: ClientState = toml::from_str(&text)
            .with_context(|| format!("cannot parse {}", self.file.display()))?;
        Ok(state.active_board)
    }

    pub(crate) fn set_active(&self, board: Option<Uuid>) -> anyhow::Result<()> {
        if let Some(dir) = self.file.parent() {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let text = toml::to_string(&ClientState {
            active_board: board,
        })?;
        let temp = self.file.with_extension("toml.tmp");
        fs::write(&temp, text).with_context(|| format!("cannot write {}", temp.display()))?;
        fs::rename(&temp, &self.file)
            .with_context(|| format!("cannot write {}", self.file.display()))
    }

    /// `--board`, else the board of the current folder's repository, else the active board.
    pub(crate) fn one(&self, client: &Client, option: Option<Uuid>) -> anyhow::Result<Uuid> {
        if let Some(board) = option {
            return Ok(board);
        }
        let active = self.active()?;
        if let Some(board) = repository_board(client, active)? {
            return Ok(board);
        }
        active.ok_or_else(|| anyhow!(NOT_SELECTED))
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
}

fn repository_board(client: &Client, active: Option<Uuid>) -> anyhow::Result<Option<Uuid>> {
    let folder = std::env::current_dir().context("cannot read the current folder")?;
    let Some(path) = folder.to_str() else {
        return Ok(None);
    };
    let runner = local_runner(client)?;
    let boards = client.list_boards_at(runner.id, path)?.items;
    Ok(pick(&boards, active))
}

/// The active board if it is among `boards`, else the oldest.
fn pick(boards: &[BoardDto], active: Option<Uuid>) -> Option<Uuid> {
    if let Some(active) = active
        && boards.iter().any(|board| board.id == active)
    {
        return Some(active);
    }
    boards
        .iter()
        .min_by_key(|board| (board.created_at, board.id))
        .map(|board| board.id)
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

    #[test]
    fn picks_the_active_board_of_the_repository_else_the_oldest() {
        let boards = [board(3, 20), board(1, 30), board(2, 10)];
        assert_eq!(
            pick(&boards, Some(Uuid::from_u128(1))),
            Some(Uuid::from_u128(1))
        );
        assert_eq!(
            pick(&boards, Some(Uuid::from_u128(9))),
            Some(Uuid::from_u128(2))
        );
        assert_eq!(pick(&boards, None), Some(Uuid::from_u128(2)));
        assert_eq!(pick(&[], Some(Uuid::from_u128(1))), None);
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
