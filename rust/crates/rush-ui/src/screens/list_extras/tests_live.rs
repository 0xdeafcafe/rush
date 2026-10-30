//! The preview's live mirror, after Go's live.go: the picked agent's own
//! screen drawn beside the list once it draws, sized to the pane; let go
//! while a session's own screen has the terminal (live.go:238) and when
//! the pane moves on; v swaps it for the summary (keys.go:491), the
//! strip staying over it; typing goes into it once it has the keys
//! (embed.go). The agent's service is a fake here: nothing is attached
//! for real.

use std::io::Write;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use rush_core::adapter::{
    self, Adapter, Features, Joiner, Kind, Level, Mirror, Profile, Registry, Terminal,
};
use rush_core::agent::{Agent, State};

use super::tests::{agent, model_of};
use crate::frame::Press;
use crate::model::Model;
use crate::screens::cmdbar::attach::Attached;

/// What the fake's screens were asked, and what they draw.
#[derive(Clone, Default)]
struct Screens {
    did: Arc<Mutex<Vec<String>>>,
    /// The bytes the next mirror draws, then it waits to be let go.
    draws: Arc<Mutex<Vec<u8>>>,
    /// Told once a mirror has drawn what it was given.
    drew: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    /// What the mirrors' sessions were answered.
    answers: Arc<Mutex<Vec<u8>>>,
    /// Told each write to a mirror's session, once set.
    wrote: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
}

/// The fake's side of an attachment: dropping it lets its relay end.
struct Conn {
    answers: Arc<Mutex<Vec<u8>>>,
    wrote: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
    _end: mpsc::Sender<()>,
}

impl Write for Conn {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.answers.lock().unwrap().extend_from_slice(b);
        if let Some(tx) = self.wrote.lock().unwrap().as_ref() {
            let _ = tx.send(b.to_vec());
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Adapter for Screens {
    fn kind(&self) -> Kind {
        Kind::from("screens")
    }
    fn name(&self) -> String {
        "Screens Code".into()
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn level(&self) -> Level {
        Level::default()
    }
    fn profiles(&self) -> Vec<Profile> {
        Vec::new()
    }
    rush_core::has!(joiner: Joiner);
}

impl Joiner for Screens {
    fn join(&self, _: &Profile, _: &str) -> Box<dyn Terminal> {
        unreachable!("the preview never joins")
    }
    fn service_up(&self, _: &Profile) -> bool {
        true
    }
    fn mirror(
        &self,
        _: &Profile,
        id: &str,
        viewer: &str,
        cols: u16,
        rows: u16,
    ) -> adapter::Result<Mirror> {
        let viewer = viewer.split('-').take(2).collect::<Vec<_>>().join("-");
        self.did
            .lock()
            .unwrap()
            .push(format!("mirror {id} as {viewer} {cols}x{rows}"));
        let (end_tx, end_rx) = mpsc::channel::<()>();
        let draws = std::mem::take(&mut *self.draws.lock().unwrap());
        let drew = self.drew.lock().unwrap().take();
        Ok(Mirror {
            modes: vec![25],
            conn: Box::new(Conn {
                answers: self.answers.clone(),
                wrote: self.wrote.clone(),
                _end: end_tx,
            }),
            relay: Box::new(move |w| {
                w.write_all(&draws)?;
                if let Some(d) = drew {
                    let _ = d.send(());
                }
                // Until it's let go.
                let _ = end_rx.recv();
                Ok(())
            }),
        })
    }
    fn resize(&self, _: &Profile, id: &str, _: &str, cols: u16, rows: u16) -> adapter::Result<()> {
        self.did
            .lock()
            .unwrap()
            .push(format!("resize {id} {cols}x{rows}"));
        Ok(())
    }
}

fn running(id: &str) -> Agent {
    let mut a = agent(id, &format!("tidy {id}"), State::Working, 0);
    a.kind = Kind::from("screens");
    a.interactive = false;
    a.worker = Some(4242);
    a
}

/// A wide screen (the Session beside the list) with two agents of the
/// fake's, the first picked.
fn wide(s: &Screens) -> Model {
    let mut m = model_of(vec![running("s1"), running("s2")], 200, 40);
    let reg = Registry::new();
    reg.register(s.clone());
    m.reg = reg;
    m.sel = "default/s1".into();
    m
}

fn screen(m: &mut Model) -> Vec<String> {
    m.frame().iter().map(|l| l.plain()).collect()
}

#[test]
fn the_preview_mirrors_the_agents_screen() {
    let s = Screens::default();
    *s.draws.lock().unwrap() = b"\x1b[1;1H> fix the tests\x1b[6n".to_vec();
    let (tx, rx) = mpsc::channel();
    *s.drew.lock().unwrap() = Some(tx);
    let mut m = wide(&s);
    // The first frame asks the service and attaches, sized to the pane.
    screen(&mut m);
    let did = s.did.lock().unwrap().clone();
    assert_eq!(did.len(), 1, "{did:?}");
    assert!(did[0].starts_with("mirror s1 as rush-preview "), "{did:?}");
    rx.recv().unwrap();
    // Its query is answered up the stream.
    assert_eq!(&*s.answers.lock().unwrap(), b"\x1b[1;16R");
    let f = screen(&mut m);
    let at = f
        .iter()
        .position(|l| l.contains("tidy s1  ● live"))
        .expect("its title");
    assert!(f[at - 1].contains(" screen   summary "), "{}", f[at - 1]);
    assert!(
        f[at - 1].contains("typing goes into it · ctrl+] leaves · ctrl+f full screen"),
        "{}",
        f[at - 1]
    );
    assert!(f[at + 1].contains("> fix the tests"), "{}", f[at + 1]);

    // v: the summary in its place, and back.
    m.press_key(&Press::of("v").unwrap());
    let f = screen(&mut m);
    assert!(!f.iter().any(|l| l.contains("● live")));
    m.press_key(&Press::of("v").unwrap());
    assert!(screen(&mut m).iter().any(|l| l.contains("● live")));

    // A session's own screen has the terminal: no mirror meanwhile.
    m.kept::<Attached>().key = "default/s2".into();
    screen(&mut m);
    assert!(!screen(&mut m).iter().any(|l| l.contains("● live")));
    assert_eq!(s.did.lock().unwrap().len(), 1, "none opened meanwhile");
    m.kept::<Attached>().key.clear();

    // The pick moves on: the next agent's screen is attached instead.
    m.sel = "default/s2".into();
    screen(&mut m);
    let did = s.did.lock().unwrap().clone();
    assert!(did[1].starts_with("mirror s2 as rush-preview "), "{did:?}");
}

/// attach lets the mirror go through close_live before the terminal is
/// handed over (model.go:2683).
#[test]
fn attach_lets_the_mirror_go() {
    let s = Screens::default();
    *s.draws.lock().unwrap() = b"ready".to_vec();
    let (tx, rx) = mpsc::channel();
    *s.drew.lock().unwrap() = Some(tx);
    let mut m = wide(&s);
    screen(&mut m);
    rx.recv().unwrap();
    assert!(screen(&mut m).iter().any(|l| l.contains("● live")));
    let close = m.kept::<Attached>().close_live.expect("set by the mirror");
    close(&mut m);
    // Let go, and opened again only on the next frame that shows it.
    let f: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(!f.iter().any(|l| l.contains("● live")));
}

/// The screen up beside the list, drawn: its rows.
fn shown(s: &Screens, draws: &[u8]) -> Model {
    *s.draws.lock().unwrap() = draws.to_vec();
    let (tx, rx) = mpsc::channel();
    *s.drew.lock().unwrap() = Some(tx);
    let mut m = wide(s);
    screen(&mut m);
    rx.recv().unwrap();
    assert!(screen(&mut m).iter().any(|l| l.contains("● live")));
    m
}

/// → gives the screen the keys: each goes to the session as a terminal
/// sends it, a paste bracketed, the title and strip say so and the box
/// waits; ctrl+] hands them back (embed.go, view.go:1497, :2760).
#[test]
fn typing_goes_into_the_screen() {
    let s = Screens::default();
    let mut m = shown(&s, b"> ");
    let (tx, rx) = mpsc::channel();
    *s.wrote.lock().unwrap() = Some(tx);
    m.press_key(&Press::of("right").unwrap());
    assert!(super::live::embedded(&m));
    let f = screen(&mut m);
    assert!(
        f.iter().any(|l| l.contains(
            "SESSION  tidy s1  ·  keys go to Screens Code · ctrl+] or click Agents to come back"
        )),
        "{f:#?}"
    );
    assert!(
        f.iter()
            .any(|l| l.contains("typing into it · ctrl+] comes back")),
        "{f:#?}"
    );
    assert!(
        f.iter()
            .any(|l| l.contains("ctrl+] or click here to come back")),
        "{f:#?}"
    );
    for k in ["a", "enter", "ctrl+c", "up", "q"] {
        m.press_key(&Press::of(k).unwrap());
    }
    m.paste("two\nlines");
    let got: Vec<Vec<u8>> = (0..6).map(|_| rx.recv().unwrap()).collect();
    assert_eq!(
        got,
        [
            b"a".to_vec(),
            b"\r".to_vec(),
            vec![3],
            b"\x1b[A".to_vec(),
            b"q".to_vec(),
            b"\x1b[200~two\nlines\x1b[201~".to_vec(),
        ]
    );
    assert!(
        m.sheet.is_none() && m.input.is_empty(),
        "nothing else had them"
    );
    m.press_key(&Press::of("ctrl+]").unwrap());
    assert!(!super::live::embedded(&m));
    let f = screen(&mut m);
    assert!(f.iter().any(|l| l.contains("tidy s1  ● live")), "{f:#?}");
    // A click on the screen gives it the keys again.
    let (lw, _) = super::split::widths(&m);
    m.mouse_event(&crate::frame::Mouse {
        x: (lw + 10) as u16,
        y: 10,
        kind: rush_draw::MouseKind::Down(rush_draw::Button::Left),
        mods: rush_draw::KeyMods::NONE,
    });
    assert!(super::live::embedded(&m));
}

/// v swaps the screen for the summary under the same strip, its summary
/// tab lit, so v again brings the screen back (view.go:1225).
#[test]
fn the_summary_keeps_the_strip() {
    let s = Screens::default();
    let mut m = shown(&s, b"> ");
    m.press_key(&Press::of("v").unwrap());
    let f = screen(&mut m);
    assert!(!f.iter().any(|l| l.contains("● live")), "{f:#?}");
    assert!(
        f.iter().any(|l| l.contains(" screen   summary ")),
        "the strip stays: {f:#?}"
    );
    // The keys stay the list's: the summary isn't typed into.
    m.press_key(&Press::of("right").unwrap());
    assert!(!super::live::embedded(&m));
}
