//! The menu on a link: a file opened (in Preview when it's an image or a
//! PDF), looked at, found or its path copied; a web address opened or
//! copied; a link of a kind rush knows offers its kind's first, a pull
//! request's Open in rush and Merge… (linkkinds.rs). Port of
//! internal/ui/linkmenu.go:17-201 (previewable :23, openLinkMenu :93 and
//! its kind's acts :109-111, shotMsg :122, drawShot :136, linkActs :144,
//! run :171, quickLook :186). The link under a click (linkAt,
//! rush_core::text::link_at) and a picked step's (pickedLink) are the
//! conversation's (session_core/conversation.rs). An image's menu draws
//! it above the acts, in blocks made off the UI thread, and v draws it
//! as big as the window leaves room for (picker.go:323 "v", pickerBody's
//! shot :419-441, model.go:1292 shotMsg).

use std::process::Command;

use rush_core::text::{Line, Span};

use super::linkkinds::{KindAct, known_link};
use crate::frame::{Out, Pick, Picker, Press, Sheet};
use crate::model::Model;
use crate::screens::plugin_ui::{settled, sheet_do};
use crate::style::{keys, tildify};

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
    let n = acts.len();
    let mut picker = Picker::new(
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
    picker.keys = vec![
        ("↑↓".into(), "choose".into()),
        ("enter".into(), "do it".into()),
        ("esc".into(), "close".into()),
    ];
    // An image's menu shows it above what can be done with it.
    match file_path(target).filter(|p| previewable(p)) {
        Some(img) => {
            m.open_sheet(Box::new(LinkMenu {
                picker,
                img,
                shot: None,
                big: false,
                acts: n,
            }));
            draw_shot(m);
        }
        None => m.open_sheet(Box::new(picker)),
    }
    true
}

/// The menu on an image's link: the menu, with the image drawn above
/// its acts once it's made (picker's img, shot and big).
pub struct LinkMenu {
    pub picker: Picker,
    /// The image file the acts are for.
    pub img: String,
    /// img drawn, once it's made.
    pub shot: Option<Vec<Line>>,
    /// img drawn as big as the window allows.
    pub big: bool,
    acts: usize,
}

/// A menu's image drawn: for the menu on path at that size (shotMsg).
type Shot = (String, bool, Vec<Line>);

/// Draws the open menu's image off the UI thread: small, or as big as
/// the window leaves room for and finer; nothing when there's no room
/// (drawShot). The one shown stays until this lands.
fn draw_shot(m: &mut Model) {
    let Some(s) = m.sheet_as::<LinkMenu>() else {
        return;
    };
    let (path, big, acts) = (s.img.clone(), s.big, s.acts);
    shot_for(m, path, big, acts);
}

fn shot_for(m: &mut Model, path: String, big: bool, acts: usize) {
    // The box's edges and padding, the title, the acts and the keys.
    let room_h = m.h as isize - 12 - acts as isize;
    let (w, h) = if big {
        (m.w as isize - 10, room_h)
    } else {
        ((m.w as isize - 10).min(48), room_h.min(12))
    };
    if w < 8 || h < 4 {
        return;
    }
    let (w, h) = (w as usize, h as usize);
    sheet_do::<LinkMenu, Shot>(
        m,
        move || {
            let rows = crate::screens::session_cards::pictures::preview(&path, w, h, big);
            (path, big, rows)
        },
        took_shot,
    );
}

/// The image drawn lands, if the menu is still on it at that size.
fn took_shot(s: &mut LinkMenu, _m: &mut Model, (path, big, rows): Shot) -> Out {
    if s.img != path || s.big != big {
        return Out::Idle;
    }
    s.shot = (!rows.is_empty()).then_some(rows);
    Out::Draw
}

impl Sheet for LinkMenu {
    /// The window's width while it shows its image big (pickerWide).
    fn width(&self, m: &Model) -> usize {
        if self.big && self.shot.is_some() {
            m.w.saturating_sub(6)
        } else {
            self.picker.width(m)
        }
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if k.s == "v" {
            self.big = !self.big;
            shot_for(m, self.img.clone(), self.big, self.acts);
            return settled(self, m, took_shot).unwrap_or(Out::Draw);
        }
        self.picker.key(m, k)
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        let mut out = self.picker.body(m, w, h);
        let Some(shot) = &self.shot else {
            return out;
        };
        // Its image, centred, between the title and the acts.
        let pad = w.saturating_sub(shot.first().map_or(0, Line::width)) / 2;
        let mut rows: Vec<Line> = shot
            .iter()
            .map(|r| {
                let mut l = Line::from_spans(vec![Span::plain(" ".repeat(pad))]);
                l.spans.extend(r.spans.iter().cloned());
                l
            })
            .collect();
        rows.push(Line::new());
        out.splice(2..2, rows);
        let size = if self.big { "smaller" } else { "bigger" };
        if let Some(last) = out.last_mut() {
            *last = keys(&[
                ("↑↓", "choose"),
                ("enter", "do it"),
                ("v", size),
                ("esc", "close"),
            ]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(m: &mut Model) -> &mut LinkMenu {
        m.sheet_as::<LinkMenu>().expect("an image's menu")
    }

    // linkmenu.go drawShot and picker.go "v": an image's menu draws it
    // above its acts; v draws it big, the window's width, and back.
    #[test]
    fn an_image_link_shows_its_picture() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("shot.png");
        let mut img = image::RgbaImage::new(400, 200);
        for (x, _, px) in img.enumerate_pixels_mut() {
            *px = image::Rgba([if x < 200 { 200 } else { 10 }, 40, 90, 255]);
        }
        img.save(&p).unwrap();
        let mut m = Model::new("t");
        (m.w, m.h) = (120, 50);
        let u = format!("file://{}", p.display());
        assert!(open_link_menu(&mut m, &u));
        let other = Model::new("t");
        let s = menu(&mut m);
        let shot = s.shot.clone().expect("drawn at once with no loop");
        assert_eq!((shot.len(), shot[0].width()), (12, 48));
        let body: Vec<String> = s.body(&other, 100, 40).iter().map(Line::plain).collect();
        assert!(
            body[2].trim().chars().all(|c| "▀▄ ".contains(c)),
            "{body:?}"
        );
        assert_eq!(
            body.last().unwrap().trim(),
            "↑↓ choose  ·  enter do it  ·  v bigger  ·  esc close"
        );
        m.sheet_key(&Press::of("v").unwrap());
        let s = menu(&mut m);
        assert!(s.big);
        let big = s.shot.clone().expect("drawn big");
        assert_eq!(big[0].width(), 110);
        m.sheet_key(&Press::of("v").unwrap());
        assert!(!menu(&mut m).big);
        assert_eq!(menu(&mut m).shot.as_ref().unwrap()[0].width(), 48);
        // A web address's menu has no picture.
        assert!(open_link_menu(&mut m, "https://example.com/a.png"));
        assert!(m.sheet_as::<LinkMenu>().is_none());
    }
}
