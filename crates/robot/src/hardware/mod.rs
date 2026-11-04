//! Wiki: docs/wiki/robot.md

#[cfg(unix)]
mod cli;
mod client;
mod commands;
mod error;
mod observation;
#[cfg(unix)]
mod store;

#[cfg(unix)]
pub use cli::{Delivery, acknowledge, command, commands, deliver, device, observe, pair, report};
pub use client::{Acknowledged, Device, Pairing, Session};
use client::{random_id, timestamp};
pub use commands::WAIT_MAX_SECONDS;
pub use error::{Error, Refusal};
pub use observation::{DeclaredClass, Source};
