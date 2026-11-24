//! Use cases and the ports they need.

mod clock;
mod error;
mod events;

pub use clock::{Clock, SystemClock};
pub use error::{AppError, ErrorKind};
pub use events::EventBus;
