//! papercut — system-level friction telemetry for coding agents.
//!
//! Capture is automatic and dumb; interpretation is deliberate and rare. The
//! store is private by default; publishing into a repo is an explicit act.

pub mod adapters;
pub mod app;
pub mod cli;
pub mod commands;
pub mod detect;
pub mod git_meta;
pub mod harness;
pub mod id;
pub mod managed_block;
pub mod model;
pub mod output;
pub mod paths;
pub mod projection;
pub mod query;
pub mod signal;
pub mod store;
pub mod time;
pub mod util;

pub use app::run;
