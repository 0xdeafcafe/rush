//! The web app: Go's own files (internal/remote/web), built into the
//! binary as Go embeds them, and served as Go's http.FileServerFS serves
//! an embedded folder (server.go :31, :95).

use crate::http::{Request, Response};

/// Each file, its name and what it holds.
const FILES: [(&str, &[u8]); 6] = [
    (
        "index.html",
        include_bytes!("../../../../internal/remote/web/index.html"),
    ),
    (
        "app.css",
        include_bytes!("../../../../internal/remote/web/app.css"),
    ),
    (
        "app.js",
        include_bytes!("../../../../internal/remote/web/app.js"),
    ),
    (
        "icon.svg",
        include_bytes!("../../../../internal/remote/web/icon.svg"),
    ),
    (
        "manifest.webmanifest",
        include_bytes!("../../../../internal/remote/web/manifest.webmanifest"),
    ),
    (
        "sw.js",
        include_bytes!("../../../../internal/remote/web/sw.js"),
    ),
];

/// A file's type, as Go's mime package names it.
fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map_or("", |(_, e)| e) {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

/// The file r asks for.
pub fn serve(r: &Request) -> Response {
    if r.method != "GET" && r.method != "HEAD" {
        let mut x = Response::new(405, "Method Not Allowed\n");
        x.set("Content-Type", "text/plain; charset=utf-8");
        return x;
    }
    // FileServer sends /index.html to its folder.
    if r.path == "/index.html" {
        let mut x = Response::new(301, Vec::new());
        x.set("Location", "./");
        return x;
    }
    let name = match r.path.trim_start_matches('/') {
        "" => "index.html",
        n => n,
    };
    match FILES.iter().find(|(n, _)| *n == name) {
        Some((n, b)) => {
            let mut x = Response::new(200, b.to_vec());
            x.set("Content-Type", content_type(n));
            x.set("Accept-Ranges", "bytes");
            x
        }
        None => {
            let mut x = Response::new(404, "404 page not found\n");
            x.set("Content-Type", "text/plain; charset=utf-8");
            x.set("X-Content-Type-Options", "nosniff");
            x
        }
    }
}
