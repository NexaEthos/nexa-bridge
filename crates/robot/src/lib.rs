//! Wiki: docs/wiki/robot.md
#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]

pub mod allowlist;
pub mod board;
pub mod config;
pub mod foxglove;
pub mod ros;

pub mod board_inventory;
pub mod hardware;
