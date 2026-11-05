//! Pyro Porch without a window: everything that needs no screen, so it can be tested on its own and
//! shared between the app and the MCP server.

pub mod config;
pub mod engine;
pub mod mcp;
pub mod migrate;
pub mod paths;
pub mod policy;
pub mod sample_plugin;
pub mod security;
pub mod transfer;
pub mod vault;
pub mod wasm;
