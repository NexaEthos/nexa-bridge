//! The managed link to Agora, shared by the desktop and the server setup. Wiki: docs/wiki/managed-auth.md
#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]

pub mod agora;
pub mod guides;
pub mod runtimes;
pub mod secret;
