//! Writing a rush plugin in Rust.
//!
//! A rush plugin is a program rush runs sandboxed, talking to it on fd 3:
//! JSON-RPC 2.0, each message a 4-byte big-endian length and then that many
//! bytes of JSON, calls going both ways. This crate is that connection
//! ([`Conn`]), the shapes the protocol sends ([`proto`]), the check rush's
//! broker makes of a `sidebar.set` ([`sidebar`]), `plugin.json`
//! ([`manifest`]) and RFC 3339 times ([`time`]).
//!
//! The smallest plugin:
//!
//! ```no_run
//! use rush_plugin_sdk::{Error, proto, tool};
//! use serde_json::{Value, json};
//!
//! rush_plugin_sdk::serve(|_conn, method, params| match method {
//!     "initialize" => Ok(json!({})),
//!     "tools.list" => Ok(json!({"tools": [tool::tool("hello", "Say hello.", tool::schema(&[], &[]))]})),
//!     "tools.call" => {
//!         let call: proto::ToolCall = rush_plugin_sdk::params(params)?;
//!         Ok(tool::text(format!("hello from {}", call.session), false))
//!     }
//!     _ => Err(Error::no_method(method)),
//! });
//! ```
//!
//! It shares no code with rush itself: rush's own crates check what a
//! plugin sends, and this crate sends it. The references in rush's
//! `plugins/skills/write-rush-plugin/references` say what every method does.

pub mod manifest;
pub mod proto;
pub mod rpc;
pub mod sidebar;
pub mod time;
pub mod tool;

pub use manifest::Manifest;
pub use rpc::{CallError, Conn, Error, Handler, connect, params, serve};
pub use sidebar::Sidebar;
