//! caffeine-daemon: an HTTP service that holds `caffeinate` open while callers
//! ask for it.
//!
//! - `holds`  — the holds themselves and the rules around them
//! - `caffeinate` — the process that keeps the Mac awake
//! - `server` — the HTTP API
//! - `client` — the `status` subcommand's side of that API
//! - `config` — the config file
//! - `hook`   — the Claude Code hook script this ships with
//! - `launchagent` — the launchd plist `install-launchagent` writes
//! - `lock`   — keeps a second daemon from starting

pub mod caffeinate;
pub mod client;
pub mod config;
pub mod holds;
pub mod hook;
pub mod launchagent;
pub mod lock;
pub mod server;
