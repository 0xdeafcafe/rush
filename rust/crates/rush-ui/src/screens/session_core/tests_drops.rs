//! Image files dropped onto the terminal arrive as a paste of their paths;
//! in a Session's box each becomes [Image #N] at once, where it was
//! dropped, and a path typed out becomes one once a space follows it.
//! After Go's imagerefs.go (inline :36), imagerefs_test.go, model.go:1420
//! and rushmode.go:4483.

use super::conn;
use super::host::Op;
use super::tests::{KEY, hosted, press, typed};
use crate::frame::editor::{Look, look_disk};
use crate::screens::session_cards::imagerefs::ImageRefs;

/// A folder of image files, and other files, for a drop.
struct Files {
    dir: tempfile::TempDir,
}

impl Files {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "Screen Shot 1.png",
            "b.JPG",
            "c.gif",
            "d.webp",
            "e.jpeg",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }
        std::fs::create_dir(dir.path().join("folder.png")).unwrap();
        Files { dir }
    }
    fn p(&self, f: &str) -> String {
        self.dir.path().join(f).to_string_lossy().into_owned()
    }
}

#[test]
fn what_a_drop_becomes() {
    let f = Files::new();
    let (shot, b) = (f.p("Screen Shot 1.png"), f.p("b.JPG"));
    let esc = shot.replace(' ', "\\ ");
    let url = format!("file://{}", shot.replace(' ', "%20"));
    let cases: Vec<(String, &str, Vec<String>)> = vec![
        // Escaped spaces, as most terminals drop a file.
        (esc.clone(), "[Image #1]", vec![shot.clone()]),
        // Quoted, single or double.
        (format!("'{shot}'"), "[Image #1]", vec![shot.clone()]),
        (format!("\"{shot}\""), "[Image #1]", vec![shot.clone()]),
        // A file URL, its escapes undone; file://localhost too.
        (url.clone(), "[Image #1]", vec![shot.clone()]),
        (
            url.replace("file://", "file://localhost"),
            "[Image #1]",
            vec![shot.clone()],
        ),
        // Several in one drop, by spaces or lines; every kind of image.
        (
            format!("{esc} {b}"),
            "[Image #1] [Image #2]",
            vec![shot.clone(), b.clone()],
        ),
        (
            format!("{}\n{}\n{}\n", f.p("c.gif"), f.p("d.webp"), f.p("e.jpeg")),
            "[Image #1]\n[Image #2]\n[Image #3]\n",
            vec![f.p("c.gif"), f.p("d.webp"), f.p("e.jpeg")],
        ),
        // Words round a path keep their places.
        (
            format!("look at {b} please"),
            "look at [Image #1] please",
            vec![b.clone()],
        ),
        // Not images, not there, or a folder: text as they are.
        (f.p("notes.txt"), "", vec![]),
        (f.p("missing.png"), "", vec![]),
        (f.p("folder.png"), "", vec![]),
        ("just words".into(), "", vec![]),
        // An image beside a file that isn't one: only the image.
        (format!("{} {b}", f.p("notes.txt")), "", vec![b.clone()]),
    ];
    let look: Look = &look_disk;
    for (paste, want, files) in cases {
        let mut r = ImageRefs::default();
        let got = r.inline(&paste, look);
        let mut paths: Vec<(usize, String)> = r.path.into_iter().collect();
        paths.sort();
        let paths: Vec<String> = paths.into_iter().map(|(_, p)| p).collect();
        assert_eq!(paths, files, "{paste:?}");
        match got {
            None => assert!(files.is_empty(), "{paste:?}"),
            Some(t) if !want.is_empty() => assert_eq!(t, want, "{paste:?}"),
            Some(_) => {}
        }
    }
}

/// A drop on the Session's box is [Image #N] at once, and goes out as the
/// image with the message.
#[test]
fn a_drop_on_the_box_is_an_image_at_once() {
    let f = Files::new();
    let (mut m, fake) = hosted("steer", Vec::new());
    typed(&mut m, "what's this");
    press(&mut m, "space");
    m.paste(&f.p("Screen Shot 1.png").replace(' ', "\\ "));
    assert_eq!(conn(&mut m, KEY).input.string(), "what's this [Image #1]");
    fake.ops.lock().unwrap().clear();
    press(&mut m, "enter");
    let sent = fake.ops().into_iter().find_map(|o| match o {
        Op::Send { text, images, .. } => Some((text, images)),
        _ => None,
    });
    assert_eq!(
        sent,
        Some((
            "what's this [Image #1]".to_string(),
            vec![f.p("Screen Shot 1.png")]
        ))
    );
}

/// A path typed out, or dropped in as keys, becomes the image once a space
/// follows it, the cursor where it was from the end.
#[test]
fn a_typed_path_becomes_an_image_at_its_space() {
    let f = Files::new();
    let (mut m, _fake) = hosted("steer", Vec::new());
    typed(&mut m, &f.p("b.JPG"));
    assert_eq!(conn(&mut m, KEY).input.string(), f.p("b.JPG"));
    press(&mut m, "space");
    let c = conn(&mut m, KEY);
    assert_eq!(c.input.string(), "[Image #1] ");
    assert_eq!(c.input.pos, c.input.text.len());
}
