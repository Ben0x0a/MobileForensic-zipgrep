//! mf-zipgrep library crate.
//!
//! Defines: the public module surface (`models`, `zip`, `search`) so both the
//! `mf-zipgrep` binary and the integration tests can use the parser and search
//! engine without going through the CLI.
//! Used by: `main.rs` (the binary) and everything under `tests/`.
//! Uses: the modules it re-exports.
//!
//! Why a separate lib crate: keeping the logic in a library (rather than only
//! in `main.rs`) lets `tests/` link against it directly, which is the standard
//! Rust layout for a tool that wants both a CLI and a tested core.

pub mod engine;
pub mod export;
pub mod filter;
pub mod inspect;
pub mod models;
pub mod output;
pub mod search;
pub mod zip;
