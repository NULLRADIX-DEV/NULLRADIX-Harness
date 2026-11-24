//! Executes runs on this machine under detached supervisors.

mod local;
mod protocol;
mod supervisor;

pub use local::{LocalRunner, SupervisorCommand};
pub use supervisor::supervise;
