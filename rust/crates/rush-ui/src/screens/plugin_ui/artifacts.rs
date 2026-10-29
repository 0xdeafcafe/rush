//! The pages a session published with the Artifact tool, newest first,
//! one row per page however many versions it went through; enter opens
//! one in the browser. After Go's internal/ui/artifacts.go (artifact
//! :19, artifacts :33, artifactsOf :81, artifactLines :91) and
//! convo/render.go:1830 URLIn.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use rush_convo::{Item, Session, Status};
use rush_core::text::Line;
use rush_core::time::Ms;

use crate::style::*;

/// A page the session published, at its latest version (artifact).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    pub file: String,
    pub about: String,
    pub version: i64,
    pub versions: usize,
    pub turn: u32,
    pub at: Ms,
}

static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(Version (\d+)\)").unwrap());

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Input {
    action: String,
    file_path: String,
    description: String,
    url: String,
}

/// What the session published, newest first (artifacts).
pub fn artifacts(s: &Session) -> Vec<Artifact> {
    let mut by_url: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<Artifact> = Vec::new();
    for t in &s.turns {
        for it in &t.items {
            let Item::Step(id) = it else { continue };
            let st = s.step(*id);
            if &*st.tool != "Artifact" || st.status != Status::Ok {
                continue;
            }
            let inp: Input = serde_json::from_str(st.input_json()).unwrap_or_default();
            if !inp.action.is_empty() && inp.action != "publish" {
                continue;
            }
            let mut url = url_in(&st.output).to_string();
            if url.is_empty() {
                url = inp.url.clone();
            }
            if url.is_empty() {
                continue;
            }
            let i = *by_url.entry(url.clone()).or_insert_with(|| {
                order.push(Artifact {
                    url: url.clone(),
                    ..Artifact::default()
                });
                order.len() - 1
            });
            let a = &mut order[i];
            a.versions += 1;
            if let Some(m) = VERSION.captures(&st.output) {
                a.version = m[1].parse().unwrap_or(0);
            }
            if !inp.file_path.is_empty() {
                a.file = inp.file_path;
            }
            if !inp.description.is_empty() {
                a.about = inp.description;
            }
            a.turn = t.n;
            a.at = st.end;
        }
    }
    order.reverse();
    order
}

/// Artifacts, worked out again only when the session changes
/// (artifactsOf).
#[derive(Default)]
pub struct Kept {
    key: (u64, usize, Ms),
    arts: Vec<Artifact>,
}

impl Kept {
    pub fn of(&mut self, s: &Session) -> &[Artifact] {
        let last = s.turns.last().map(|t| t.end.max(t.start)).unwrap_or(0);
        let key = (s.serial(), s.turns.len(), last);
        if self.key != key || self.arts.is_empty() && !s.turns.is_empty() {
            self.arts = artifacts(s);
            self.key = key;
        }
        &self.arts
    }
}

/// The artifacts view: each published page with what it is, its version
/// and when, as rows with their refs ("art:<url>") (artifactLines).
pub fn artifact_lines(
    arts: &[Artifact],
    w: usize,
    now: Ms,
    selected: &str,
    focused: bool,
) -> Vec<(Line, String)> {
    let mut out = Vec::new();
    let mut push = |l: Line, r: &str| out.push((fit(l, w), r.to_string()));
    push(
        line([
            plain("  "),
            spb(format!("Artifacts  {}", arts.len()), SUB),
            plain("   "),
            sp("pages this session published · enter opens one", DIM),
        ]),
        "",
    );
    push(
        line([plain("  "), sp("─".repeat(w.saturating_sub(4)), FAINT)]),
        "",
    );
    for a in arts {
        let r = format!("art:{}", a.url);
        let p = Path::new(&a.file);
        let mut name = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() || name == "." {
            name = a.url[a.url.rfind('/').map_or(0, |i| i + 1)..].to_string();
        }
        let mut meta = format!("#{}", a.turn);
        if a.version > 0 {
            meta = format!("v{} · {meta}", a.version);
        }
        if a.at != 0 {
            meta += &format!(" · {} ago", dur(now - a.at));
        }
        let top = super::views::spread(
            line([plain("  "), sp("◆ ", BLUE), spb(name, TEXT)]),
            line([sp(meta, DIM), plain("  ")]),
            w,
        );
        let rows = [
            top,
            line([
                plain("      "),
                sp(
                    rush_draw::truncate(&one_line(&a.about), 10.max(w.saturating_sub(10)), "…")
                        .into_owned(),
                    SUB,
                ),
            ]),
            line([plain("      "), sp(a.url.clone(), FAINT)]),
        ];
        for row in rows {
            if r == selected {
                let bar = if focused {
                    sp("▍", ORANGE)
                } else {
                    sp("▍", FAINT)
                };
                // The bar stands in the row's first cell, a blank.
                let mut rest = fit(row, w);
                if let Some(f) = rest.spans.first_mut() {
                    f.text.remove(0);
                }
                let mut l = line([bar]);
                join(&mut l, rest);
                push(highlight(l, w), &r);
            } else {
                push(row, &r);
            }
        }
        push(Line::new(), "");
    }
    out
}

/// The first link in s, whoever's it is; "" when there's none (URLIn).
pub fn url_in(s: &str) -> &str {
    let mut from = 0;
    while let Some(k) = s[from..].find("http") {
        let i = from + k;
        let mut j = i + 4;
        if s[j..].starts_with("s://") {
            j += 4;
        } else if s[j..].starts_with("://") {
            j += 3;
        } else {
            from = i + 4;
            continue;
        }
        let n = url_len(&s[j..]);
        if n > 0 {
            return &s[i..j + n];
        }
        from = j;
    }
    ""
}

/// How much of s, just past a URL's scheme, is the URL (render.go:2078
/// urlLen).
fn url_len(s: &str) -> usize {
    let b = s.as_bytes();
    let (mut angle, mut paren, mut end) = (0, 0, 0);
    while end < b.len()
        && !matches!(
            b[end],
            b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b']' | b'"' | b'\'' | b'`' | b','
        )
    {
        match b[end] {
            b'<' => angle += 1,
            b'(' => paren += 1,
            b'>' => {
                if angle == 0 {
                    break;
                }
                angle -= 1;
            }
            b')' => {
                if paren == 0 {
                    break;
                }
                paren -= 1;
            }
            _ => {}
        }
        end += 1;
    }
    s[..end].trim_end_matches(['.', ':', ';', '!', '?']).len()
}
