//! rush serve: an HTTP server in front of this machine's session hosts,
//! for the web app and for rush on other machines (a serve with peers is a
//! hub, proxying to theirs); paired devices, push notifications, what puts
//! serve in reach of other devices (Tailscale, Cloudflare), another
//! machine's sessions kept in this machine's rush (attach), and the
//! bundled plugins that are serve's switches. See docs/remote.md.
//!
//! Ported from Go's internal/remote (config.go, plugins.go, devices.go,
//! pair.go, service.go, server.go, stream.go, watch.go, push.go, expose.go,
//! git.go, others.go, create.go, attach.go and sshd.go). Its HTTP is std's
//! sockets and a thread per connection (http.rs), its SSH the same with
//! ring's cryptography (sshwire.rs, sshd.rs): nothing runs while no
//! request comes.

pub mod attach;
pub mod client;
pub mod config;
pub mod ctx;
pub mod devices;
pub mod expose;
pub mod git;
pub mod http;
pub mod others;
pub mod pair;
pub mod plugins;
pub mod push;
pub mod server;
pub mod service;
pub mod sshd;
pub mod sshkey;
pub mod sshwire;
pub mod watch;
pub mod web;

pub use config::{Config, Peer, load_config, save_config, token};
pub use ctx::Ctx;
pub use plugins::{PLUGIN_CLOUDFLARED, PLUGIN_REMOTE, PLUGIN_SSH, PLUGIN_TAILSCALE};
pub use server::{Server, Session, StartRequest};

#[cfg(test)]
mod tests_server;
#[cfg(test)]
mod tests_sshd;
#[cfg(test)]
pub(crate) mod testutil;

// Every test runs in a throwaway home (rush-testenv).
#[cfg(test)]
rush_testenv::sandboxed!();
