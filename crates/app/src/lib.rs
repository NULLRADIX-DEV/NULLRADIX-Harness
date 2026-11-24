//! Use cases and the ports they need.

pub mod auth;
mod changes;
mod clock;
mod error;
mod ids;
pub mod projects;

pub use changes::{Change, ChangeFeed, ChangeOp, ChangeSignal, Entity};
pub use clock::{Clock, SystemClock};
pub use error::{AppError, ErrorKind};
pub use ids::{IdGenerator, UuidV7Generator};
