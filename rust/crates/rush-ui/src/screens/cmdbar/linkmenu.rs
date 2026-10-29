//! The menu on a link: a file opened (in Preview when it's an image or a
//! PDF), looked at, found or its path copied; a web address opened or
//! copied; a link of a kind rush knows offers its kind's first, a pull
//! request's Open in rush and Merge… (linkkinds.rs). Port of
//! internal/ui/linkmenu.go:17-201 (previewable :23, openLinkMenu :93 and
//! its kind's acts :109-111, linkActs :144, run :171, quickLook :186). The
//! link under a click (linkAt, rush_core::text::link_at) and a picked
//! step's (pickedLink) are the conversation's (session_core/
//! conversation.rs). The menu's picture (drawShot) waits on images in the
//! terminal.

use std::process::Command;

use super::linkkinds::{KindAct, known_link};
use crate::frame::{Out, Pick, Picker};
use crate::model::Model;
use crate::style::tildify;

/// The files Preview opens; the rest get Quick Look (previewable).
pub fn previewable(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "heic"
            | "tif"
            | "tiff"
            | "bmp"
            | "pdf"
            | "psd"
            | "ico"
    )
}

/// One thing the menu can do with a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAct {
    Browse(String),
    CopyLink(String),
    Open(String),
    Preview(String),
    QuickLook(String),
    Reveal(String),
    CopyPath(String),
}

impl LinkAct {
    pub fn label(&self) -> &'static str {
        match self {
            LinkAct::Browse(_) => "Open in browser",
            LinkAct::CopyLink(_) => "Copy link",
            LinkAct::Open(_) => "Open",
            LinkAct::Preview(_) => "Open with Preview",
            LinkAct::QuickLook(_) => "Quick Look",
            LinkAct::Reveal(_) => "Reveal in Finder",
            LinkAct::CopyPath(_) => "Copy path",
        }
    }
}

/// A file: URL's path, unescaped; None for any other link.
fn file_path(target: &str) -> Option<String> {
    let rest = target
        .strip_prefix("file://")
        .or_else(|| target.strip_prefix("file:"))?;
    // file://host/path: the path starts at the first slash.
    let path = if rest.starts_with('/') {
        rest
    } else {
        &rest[rest.find('/')?..]
    };
    Some(unescape(path))
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What can be done with a link (linkActs); empty for one that isn't.
pub fn link_acts(target: &str) -> Vec<LinkAct> {
    if target.trim().is_empty() || target.contains(char::is_whitespace) {
        return Vec::new();
    }
    let Some(p) = file_path(target) else {
        return vec![
            LinkAct::Browse(target.into()),
            LinkAct::CopyLink(target.into()),
        ];
    };
    let mut acts = vec![LinkAct::Open(p.clone())];
    if previewable(&p) {
        acts.push(LinkAct::Preview(p.clone()));
    }
    acts.extend([
        LinkAct::QuickLook(p.clone()),
        LinkAct::Reveal(p.clone()),
        LinkAct::CopyPath(p),
    ]);
    acts
}

fn base(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Runs a command off the UI thread, saying done when it's done or why it
/// couldn't (run).
fn run(m: &mut Model, done: String, name: &'static str, args: Vec<String>) {
    m.later(
        move || match Command::new(name).args(&args).output() {
            Ok(o) if o.status.success() => Ok(done),
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stderr).trim().to_string();
                Err(if s.is_empty() {
                    o.status.to_string()
                } else {
                    s
                })
            }
            Err(e) => Err(e.to_string()),
        },
        |m, r| {
            match r {
                Ok(t) => m.flash(&t),
                Err(e) => m.flash_err(&e),
            }
            Out::Draw
        },
    );
}

/// Does what an act says.
pub fn do_act(m: &mut Model, a: &LinkAct) -> Out {
    match a {
        LinkAct::Browse(u) => run(m, format!("opened {u}"), "open", vec![u.clone()]),
        LinkAct::CopyLink(u) | LinkAct::CopyPath(u) => m.copy_text(u.clone()),
        LinkAct::Open(p) => run(m, format!("opened {}", base(p)), "open", vec![p.clone()]),
        LinkAct::Preview(p) => run(
            m,
            format!("opened {} in Preview", base(p)),
            "open",
            vec!["-a".into(), "Preview".into(), p.clone()],
        ),
        LinkAct::QuickLook(p) => {
            // qlmanage stays until the panel is closed, so it's left to run.
            let p = p.clone();
            m.later(
                move || {
                    Command::new("qlmanage")
                        .args(["-p", &p])
                        .spawn()
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                },
                |m, r| {
                    if let Err(e) = r {
                        m.flash_err(&e);
                    }
                    Out::Draw
                },
            );
        }
        LinkAct::Reveal(p) => run(
            m,
            format!("revealed {}", base(p)),
            "open",
            vec!["-R".into(), p.clone()],
        ),
    }
    Out::Draw
}

/// One row of the menu on a link: its kind's, or any link's.
enum MenuAct {
    Kind(KindAct),
    Link(LinkAct),
}

impl MenuAct {
    fn label(&self) -> &'static str {
        match self {
            MenuAct::Kind(k) => k.label,
            MenuAct::Link(a) => a.label(),
        }
    }
}

/// Opens the menu for a link, reporting whether it could (openLinkMenu).
pub fn open_link_menu(m: &mut Model, target: &str) -> bool {
    let link = link_acts(target);
    if link.is_empty() {
        return false;
    }
    // What its kind does with it comes first.
    let mut acts: Vec<MenuAct> = match known_link(target) {
        Some((h, v)) => h
            .acts
            .map(|f| f(target, &v))
            .unwrap_or_default()
            .into_iter()
            .map(MenuAct::Kind)
            .collect(),
        None => Vec::new(),
    };
    acts.extend(link.into_iter().map(MenuAct::Link));
    let title = file_path(target).map_or_else(|| target.to_string(), |p| tildify(&p));
    let rows = acts
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mut p = Pick::new(a.label());
            p.value = i.to_string();
            p
        })
        .collect();
    let picker = Picker::new(
        title,
        rows,
        Box::new(move |m, pick, _| {
            let Some(i) = pick.and_then(|p| p.value.parse::<usize>().ok()) else {
                return Out::Idle;
            };
            match &acts[i] {
                MenuAct::Kind(k) => (k.run)(m),
                MenuAct::Link(a) => do_act(m, a),
            }
        }),
    );
    m.open_sheet(Box::new(picker));
    true
}
