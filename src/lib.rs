//! Core library for the Rust rewrite of `rdp-tui`.

pub mod cli;
pub mod config;
pub mod freerdp;
pub mod model;
pub mod planner;
pub mod profile_store;

pub use profile_store::ProfileStore;
