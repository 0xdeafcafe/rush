//! Plugins on screen: their sheets, approvals, picks and views, and the
//! screens that sit beside them (the layout question, typing into a live
//! pane, a session's artifacts, the file editor).
//!
//! After Go's internal/ui: pluginui.go (hooks.rs, intercept.rs), pluginsheet.go
//! (sheet.rs), pluginpick.go (pick.rs), pluginpicks.go (picks.rs),
//! pluginoffer.go (offer.rs), pluginapproval.go (approval.rs),
//! sidebar.go (sidebar.rs), viewsheet.go (layout.rs), embed.go
//! (embed.rs), artifacts.go (artifacts.rs), docedit.go and docstyle.go
//! (doc/), agentviews.go and viewblocks*.go (views/).
//!
//! Plugins take part in the screen only through the hooks client
//! (rush_plugind::hooks), which never waits. It and everything that reads
//! or changes plugins on disk is in [`Plugins`], kept on the model; nothing
//! here touches the broker or the disk until [`start`] is called, so a
//! model made in a test or for `--render` never does.

use std::collections::BTreeMap;
use std::sync::Arc;

use rush_core::text::{Color, Line, Span};
use rush_plugin::{Bundle, Offer, Plugin, Sidebar};
use rush_plugind::hooks::Client;

use crate::frame::{Ext, Out, Sheet};
use crate::model::Model;
use crate::style::*;

pub mod approval;
pub mod artifacts;
pub mod doc;
pub mod embed;
pub mod hooks;
pub mod intercept;
pub mod layout;
pub mod offer;
pub mod pick;
pub mod picks;
pub mod sheet;
pub mod sidebar;
pub mod views;

#[cfg(test)]
mod tests;

/// The bundled plugin that is the stash (stash.go:97 stashPlugin).
pub const STASH_PLUGIN: &str = "drafts";

/// What plugins on screen read and change on disk: the real files in
/// rush, a fake in tests. Every call is made off the UI thread.
pub trait Disk: Send + Sync {
    /// Which bundled plugins are on (plugin.BundlesOn).
    fn bundles_on(&self) -> BTreeMap<String, bool> {
        rush_plugin::bundles_on()
    }
    /// Turns a bundled plugin on or off (plugin.SetBundled).
    fn set_bundled(&self, name: &str, on: bool) -> Result<(), String> {
        rush_plugin::set_bundled(name, on)
    }
    /// Turns development mode on or off (plugin.SetDev).
    fn set_dev(&self, name: &str, on: bool) -> Result<(), String> {
        rush_plugin::set_dev(name, on)
    }
    /// Has the broker read plugins again (hooks.Reload).
    fn reload(&self) -> Result<(), String> {
        rush_plugind::hooks::reload()
    }
    /// The bundled plugins (plugin.Bundles).
    fn bundles(&self) -> Vec<Bundle> {
        rush_plugin::bundles()
    }
    /// Plugins installed and never approved, or changed since.
    fn pending(&self) -> Vec<Plugin> {
        rush_plugin::pending()
    }
    /// Whether name was approved before.
    fn approved_before(&self, name: &str) -> bool {
        rush_plugin::approvals().contains_key(name)
    }
    fn approve(&self, p: &Plugin) -> Result<(), String> {
        rush_plugin::approve(p)
    }
    fn decline(&self, p: &Plugin) -> Result<(), String> {
        rush_plugin::decline(p)
    }
    /// The plugins the repositories in roots ship (plugin.InProjects).
    fn in_projects(&self, roots: &[String]) -> Vec<Offer> {
        rush_plugin::in_projects(roots)
    }
    fn install(&self, o: &Offer) -> Result<(), String> {
        rush_plugin::install(o)
    }
    fn skip(&self, o: &Offer) -> Result<(), String> {
        rush_plugin::skip(o)
    }
    /// The optional plugins asked about, and the onboarding steps done
    /// (state.Onboarding's Plugins and Steps).
    fn onboarding(&self) -> (Vec<String>, Vec<String>) {
        let s = rush_state::Store::load();
        let o = &s.config.onboarding;
        (o.plugins.clone(), o.steps.clone())
    }
    /// Records plugins as asked about (markAsked).
    fn mark_asked(&self, names: &[String]) {
        let names = names.to_vec();
        let _ = crate::config::keep(move |c| {
            for n in names {
                if !c.onboarding.plugins.contains(&n) {
                    c.onboarding.plugins.push(n);
                }
            }
        });
    }
    /// Keeps the layout picked (Config.SetView).
    fn set_view(&self, v: &str) {
        let v = v.to_string();
        let _ = crate::config::keep(move |c| c.set_view(&v));
    }
    /// The plugins' arrangements of the list (sidebarFiles.Load).
    fn sidebars(&self, files: &mut rush_plugin::Sidebars) -> Vec<Sidebar> {
        files.load()
    }
    /// The network is down, as netwatch last found it.
    fn offline(&self) -> bool {
        rush_net::netwatch::down()
    }
}

/// rush's own files.
pub struct RealDisk;
impl Disk for RealDisk {}

/// What plugins on screen keep between frames (Go's Model fields hooks,
/// hookSeen, bundledOn, offers, optPicks, sidebars …).
pub struct Plugins {
    /// The window's way to its plugins; None until started.
    pub client: Option<Arc<Client>>,
    pub disk: Arc<dyn Disk>,
    /// Plugins and the disk are looked at: set by [`start`].
    pub live: bool,
    pub seen: hooks::HookSeen,
    /// Which bundled plugins are on, as last read.
    pub bundled_on: BTreeMap<String, bool>,
    /// Plugins projects ship, waiting to be offered.
    pub offers: Vec<Offer>,
    pub offer_roots: String,
    pub offer_at: rush_core::time::Ms,
    /// Optional plugins found, waiting for the screen to be free.
    pub opt_picks: Option<Vec<picks::OptPick>>,
    pub opt_checked: bool,
    /// When [`start`] ran: optional plugins are looked for a few seconds
    /// after (0 until then).
    pub started: rush_core::time::Ms,
    /// Plugins found installed and not approved, waiting.
    pub pending: Vec<Plugin>,
    /// The plugins' arrangements of the list.
    pub sidebars: Vec<Sidebar>,
    pub sidebar_files: rush_plugin::Sidebars,
}

impl Default for Plugins {
    fn default() -> Plugins {
        Plugins {
            client: None,
            disk: Arc::new(RealDisk),
            live: false,
            seen: hooks::HookSeen::default(),
            bundled_on: BTreeMap::new(),
            offers: Vec::new(),
            offer_roots: String::new(),
            offer_at: 0,
            opt_picks: None,
            opt_checked: false,
            started: 0,
            pending: Vec::new(),
            sidebars: Vec::new(),
            sidebar_files: rush_plugin::Sidebars::default(),
        }
    }
}

impl Model {
    /// What plugins on screen keep.
    pub fn plugins(&mut self) -> &mut Plugins {
        self.kept::<Plugins>()
    }

    /// The hooks client, when plugins are connected.
    pub fn hooks(&self) -> Option<Arc<Client>> {
        self.kept.peek::<Plugins>()?.client.clone()
    }

    /// Whether nothing else has the screen: no sheet, no question, and the
    /// Agents list showing (Go's sheet, dialog, confirm and modeList
    /// checks before a sheet opens unasked).
    pub fn screen_free(&self) -> bool {
        self.sheet.is_none()
            && self.confirm.is_none()
            && self.session.is_none()
            && self.place == crate::frame::PlaceId::Agents
    }
}

/// Connects this window to its plugins (model.go's startHooks): the
/// binary calls it once the loop runs. Messages from the broker come in
/// on a lane of their own.
pub fn start(m: &mut Model, client: Arc<Client>) {
    let p = m.plugins();
    p.live = true;
    p.started = rush_core::time::now();
    p.client = Some(client.clone());
    client.start();
    if let Some(tx) = m.sender() {
        let tx = tx.lane(1);
        let c = client.clone();
        let _ = std::thread::Builder::new()
            .name("rush-plugin-msgs".into())
            .spawn(move || {
                while let Some(msg) = c.next() {
                    if !tx.send(crate::worker::Note::Apply(Box::new(
                        move |m: &mut Model| hooks::on_hooks(m, msg),
                    ))) {
                        return;
                    }
                }
            });
    }
    hooks::read_bundled(m);
    // Hosted asks no plugin approvals (model.go:556 Init).
    if !crate::hosted::quiet(m) {
        approval::check(m);
    }
    // Go looks for optional plugins on its fifth tick: one look a few
    // seconds on, not a timer, and only with a loop to wait in.
    if m.sender().is_some() {
        m.later(
            || std::thread::sleep(std::time::Duration::from_millis(picks::LOOK_AFTER as u64)),
            |m, ()| {
                picks::check(m);
                Out::Draw
            },
        );
    }
}

/// Adds this group's hooks: after each reading of the list plugins hear
/// what changed, and what waits on you is offered once the screen is free.
pub fn register(x: &mut Ext) {
    // Settings › Plugins asks the broker through this window's hooks
    // client (settings_plugins.go:192 m.hooks.State().Plugins, :456
    // m.hooks.SetSetting).
    crate::screens::settings::plugins::provide(crate::screens::settings::plugins::Broker {
        running: |m| {
            m.hooks()
                .map(|h| h.state().plugins.clone())
                .unwrap_or_default()
        },
        set_setting: set_plugin_setting,
    });
    x.on_fleet.push(on_fleet);
    // The list's view summaries, and the open session's views.
    x.on_fleet.push(views::beside::on_fleet);
    x.hash.push(hooks::hash_command);
    // Plugins with "input" hear what's in the box as it changes: looked
    // at as each frame is drawn, and nothing at all without plugins.
    x.overlays.push(|m, _| hooks::emit_input(m));
}

/// Changes one of a running plugin's settings, off the UI thread; a
/// failure is flashed (settings_plugins.go:455).
fn set_plugin_setting(m: &mut Model, name: &str, key: &str, value: &str) -> Out {
    let Some(h) = m.hooks() else {
        m.flash_err(&format!("{name}: rush isn't talking to its plugins yet"));
        return Out::Draw;
    };
    let (name, key, value) = (name.to_string(), key.to_string(), value.to_string());
    let n = name.clone();
    m.later(
        move || h.set_setting(&name, &key, &value),
        move |m, r| {
            if let Err(e) = r {
                m.flash_err(&format!("{n}: {e}"));
            }
            Out::Draw
        },
    );
    Out::Draw
}

fn on_fleet(m: &mut Model) {
    if !m.kept.peek::<Plugins>().is_some_and(|p| p.live) {
        return;
    }
    hooks::emit_hooks(m);
    offer::check(m);
    picks::check(m);
    let pending = std::mem::take(&mut m.plugins().pending);
    approval::open(m, pending);
}

/// A plugin's text in its tone (pluginui.go:587 toned).
pub fn toned(tone: &str, s: &str) -> Span {
    match tone {
        "dim" => sp(s, DIM),
        "good" => sp(s, GREEN),
        "warn" => sp(s, YELLOW),
        "bad" => sp(s, RED),
        "accent" => sp(s, BLUE),
        _ => plain(s),
    }
}

/// How many installs, short: 1.2k, 3.4M (pluginsheet.go:444 installs).
pub fn installs(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// A file's size in words (memory.go:603 fileSize).
pub fn file_size(n: i64) -> String {
    if n < 1 << 10 {
        format!("{n} B")
    } else if n < 1 << 20 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    }
}

/// A picked choice row: its name bright, what it does dim (the rows of
/// the approval, offer and layout sheets).
pub fn choice(what: &str, about: &str) -> Line {
    line([spb(what, TEXT), plain("  "), sp(about, DIM)])
}

/// A string's text in colour c, bold.
pub fn bright(c: Color, s: impl Into<String>) -> Span {
    spb(s, c)
}

/// The image paths a message carries as their names (editor.go:429
/// shortImages).
pub fn short_images(s: &str) -> String {
    static TAG: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\[image: ([^\]]+)\]").unwrap());
    TAG.replace_all(s, |c: &regex::Captures| {
        format!("▣ {}", rush_convo::render::image_label(&c[1]))
    })
    .into_owned()
}

/// Work a sheet hands off the UI, waiting to be handed back to it.
struct Back<S, T>(Option<T>, std::marker::PhantomData<fn(S)>);

impl<S, T> Default for Back<S, T> {
    fn default() -> Self {
        Back(None, std::marker::PhantomData)
    }
}

/// What a sheet does with its work's result: as Go's sheetDo's apply,
/// run on the UI thread with the sheet itself.
pub type Done<S, T> = fn(&mut S, &mut Model, T) -> Out;

/// Runs work off the UI thread, then done with the open S and the result
/// (sheet.go:24 sheetDo). A sheet closed meanwhile never hears of it.
/// With no loop (tests, --render) the work is done at once, while the
/// sheet is still out of the model running its key: [`settled`] hands
/// it the result there.
pub fn sheet_do<S: Sheet, T: Send + 'static>(
    m: &mut Model,
    work: impl FnOnce() -> T + Send + 'static,
    done: Done<S, T>,
) {
    m.later(work, move |m, v| {
        if !m
            .sheet
            .as_ref()
            .is_some_and(|b| (b.as_ref() as &dyn std::any::Any).is::<S>())
        {
            if m.sheet.is_none() && m.sender().is_none() {
                m.kept::<Back<S, T>>().0 = Some(v); // done at once: see settled
            }
            return Out::Idle;
        }
        with_sheet(m, |s: &mut S, m| done(s, m, v))
    });
}

/// The result of work [`sheet_do`] finished at once, handed to s.
pub fn settled<S: Sheet, T: 'static>(s: &mut S, m: &mut Model, done: Done<S, T>) -> Option<Out> {
    let v = m.kept::<Back<S, T>>().0.take()?;
    Some(done(s, m, v))
}

/// Runs f with the open sheet when it's an S, taken out of the model as
/// the frame takes it out for a key: put back after unless f closed it or
/// opened another.
pub fn with_sheet<S: Sheet>(m: &mut Model, f: impl FnOnce(&mut S, &mut Model) -> Out) -> Out {
    let Some(mut b) = m.sheet.take() else {
        return Out::Idle;
    };
    let any: &mut dyn std::any::Any = b.as_mut();
    let Some(s) = any.downcast_mut::<S>() else {
        m.sheet = Some(b);
        return Out::Idle;
    };
    let out = f(s, m);
    if out != Out::Close && m.sheet.is_none() {
        m.sheet = Some(b);
    }
    out
}
