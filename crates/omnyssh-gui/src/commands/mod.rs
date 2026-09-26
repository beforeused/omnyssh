//! IPC command handlers, one module per domain (tech-gui.md §3.1). Commands are
//! thin: validate input, call the core, return a DTO or an error.

pub mod docker;
pub mod editor;
pub mod files;
pub mod hosts;
pub mod keysetup;
pub mod sftp;
pub mod snippets;
pub mod terminal;
pub mod transfer;
pub mod update;
pub mod vpn;
