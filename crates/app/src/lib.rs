//! Use cases and the ports they need.

pub mod auth;
pub mod boards;
mod changes;
mod clock;
mod error;
pub mod hooks;
mod ids;
pub mod projects;
pub mod repositories;
pub mod runs;
pub mod tickets;
pub mod worktrees;

pub use changes::{Change, ChangeFeed, ChangeOp, ChangeSignal, Entity};
pub use clock::{Clock, SystemClock};
pub use error::{AppError, ErrorKind};
pub use ids::{IdGenerator, UuidV7Generator};
