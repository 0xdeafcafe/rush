//! The Harnesses and Settings places: every settings page, the form they
//! share, and the sheets opened from them. After Go's
//! internal/ui/settings.go (page :25, settingsPages :50, dialog :71,
//! openDialog :131, dialogKey :175, dialogEdit :223, dialogBody :240) and
//! model.go:1828-1896 (setView's dialog, placePages, setSettingsPage,
//! stepSettingsPage).
//!
//! The two places share one dialog, as in Go: Harnesses shows its first
//! three pages (Providers, Harnesses, Config), Settings the rest. What a
//! page's rows change is rush's config.json, through rush-state's Store,
//! saved after each change as Go saves it.

pub mod acct;
pub(crate) mod catalog;
pub(crate) mod common;
pub(crate) mod configpage;
pub(crate) mod files;
pub(crate) mod form;
pub(crate) mod general;
pub(crate) mod harnesses;
pub(crate) mod interface;
pub(crate) mod keyboard;
pub(crate) mod keys;
#[allow(dead_code)]
pub(crate) mod paint;
pub mod plugins;
pub mod providers;
pub mod remote;
pub(crate) mod sample;
pub mod sheets;
pub mod statusline;
pub(crate) mod updates;
pub(crate) mod workspace;

#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::LazyLock;

use rush_core::text::Line;
use rush_state::{Config, Store};

use crate::frame::{Ext, Mouse, Out, Place, PlaceId, Press};
use crate::model::Model;
use crate::style::*;

pub(crate) use form::Section;
use workspace::Geometry;

/// The pages, in order (settings.go:39).
pub const PAGE_PROVIDERS: usize = 0;
pub const PAGE_HARNESSES: usize = 1;
pub const PAGE_CONFIG: usize = 2;
pub const PAGE_GENERAL: usize = 3;
pub const PAGE_KEYS: usize = 4;
pub const PAGE_PLUGINS: usize = 5;
pub const PAGE_UPDATES: usize = 6;
pub const PAGE_REMOTE: usize = 7;

/// A page's own drawing, when it isn't a form: w wide, h tall, its
/// geometry noted for the mouse.
pub(crate) type BodyFn = fn(&Model, &mut Geometry, usize, usize) -> Vec<Line>;
/// A page's own keys; Pass when it isn't one.
pub(crate) type PageKey = fn(&mut Model, &str) -> Out;

/// One page of Settings: a form, or one that draws itself (page).
#[derive(Clone, Default)]
pub struct Page {
    pub name: &'static str,
    /// Its own keys, for a form's key line.
    pub keys: Vec<String>,
    /// Sees a key before the page moves or closes: a page with a page
    /// inside it takes esc back to itself.
    pub pre: Option<PageKey>,
    pub form: Option<fn(&Model) -> Vec<Section>>,
    /// Lines above a form's sections.
    pub head: Option<fn(&Model, usize) -> Vec<Line>>,
    pub body: Option<BodyFn>,
    pub key: Option<PageKey>,
    /// The lines the cursor goes through.
    pub rows: Option<fn(&Model) -> usize>,
    /// Settles what the page shows before it's drawn, after each key
    /// (what Go's body did as it drew).
    pub prep: Option<fn(&mut Model)>,
}

/// The pages (settingsPages).
pub(crate) fn pages() -> Vec<Page> {
    vec![
        providers::page(),
        harnesses::page(),
        configpage::page(),
        Page {
            name: "General",
            form: Some(|m| {
                let mut s = general::sections(m);
                s.extend(interface::sections(m));
                s
            }),
            ..Page::default()
        },
        Page {
            name: "Keys",
            body: Some(keys::body),
            key: Some(keys::key),
            rows: Some(keys::len),
            ..Page::default()
        },
        plugins::page(),
        updates::page(),
        remote::page(),
    ]
}

/// What a typed answer does with the text.
pub(crate) type Answer = Rc<dyn Fn(&mut Model, &str) -> Out>;

/// Settings while it's open: the page, the cursor, and a value being
/// typed (dialog).
#[derive(Default)]
pub(crate) struct Dialog {
    pub page: usize,
    pub cursor: usize,
    /// What the input line is for; empty when not typing.
    pub asking: String,
    pub input: Vec<char>,
    /// What's typed is shown as dots: a key.
    pub secret: bool,
    pub on_answer: Option<Answer>,
    /// Keys' context, practice and pointer.
    pub keys: keys::KeysDialog,
    /// The line of Providers' list shown beside it; inside is whether
    /// the cursor is in what it shows rather than in the list.
    pub pick: usize,
    pub inside: bool,
    /// The agent Harnesses shows.
    pub model_kind: String,
    /// Config's column in focus: 0 the harness, then its providers.
    pub config_col: usize,
    /// A cell's models, open on Config.
    pub cell_pick: Option<configpage::CellPick>,
    pub harness_inside: bool,
    pub harness_return: usize,
    /// How far a catalog's detail is scrolled; -1 keeps the cursor in
    /// view. Drawing settles it.
    pub catalog_scroll: Cell<isize>,
    /// A provider shows its agent's advanced sections.
    pub advanced: bool,
    /// The plugin Plugins has open; empty lists them.
    pub plugin: String,
    /// The agent showing's own settings file, as last read.
    pub settings: Option<files::Loaded>,
}

/// The settings group's state: the dialog while a settings place shows,
/// the page each place was last on, and rush's store.
#[derive(Default)]
pub(crate) struct State {
    pub dialog: Option<Dialog>,
    pub settings_page: usize,
    pub harness_page: usize,
    store: Option<Store>,
    /// The list as last drawn, for General's previews.
    pub preview: interface::ListPreview,
    /// The keymap Keys shows and changes.
    pub keys: keys::Keys,
    /// Each agent's models as read from its home, by kind.
    pub listed: HashMap<String, Vec<rush_core::adapter::Choice>>,
    /// What the agents' pages read off the UI: definitions, settings
    /// files, plugins.
    pub reads: files::Reads,
    /// Plugins as last read, and whether they're being read.
    pub plugins: Option<plugins::Read>,
    pub plugins_reading: bool,
    pub updates: updates::Updates,
    pub remote: remote::Snap,
}

thread_local! {
    /// The dialog read while none is open: empty.
    static NO_DIALOG: &'static Dialog = Box::leak(Box::default());
}
static NO_CONFIG: LazyLock<Config> = LazyLock::new(Config::default);

/// The dialog as it is; an empty one when none is open.
pub(crate) fn dialog(m: &Model) -> &Dialog {
    m.kept
        .peek::<State>()
        .and_then(|s| s.dialog.as_ref())
        .unwrap_or_else(|| NO_DIALOG.with(|d| *d))
}

/// The dialog, to change; opened on General if none is.
pub(crate) fn dialog_mut(m: &mut Model) -> &mut Dialog {
    m.kept::<State>().dialog.get_or_insert_with(Dialog::default)
}

/// rush's store, read the first time it's wanted.
pub(crate) fn store(m: &mut Model) -> &mut Store {
    m.kept::<State>().store.get_or_insert_with(Store::load)
}

/// rush's config as loaded; the defaults before it is.
pub(crate) fn config(m: &Model) -> &Config {
    m.kept
        .peek::<State>()
        .and_then(|s| s.store.as_ref())
        .map(|s| &s.config)
        .unwrap_or(&NO_CONFIG)
}

/// rush's config, to change: save it after with [`changed`].
pub(crate) fn config_mut(m: &mut Model) -> &mut Config {
    &mut store(m).config
}

/// Puts a store in place of the one on disk (tests).
pub fn use_store(m: &mut Model, s: Store) {
    m.kept::<State>().store = Some(s);
}

/// After a change: config.json saved, and the list drawn from it again
/// (formKey's changed: SaveConfig, rebuild).
pub(crate) fn changed(m: &mut Model) {
    if let Err(e) = store(m).save_config() {
        m.flash_err(&format!("couldn't save the settings: {e}"));
    }
    sync(m);
}

/// The list's own copy of the config, taken again from the store's.
pub(crate) fn sync(m: &mut Model) {
    let cfg = serde_json::to_value(config(m))
        .ok()
        .and_then(|v| serde_json::from_value(v).ok());
    if let Some(c) = cfg {
        m.cfg = c;
    }
    m.rebuild();
}

/// Settings' pages a place shows: Harnesses the agents' set-up, Settings
/// the rest (placePages).
pub(crate) fn place_pages(p: PlaceId) -> (usize, usize) {
    if p == PlaceId::Harnesses {
        (PAGE_PROVIDERS, PAGE_GENERAL)
    } else {
        (PAGE_GENERAL, PAGE_REMOTE + 1)
    }
}

/// Opens page p afresh (openDialog, loadDialog).
pub(crate) fn open_dialog(m: &mut Model, p: usize) {
    store(m);
    m.kept::<State>().dialog = Some(Dialog {
        page: p,
        ..Dialog::default()
    });
    if p == PAGE_KEYS {
        keys::load(m);
    }
    match p {
        PAGE_PLUGINS => plugins::read(m),
        PAGE_UPDATES => updates::recheck_after(m),
        PAGE_REMOTE => remote::want(m),
        _ => {}
    }
    if p <= PAGE_CONFIG {
        // An agent installed since shows at once.
        common::reg().recheck();
        common::read_settings_models(m);
    }
}

/// What a page shows, settled after a key: its own prep, and what the
/// agent it's about reads, read again.
pub(crate) fn prep(m: &mut Model) {
    if m.kept.peek::<State>().is_none_or(|s| s.dialog.is_none()) {
        return;
    }
    if let Some(f) = cur_page(m).prep {
        f(m);
    }
    let d = dialog(m);
    let k = match d.page {
        PAGE_HARNESSES if d.harness_inside => d.model_kind.clone(),
        PAGE_PROVIDERS if d.inside => providers::prov_kind(m, &providers::picked(m)),
        _ => String::new(),
    };
    files::want(m, &k);
}

/// Shows one of the pages, in the place it's on (setSettingsPage).
pub fn set_settings_page(m: &mut Model, p: usize) {
    let v = if p < PAGE_GENERAL {
        PlaceId::Harnesses
    } else {
        PlaceId::Settings
    };
    if m.place != v {
        m.set_place(v);
    }
    let st = m.kept::<State>();
    if v == PlaceId::Harnesses {
        st.harness_page = p;
    } else {
        st.settings_page = p;
    }
    open_dialog(m, p);
}

/// [ and ]: the next page of the place's, round (stepSettingsPage).
pub(crate) fn step_settings_page(m: &mut Model, d: isize) {
    let (lo, hi) = place_pages(m.place);
    let n = (hi - lo) as isize;
    let cur = dialog(m).page.clamp(lo, hi - 1) as isize - lo as isize;
    set_settings_page(m, lo + ((cur + d) % n + n) as usize % n as usize);
}

/// The page showing (curPage).
pub(crate) fn cur_page(m: &Model) -> Page {
    let mut ps = pages();
    let i = dialog(m).page.min(ps.len() - 1);
    ps.swap_remove(i)
}

/// How many lines the cursor goes through (dialogLen).
pub(crate) fn dialog_len(m: &Model) -> usize {
    let p = cur_page(m);
    match (p.form, p.rows) {
        (Some(f), _) => form::flat(&f(m)).len(),
        (None, Some(r)) => r(m),
        _ => 0,
    }
}

/// Opens the input line for a typed value; answer gets it, trimmed,
/// unless it's empty (ask).
pub(crate) fn ask(m: &mut Model, what: &str, prefill: &str, answer: Answer) {
    let d = dialog_mut(m);
    d.asking = what.to_string();
    d.input = prefill.chars().collect();
    d.on_answer = Some(answer);
    d.secret = false;
}

/// ask for a secret, shown as dots (askSecret).
#[allow(dead_code)]
pub(crate) fn ask_secret(m: &mut Model, what: &str, answer: Answer) {
    ask(m, what, "", answer);
    dialog_mut(m).secret = true;
}

/// One of the two places: Harnesses or Settings.
pub struct SettingsPlace {
    id: PlaceId,
    geo: Geometry,
    /// The dialog's page and whether it's asking, as of the last key or
    /// frame: the frame asks without the model.
    page: usize,
    asking: bool,
}

impl SettingsPlace {
    pub fn new(id: PlaceId) -> SettingsPlace {
        SettingsPlace {
            id,
            geo: Geometry::default(),
            page: 0,
            asking: false,
        }
    }

    /// After a key: what the page shows taken again where it's kept.
    fn after(&mut self, m: &mut Model) {
        if dialog(m).page == PAGE_GENERAL {
            interface::refresh_preview(m);
        }
        prep(m);
        self.note(m);
    }

    /// Notes the dialog's page and question for the frame.
    fn note(&mut self, m: &Model) {
        let d = dialog(m);
        let (lo, _) = place_pages(self.id);
        self.page = d.page.saturating_sub(lo);
        self.asking = !d.asking.is_empty();
    }
}

/// A key on a settings page (dialogKey).
pub(crate) fn dialog_key(m: &mut Model, k: &Press) -> Out {
    // Where there's no list and Session to go between, tab and shift+tab
    // turn the page, as ] and [ do (keys.go:199).
    let s = match k.s.as_str() {
        "tab" => "]",
        "shift+tab" => "[",
        s => s,
    }
    .to_string();
    if !dialog(m).asking.is_empty() {
        match s.as_str() {
            "esc" => {
                let d = dialog_mut(m);
                d.asking.clear();
                d.input.clear();
                d.on_answer = None;
            }
            "enter" => {
                let d = dialog_mut(m);
                let v: String = d.input.iter().collect::<String>().trim().to_string();
                let f = d.on_answer.take();
                d.asking.clear();
                d.input.clear();
                if let Some(f) = f
                    && !v.is_empty()
                {
                    return f(m, &v).or_idle();
                }
            }
            _ => dialog_edit(m, k),
        }
        return Out::Draw;
    }
    let p = cur_page(m);
    if let Some(pre) = p.pre {
        let o = pre(m, &s);
        if o.used() {
            return o;
        }
    }
    if workspace::navigation(m, &s) {
        return Out::Draw;
    }
    match s.as_str() {
        "esc" | "q" | "ctrl+g" | "ctrl+a" => {
            m.set_place(PlaceId::Agents);
            return Out::Draw;
        }
        "[" | "]" => {
            step_settings_page(m, if s == "[" { -1 } else { 1 });
            return Out::Draw;
        }
        "up" | "k" => {
            let n = dialog_len(m);
            let d = dialog_mut(m);
            d.cursor = crate::frame::picker::round_move(d.cursor, -1, n);
            return Out::Draw;
        }
        "down" | "j" => {
            let n = dialog_len(m);
            let d = dialog_mut(m);
            d.cursor = crate::frame::picker::round_move(d.cursor, 1, n);
            return Out::Draw;
        }
        _ => {}
    }
    match (p.form, p.key) {
        (Some(f), _) => {
            let secs = f(m);
            form::form_key(m, &secs, &s).or_idle()
        }
        (None, Some(key)) => key(m, &s).or_idle(),
        _ => Out::Idle,
    }
}

/// Typing into the input line (dialogEdit).
fn dialog_edit(m: &mut Model, k: &Press) {
    let d = dialog_mut(m);
    match k.s.as_str() {
        "backspace" => {
            d.input.pop();
        }
        "ctrl+u" | "super+backspace" => d.input.clear(),
        _ => {
            if let Some(c) = k.text() {
                d.input.push(c);
            }
        }
    }
}

/// The page at width w, h tall (dialogBody).
pub(crate) fn dialog_body(m: &Model, geo: &mut Geometry, w: usize, h: usize) -> Vec<Line> {
    let p = cur_page(m);
    *geo = Geometry::default();
    let mut out = workspace::heading(m, w);
    let rest = h.saturating_sub(out.len());
    if let Some(f) = p.form {
        out.extend(workspace::form(m, geo, &f(m), &p.keys, w, rest));
        return out;
    }
    if let Some(b) = p.body {
        out.extend(b(m, geo, w, rest));
    }
    out
}

impl Place for SettingsPlace {
    fn name(&self) -> &str {
        self.id.name()
    }

    fn pages(&self, _m: &Model) -> Vec<String> {
        let (lo, hi) = place_pages(self.id);
        pages()[lo..hi].iter().map(|p| p.name.to_string()).collect()
    }

    fn page(&self) -> usize {
        self.page
    }

    fn set_page(&mut self, m: &mut Model, p: usize) {
        let (lo, hi) = place_pages(self.id);
        set_settings_page(m, (lo + p).min(hi - 1));
        self.note(m);
    }

    fn pages_line(&self, m: &Model, w: usize) -> Option<Line> {
        Some(workspace::tabs(m, self.id, w))
    }

    fn enter(&mut self, m: &mut Model) {
        let p = {
            let st = m.kept::<State>();
            if self.id == PlaceId::Harnesses {
                st.harness_page
            } else {
                st.settings_page.max(PAGE_GENERAL)
            }
        };
        open_dialog(m, p);
        self.after(m);
    }

    fn leave(&mut self, m: &mut Model) {
        m.kept::<State>().dialog = None;
        self.asking = false;
    }

    fn key(&mut self, m: &mut Model, k: &Press) -> Out {
        if m.kept.peek::<State>().is_none_or(|s| s.dialog.is_none()) {
            self.enter(m);
        }
        let o = dialog_key(m, k);
        self.after(m);
        o
    }

    fn mouse(&mut self, m: &mut Model, e: &Mouse, x: usize, y: usize) -> Out {
        let o = workspace::mouse(m, &self.geo, e, x, y);
        if o.used() {
            self.after(m);
        }
        self.note(m);
        o
    }

    fn paste(&mut self, m: &mut Model, text: &str) -> Out {
        if dialog(m).asking.is_empty() {
            return Out::Idle;
        }
        let t = crate::style::one_line(text);
        dialog_mut(m).input.extend(t.chars());
        Out::Draw
    }

    fn body(&mut self, m: &Model, w: usize, h: usize) -> Vec<Line> {
        self.note(m);
        // Go draws the dialog m.w-6 wide; the frame gives w-2.
        dialog_body(m, &mut self.geo, w.saturating_sub(4), h)
    }

    fn hint(&self, m: &Model, _w: usize) -> Line {
        let d = dialog(m);
        if d.asking.is_empty() {
            return Line::new();
        }
        let typed: String = if d.secret {
            "•".repeat(d.input.len().min(24))
        } else {
            d.input.iter().collect()
        };
        line([
            sp(format!("{} ❯ ", d.asking), ORANGE),
            sp(typed, TEXT),
            sp("▏", ORANGE),
        ])
    }

    fn asking(&self) -> bool {
        self.asking
    }
}

/// The rows a page has under the header (settingsHeight).
pub(crate) fn settings_height(m: &Model) -> usize {
    m.h.saturating_sub(m.top_h() + 2).max(1)
}

/// What goes before the page's description: the plugin open on Plugins.
pub(crate) fn extra_title(m: &Model) -> Option<String> {
    let p = &dialog(m).plugin;
    (!p.is_empty()).then(|| p.clone())
}

/// The line under Providers' description: what's been spent.
pub(crate) fn totals_line(m: &Model, _w: usize) -> Option<Line> {
    (dialog(m).page == PAGE_PROVIDERS).then(|| acct::totals(m))
}

/// A click on a page laid out as a catalog (catalogMouse).
pub(crate) fn catalog_mouse(m: &mut Model, g: &Geometry, x: usize, y: usize) -> Out {
    catalog::mouse(m, g, x, y)
}

/// The wheel on a page laid out as a catalog (catalogWheel).
pub(crate) fn catalog_wheel(m: &mut Model, g: &Geometry, x: usize, y: usize, d: isize) -> Out {
    catalog::wheel(m, g, x, y, d)
}

/// Adds the Harnesses and Settings places.
pub fn register(x: &mut Ext) {
    x.place(
        PlaceId::Harnesses,
        Box::new(SettingsPlace::new(PlaceId::Harnesses)),
    );
    x.place(
        PlaceId::Settings,
        Box::new(SettingsPlace::new(PlaceId::Settings)),
    );
    x.command(
        "network",
        "whether the API answers, the network rush is on and how fast it moves, and what waits for it (#net)",
        |m, _| {
            m.open_sheet(Box::new(sheets::NetSheet::new()));
            Out::Draw
        },
    );
    x.command(
        "statusline",
        "build the top bar, the agent header and Claude Code's status line",
        |m, _| statusline::open_top_bar(m),
    );
    x.keys.push(keys::capture);
    x.overlays.push(keys::modal);
}
