//! Finding and calling the daemon from the CLI, hooks and the desktop shell.

mod discovery;
mod http;
mod spool;

pub use discovery::{Discovery, DiscoveryError, DiscoveryFile};
pub use http::{Client, ClientError};
pub use spool::{Spool, SpooledHook};
