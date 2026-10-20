//! Wiki: docs/wiki/robot.md

#[cfg(unix)]
mod cli;
mod client;
mod error;
mod observation;
#[cfg(unix)]
mod store;

#[cfg(unix)]
pub use cli::{observe, pair};
pub use client::{Acknowledged, Device, Pairing, Session};
use client::{random_id, timestamp};
pub use error::{Error, Refusal};
pub use observation::Source;
