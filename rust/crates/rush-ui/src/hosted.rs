//! The view of one rush-mode session alone (`rush open <id>`), for
//! embedding in another app: rush's header, then its Session at the whole
//! width, with no Agents list and no keys that lead to another agent.
//! Projects, Harnesses and Settings open with ctrl+\, and Agents is this
//! session; ctrl+6 shows every agent beside it, and hides them again. Only
//! ctrl+q quits: esc never leaves the session. The session's host keeps
//! running. After Go's internal/ui/hosted.go (NewHosted :26, hostedSnap
//! :39, leads :74, hostedAlone :79, listToggleKey :82, allAgents :96,
//! toggleList :133, pinHosted :178, hostedKeyGuard :236) and keys.go:43-52
//! (the toggle, the guard, no command bar).
//!
//! Past conversations are left out of each reading until the list is
//! shown (NewHosted's SkipPast, everyAgent :88). Init skips what Go's
//! does (model.go:556): the layout question, usage, quotas, logins, the
//! update check, plugin approvals and the stream (hosted::quiet).
//!
//! Not here: hostedShow (the command bar's way to another agent, which hosted has
//! no bar to reach).

use rush_core::agent::Agent;

use crate::frame::{Out, PlaceId, Press};
use crate::model::Model;
use crate::screens::list_extras::hosted::list_toggle_key;

/// The session hosted shows, and what the list last read.
#[derive(Default)]
pub struct Hosted {
    /// The session's rush id; empty when this isn't a hosted view.
    pub id: String,
    /// Its row's key, once the list has it.
    pub key: String,
    /// Agents shown beside it (ctrl+6).
    pub list: bool,
    /// Every agent the last reading had, the hidden ones too.
    pub all: Vec<Agent>,
    /// Its session couldn't be opened from the last reading.
    unread: bool,
    /// The keys are off the message box, after esc with nothing to stop
    /// (hostedAway): esc again only stops a turn, anything else goes back
    /// to the box.
    pub away: bool,
    /// Discovery's switch for past conversations, once it's running.
    discovery: Option<crate::worker::DiscoveryCtl>,
    /// The list was shown before discovery handed its switch over.
    every: bool,
}

/// Takes discovery's switch for past conversations, left out until the
/// list is shown; put back now if it already was.
pub(crate) fn set_discovery(m: &mut Model, ctl: crate::worker::DiscoveryCtl) -> Out {
    let h = m.kept::<Hosted>();
    if h.every {
        ctl.every_agent();
    }
    h.discovery = Some(ctl);
    Out::Idle
}

/// Puts past conversations back in the readings, for the list to show
/// every agent (everyAgent).
fn every_agent(m: &mut Model) {
    let h = m.kept::<Hosted>();
    if h.every {
        return;
    }
    h.every = true;
    if let Some(ctl) = &h.discovery {
        ctl.every_agent();
    }
}

/// Takes the keys off the box: alone there's no list to go back to, so
/// esc does this instead (paneKey's esc, hostedAlone).
pub(crate) fn set_away(m: &mut Model) {
    if peek(m).is_some() {
        m.kept::<Hosted>().away = true;
    }
}

/// Whether this rush leaves out what's about the app as a whole, as
/// hosted's Init does (model.go:556): usage, quotas, logins, the menu
/// bar, the update check, harness checks, plugin approvals, the stream.
pub fn quiet(m: &Model) -> bool {
    peek(m).is_some()
}

/// Whether the keys are off the box (hostedAway).
pub fn away(m: &Model) -> bool {
    peek(m).is_some_and(|h| h.away)
}

/// A key while the box doesn't have the keys (hostedAwayKey): esc stops
/// the turn if one runs and does nothing else; enter or → give the box
/// the keys back; any other key goes back to the box and on to it.
fn away_key(m: &mut Model, s: &str) -> Out {
    match s {
        "esc" => {
            if let Some(key) = crate::screens::session_core::open_key(m)
                && crate::screens::session_core::keys::can_interrupt(m, &key)
            {
                crate::screens::session_core::keys::stop_turn(m, &key);
                return Out::Draw;
            }
            Out::Idle
        }
        "enter" | "right" => {
            m.kept::<Hosted>().away = false;
            Out::Draw
        }
        _ => {
            m.kept::<Hosted>().away = false;
            Out::Pass
        }
    }
}

fn peek(m: &Model) -> Option<&Hosted> {
    m.kept.peek::<Hosted>().filter(|h| !h.id.is_empty())
}

/// Makes m the view of session id alone (NewHosted): Getting started
/// and the tips off, the session opened as soon as the list has it.
pub fn set(m: &mut Model, id: &str) {
    m.kept::<Hosted>().id = id.to_string();
    crate::screens::accounts::onboard::set_hosted(m, true);
}

/// The session id this view hosts, if it's a hosted view.
pub fn id(m: &Model) -> Option<&str> {
    peek(m).map(|h| h.id.as_str())
}

/// Hosted showing its one session, the list hidden (hostedAlone).
pub fn alone(m: &Model) -> bool {
    peek(m).is_some_and(|h| !h.list)
}

/// Every agent, hosted's hidden ones included (allAgents).
pub fn all_agents(m: &Model) -> &[Agent] {
    match peek(m) {
        Some(h) => &h.all,
        None => &m.agents,
    }
}

/// A reading of the list as hosted takes it: unless its list is shown,
/// only the one session is in it, so nothing else is listed, counted or
/// acted on (hostedSnap). Called before the list is built from it.
pub(crate) fn snap(m: &mut Model) {
    if peek(m).is_none() {
        return;
    }
    let mut h = std::mem::take(m.kept::<Hosted>());
    h.all = m.agents.clone();
    h.unread = false;
    let mine = m
        .agents
        .iter()
        .find(|a| a.rush && a.job.id == h.id)
        .cloned();
    if let Some(a) = &mine {
        h.key = a.key.clone();
    }
    if !h.list {
        m.agents = mine.into_iter().collect();
    }
    *m.kept::<Hosted>() = h;
}

/// Keeps the view on its session, whatever a key or message did to the
/// selection: Agents is the one session, opened at the whole width,
/// unless its list is shown (pinHosted). Called after every update and
/// reading; true when it opened the session.
pub(crate) fn pin(m: &mut Model) -> bool {
    let Some(h) = peek(m) else {
        return false;
    };
    if m.place != PlaceId::Agents || m.agents_page > 0 || h.list || h.key.is_empty() {
        return false;
    }
    let (key, unread) = (h.key.clone(), h.unread);
    m.sel = key.clone();
    m.shown = key.clone();
    let mut opened = false;
    if m.session.as_ref().is_none_or(|v| v.key != key)
        && !unread
        && let Some(i) = m.agents.iter().position(|a| a.key == key)
    {
        m.open(i);
        opened = m.session.as_ref().is_some_and(|v| v.key == key);
        // One that can't be read is tried again with the next reading,
        // not on every key.
        m.kept::<Hosted>().unread = !opened;
    }
    // Its rows may run the whole width (Options.Wide: hostedAlone).
    if let Some(v) = m.session.as_mut() {
        v.o.wide = true;
    }
    opened
}

/// Whether closing the open Session is refused: alone, there's no list to
/// go back to (leavePane).
pub(crate) fn keeps_open(m: &Model) -> bool {
    alone(m) && peek(m).is_some_and(|h| m.session.as_ref().is_some_and(|v| v.key == h.key))
}

/// Shows or hides Agents beside the session (toggleList's hosted half):
/// shown, the list has every agent, to pick and answer; hidden again, the
/// view is back on its own session with hosted's limits. The reading kept
/// is filtered again, not read anew.
pub fn toggle_list(m: &mut Model) -> Out {
    let h = m.kept::<Hosted>();
    h.list = !h.list;
    h.away = false;
    let (list, all) = (h.list, h.all.clone());
    if list {
        every_agent(m);
        // The list's own split shows the session beside it when there's
        // room.
        m.session = None;
        m.flash("Agents beside the session · ctrl+6 or esc hides them");
    }
    m.agents = all;
    snap(m);
    m.rebuild();
    pin(m);
    Out::Draw
}

/// The keys hosted takes before anything else sees them, in Go's order
/// (keys.go:43-52): ctrl+6 shows or hides the list; alone, the keys that
/// would leave the one session are dropped: the command bar, ctrl+n
/// to the next agent, { } and ← off the box, and , . < > between places.
/// ctrl+\ still goes to the next place (hostedKeyGuard).
pub fn key(m: &mut Model, k: &Press) -> Out {
    let s = k.s.as_str();
    let over = m.confirm.is_some() || m.sheet.is_some() || m.help;
    let Some(h) = peek(m) else {
        // Not hosted: ctrl+6 moves between the split and the Session
        // alone (toggleList), on the list's screen only.
        if list_toggle_key(s) && !over && !m.on_place() && !crate::screens::cmdbar::bar::is_open(m)
        {
            return crate::screens::list_extras::split::toggle(m);
        }
        return Out::Pass;
    };
    let list = h.list;
    if list_toggle_key(s) && !over && !m.on_place() {
        return toggle_list(m);
    }
    if list {
        return match s {
            "esc" if !over && m.session.is_none() && m.place == PlaceId::Agents => toggle_list(m),
            _ => Out::Pass,
        };
    }
    match s {
        "ctrl+q" | "ctrl+\\" => return Out::Pass,
        "ctrl+n" => return Out::Idle,
        _ => {}
    }
    if m.place_step(k) != 0 {
        return Out::Idle;
    }
    if m.on_place() || over {
        return Out::Pass; // the place's own keys, its pages included
    }
    let Some(key) = crate::screens::session_core::open_key(m) else {
        // Still opening: nothing to type into yet.
        return if s == "ctrl+c" { Out::Pass } else { Out::Idle };
    };
    if away(m) {
        let o = away_key(m, s);
        if o.used() {
            return o;
        }
    }
    let empty = crate::screens::session_core::conn(m, &key).input.is_empty();
    let picked = m.session.as_ref().is_some_and(|v| !v.sel.is_empty());
    match s {
        "{" | "}" if empty => Out::Idle,
        // ← only ever leaves the box for the list, which isn't there.
        "left" if empty && !picked => Out::Idle,
        // Tab completes a / command, and never goes to the list.
        "tab"
            if !crate::screens::session_core::conn(m, &key)
                .input
                .string()
                .starts_with('/') =>
        {
            Out::Idle
        }
        _ => Out::Pass,
    }
}

/// The command bar's keys, which hosted has none of: it reaches every
/// agent and place (keys.go:49).
pub fn no_bar(m: &Model) -> bool {
    peek(m).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::Fleet;

    fn row(key: &str, id: &str, rush: bool) -> Agent {
        let mut a = Agent {
            key: key.into(),
            display_name: format!("agent {id}"),
            rush,
            pid: 4242,
            ..Agent::default()
        };
        a.job.id = id.into();
        a
    }

    /// The view of session s1 alone, read with another agent beside it.
    fn hosted() -> Model {
        let mut m = Model::new("t");
        m.w = 120;
        m.h = 40;
        set(&mut m, "s1");
        let agents = vec![
            row("claude:x:s0", "s0", false),
            row("claude:x:s1", "s1", true),
        ];
        m.set_fleet(Fleet {
            roots: vec![String::new(); agents.len()],
            agents,
            ..Fleet::default()
        });
        m
    }

    fn open(m: &Model) -> Option<&str> {
        m.session.as_ref().map(|v| v.key.as_str())
    }

    /// Only its session is listed, opened at the whole width as soon as
    /// the list has it; every agent is still known (hostedSnap, pinHosted,
    /// allAgents). A hosted view doesn't lead (leads).
    #[test]
    fn it_shows_its_one_session() {
        let m = hosted();
        assert_eq!(m.agents.len(), 1);
        assert_eq!(all_agents(&m).len(), 2);
        assert_eq!(open(&m), Some("claude:x:s1"));
        assert!(m.session.as_ref().unwrap().o.wide);
        assert_eq!(m.sel, "claude:x:s1");
        assert!(alone(&m) && !m.leads());
        assert_eq!(id(&m), Some("s1"));
    }

    /// Its frame: rush's header counting every agent, no pages row,
    /// nothing that says a list is behind it; the keys' row says
    /// where the list is (TestHostedShowsOneSession, rushmode.go:3766).
    #[test]
    fn its_frame() {
        let mut m = hosted();
        for w in [80, 120, 200] {
            let out = crate::frame(&mut m, w, 40, &[], false);
            for not in ["another session", "esc back to the list", "Agents · Wall"] {
                assert!(!out.contains(not), "{w}: shows {not:?}:\n{out}");
            }
            for want in ["2 finished", "ctrl+\\ places", "agent s1"] {
                assert!(out.contains(want), "{w}: lacks {want:?}:\n{out}");
            }
        }
        let out = crate::frame(&mut m, 120, 40, &[], false);
        assert!(
            out.contains("f1 more  ·  ctrl+6 Agents  ·  esc leave the box"),
            "{out}"
        );
    }

    /// Nothing leaves the session: esc, ← with nothing typed, ctrl+n, the
    /// command bar, , and . between places; ctrl+\ still goes to the next
    /// place (hostedKeyGuard, leavePane).
    #[test]
    fn keys_never_leave_it() {
        let mut m = hosted();
        for k in [
            "esc", "esc", "left", "ctrl+n", "ctrl+z", "{", "tab", "ctrl+k", ",",
        ] {
            m.press(k);
            assert_eq!(open(&m), Some("claude:x:s1"), "after {k}");
            assert_eq!(m.place, PlaceId::Agents, "after {k}");
        }
        assert!(!crate::screens::cmdbar::bar::is_open(&m), "no command bar");
        m.press("ctrl+\\");
        assert_ne!(m.place, PlaceId::Agents);
    }

    /// esc takes the keys off the box; esc again there does nothing with
    /// no turn to stop; enter gives the box the keys back, and esc then,
    /// within a second of the first, asks whether to close the session
    /// (hostedAwayKey, paneKey's esc twice).
    #[test]
    fn esc_takes_the_keys_off_the_box() {
        let mut m = hosted();
        m.clock = Some(1_790_000_000_000);
        m.press("esc");
        assert!(away(&m));
        m.press("esc");
        assert!(away(&m) && m.confirm.is_none(), "nothing to stop");
        m.press("enter");
        assert!(!away(&m));
        m.press("esc");
        let c = m.confirm.as_ref().expect("the close question");
        assert!(c.question.contains("agent s1"), "{}", c.question);
        // Any other key goes back to the box, and on to it.
        let mut m = hosted();
        m.press("esc");
        m.press("x");
        assert!(!away(&m));
        let key = crate::screens::session_core::open_key(&m).unwrap();
        assert_eq!(
            crate::screens::session_core::conn(&mut m, &key)
                .input
                .string(),
            "x"
        );
        // ctrl+6 does too.
        m.press("esc");
        m.press("ctrl+6");
        assert!(!away(&m));
    }

    /// Off the box, its keys' row says how to come back and how to quit,
    /// and the box how to talk to the agent (paneDock, rushmode.go:3848).
    #[test]
    fn off_the_box_says_how_to_quit() {
        let mut m = hosted();
        m.press("esc");
        for w in [80, 120, 200] {
            let out = crate::frame(&mut m, w, 40, &[], false);
            assert!(out.contains("ctrl+q quit"), "{w}:\n{out}");
            assert!(out.contains("enter · → type here"), "{w}:\n{out}");
            assert!(
                out.contains("enter or → to talk to this agent"),
                "{w}:\n{out}"
            );
        }
    }

    /// Hosted's list has no command bar in its keys, and says how to hide
    /// it (view.go:2842).
    #[test]
    fn its_list_hint() {
        let mut m = hosted();
        m.press("ctrl+6");
        m.flash = None; // what ctrl+6 said has gone by
        let out = crate::frame(&mut m, 200, 40, &[], false);
        assert!(out.contains("esc · ctrl+6 hide Agents"), "{out}");
        assert!(!out.contains("go anywhere"), "{out}");
    }

    /// No page tabs on Agents or Projects, and the Projects tab neither
    /// lights up nor opens under the pointer (pageTabs, headerTabAt).
    #[test]
    fn no_page_tabs_and_no_projects_tab() {
        let mut m = hosted();
        assert!(m.page_tabs().is_none());
        m.set_place(PlaceId::Projects);
        assert!(m.page_tabs().is_none());
        m.set_place(PlaceId::Agents);
        let _ = crate::frame(&mut m, 120, 40, &[], false);
        let row = m.header_text[0].clone();
        let x = row
            .find(" Projects ")
            .map(|j| rush_core::text::width(&row[..j]) + 2);
        let x = x.expect("the Projects tab");
        assert_eq!(m.header_tab_at(x, 2), None);
        let x = row
            .find(" Settings ")
            .map(|j| rush_core::text::width(&row[..j]) + 2);
        assert!(
            m.header_tab_at(x.unwrap(), 2).is_some(),
            "Settings still opens"
        );
    }

    /// Hosted leaves out what's about the app as a whole; a plain view
    /// doesn't (model.go:556).
    #[test]
    fn hosted_is_quiet() {
        assert!(quiet(&hosted()));
        assert!(!quiet(&Model::new("t")));
    }

    /// ctrl+6 shows every agent beside it, and ctrl+6 or esc from the list
    /// hides them again, back on its session (toggleList).
    #[test]
    fn ctrl_6_shows_and_hides_the_list() {
        let mut m = hosted();
        m.press("ctrl+6");
        assert!(!alone(&m));
        assert_eq!(m.agents.len(), 2);
        assert_eq!(open(&m), None, "the list, its Session beside it");
        assert_eq!(
            m.said.last().unwrap().text,
            "Agents beside the session · ctrl+6 or esc hides them"
        );
        m.press("esc");
        assert!(alone(&m));
        assert_eq!(m.agents.len(), 1);
        assert_eq!(open(&m), Some("claude:x:s1"));
        m.press("ctrl+^");
        assert!(!alone(&m));
        m.press("ctrl+6");
        assert!(alone(&m));
    }

    /// Not a hosted view: none of it applies.
    #[test]
    fn a_plain_view_is_untouched() {
        let mut m = Model::new("t");
        let agents = vec![
            row("claude:x:s0", "s0", false),
            row("claude:x:s1", "s1", true),
        ];
        m.set_fleet(Fleet {
            roots: vec![String::new(); agents.len()],
            agents,
            ..Fleet::default()
        });
        assert_eq!(m.agents.len(), 2);
        assert!(open(&m).is_none() && !alone(&m) && id(&m).is_none());
        assert_eq!(key(&mut m, &Press::of("x").unwrap()), Out::Pass);
    }

    /// A plain view, wide, s1 picked.
    fn plain(w: usize, list_only: bool) -> Model {
        let mut m = Model::new("t");
        m.w = w;
        m.h = 40;
        m.cfg.list_only = list_only;
        let agents = vec![
            row("claude:x:s0", "s0", false),
            row("claude:x:s1", "s1", true),
        ];
        m.set_fleet(Fleet {
            roots: vec![String::new(); agents.len()],
            agents,
            ..Fleet::default()
        });
        m.select("claude:x:s1".into());
        m
    }

    fn pane_w(m: &Model) -> usize {
        crate::screens::list_extras::split::widths(m).1
    }

    /// Outside hosted, ctrl+6 moves between the split and the Session
    /// alone, and keeps the layout #view keeps (toggleList).
    #[test]
    fn ctrl_6_outside_hosted() {
        // Side by side is the layout: the Session takes the screen, and
        // comes back beside the list.
        let mut m = plain(200, false);
        assert!(pane_w(&m) > 0);
        m.press("ctrl+6");
        assert_eq!(open(&m), Some("claude:x:s1"));
        m.press("ctrl+6");
        assert_eq!(open(&m), None);
        assert!(pane_w(&m) > 0, "beside the list again");
        assert!(!m.cfg.list_only, "the layout kept");

        // The list alone is the layout: ctrl+6 brings the Session beside
        // it for now, then gives it the screen, then the split again; esc
        // from the Session goes back to the list alone.
        let mut m = plain(200, true);
        assert_eq!(pane_w(&m), 0);
        m.press("ctrl+6");
        assert!(pane_w(&m) > 0, "the Session beside the list");
        m.press("ctrl+6");
        assert_eq!(open(&m), Some("claude:x:s1"));
        m.press("ctrl+6");
        assert!(open(&m).is_none() && pane_w(&m) > 0);
        m.press("ctrl+6");
        m.press("esc");
        m.press("esc");
        assert!(open(&m).is_none());
        assert_eq!(pane_w(&m), 0, "your layout again");
        assert!(m.cfg.list_only);

        // The Session alone is the layout: from the list alone, the list
        // keeps the screen; from the Session, the list takes it.
        let mut m = plain(200, true);
        m.cfg.chat_full = true;
        m.press("ctrl+6");
        assert!(open(&m).is_none());
        assert_eq!(pane_w(&m), 0);
        m.press("enter");
        assert_eq!(open(&m), Some("claude:x:s1"));
        m.press("ctrl+6");
        assert!(open(&m).is_none());
        assert_eq!(pane_w(&m), 0);

        // Too narrow for both: the list keeps the screen.
        let mut m = plain(100, true);
        m.press("ctrl+6");
        assert!(open(&m).is_none());
        assert_eq!(pane_w(&m), 0);
        // And the Session alone goes back to the list.
        m.press("enter");
        m.press("ctrl+6");
        assert!(open(&m).is_none());
    }
}
