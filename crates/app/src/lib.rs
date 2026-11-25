//! Use cases and the ports they need.

pub mod accounts;
pub mod auth;
mod authors;
pub mod boards;
mod changes;
mod clock;
pub mod comments;
mod error;
pub mod favorites;
pub mod hooks;
mod ids;
pub mod inbox;
pub mod notes;
pub mod projects;
pub mod questions;
pub mod repositories;
pub mod runs;
pub mod secrets;
pub mod session_detail;
pub mod sessions;
pub mod tickets;
pub mod worktrees;

pub use changes::{Change, ChangeFeed, ChangeOp, ChangeSignal, Entity};
pub use clock::{Clock, SystemClock};
pub use error::{AppError, ErrorKind, SimilarNote};
pub use ids::{IdGenerator, UuidV7Generator};
