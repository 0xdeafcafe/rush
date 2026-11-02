//! probe is a plugin that tries to get out of its sandbox, for the sandbox
//! tests. Asked to "probe", it attempts each thing it should not be able
//! to do and says which it managed.
//! Ported from rust/crates/rush-plugin/tests/testdata/probe/main.go.

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    rush_plugin_sdk::serve(|_, _, p| Ok(json!(probe(&p))))
}

fn probe(p: &Value) -> BTreeMap<&'static str, String> {
    let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
    let (secret, outside, sock, data) = (s("secret"), s("outside"), s("sock"), s("data"));
    let ok = |r: std::io::Result<()>| match r {
        Ok(()) => "allowed".to_string(),
        Err(e) => format!("denied: {e}"),
    };
    let dial = |addr: &str| {
        let a: SocketAddr = addr.parse().map_err(std::io::Error::other)?;
        TcpStream::connect_timeout(&a, Duration::from_secs(2)).map(drop)
    };
    let mut out = BTreeMap::new();
    out.insert("read secret", ok(std::fs::read(&secret).map(drop)));
    out.insert("write outside", ok(std::fs::write(&outside, "x")));
    out.insert(
        "write data",
        ok(std::fs::write(Path::new(&data).join("x"), "x")),
    );
    let secret_dir = Path::new(&secret).parent().unwrap_or(Path::new("/"));
    out.insert(
        "list secret dir",
        ok(std::fs::read_dir(secret_dir).map(drop)),
    );
    out.insert(
        "exec",
        ok(std::process::Command::new("/bin/echo")
            .arg("hi")
            .output()
            .map(drop)),
    );
    out.insert("internet", ok(dial("1.1.1.1:443")));
    out.insert("localhost", ok(dial("127.0.0.1:22")));
    out.insert("rush socket", ok(UnixStream::connect(&sock).map(drop)));
    if let Some(port) = p["proxy"].as_u64().filter(|&n| n > 0) {
        out.insert("proxy", ok(dial(&format!("127.0.0.1:{port}"))));
    }
    out.insert("home", std::env::var("HOME").unwrap_or_default());
    out.insert(
        "token",
        std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
    );
    out
}
