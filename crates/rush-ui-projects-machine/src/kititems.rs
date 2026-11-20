//! What the clean-up kits find, as rows: every plugin that provides
//! cleanup.report is asked for its items (go-, rust-, js-, git-clean:
//! rush-bundled's clean/mod.rs `items`), and one is cleaned by plugin and
//! id through cleanup.clean (rush_plugin::clean_item), each off the UI
//! thread, into Data's kits. Rust-only (the Projects redesign's third
//! step, docs/projects-redesign-task-list.md): Go's Projects had no kits'
//! rows. Projects puts them under This Mac, and those in a worktree
//! (target/, node_modules) under it; drawing them is the tree's.

use std::collections::HashMap;

use rush_core::time::{Ms, now};
use rush_plugin::{
    Answer, CleanupDone, CleanupItem, CleanupReport, Ctx, SERVICE_CLEANUP_REPORT, SERVICE_TIMEOUT,
};

use super::data::*;
use crate::frame::{Out, PlaceId};
use crate::model::Model;

/// The kits are asked again after this while Projects shows.
pub const KITS_EVERY: Ms = 2 * 60_000;
/// How soon after a clean they're asked again: a kit measures again five
/// seconds after one, and measuring takes a while.
pub const KITS_AFTER_CLEAN: Ms = 30_000;

/// One plugin's cleanup.report items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KitItems {
    /// The plugin, which cleanup.clean is asked of: "rust-clean".
    pub plugin: String,
    /// Its report's title; the plugin's name when it gave none.
    pub title: String,
    pub items: Vec<CleanupItem>,
}

/// An item, by plugin and id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct KitRef {
    pub plugin: String,
    pub id: String,
}

impl KitRef {
    pub fn new(plugin: impl Into<String>, id: impl Into<String>) -> KitRef {
        KitRef {
            plugin: plugin.into(),
            id: id.into(),
        }
    }
}

/// What the kits found and what cleaning them did, as Data keeps it.
#[derive(Clone, Debug, Default)]
pub struct Kits {
    /// Each plugin that answered with items, by name.
    pub found: Vec<KitItems>,
    /// When they were last asked (0: never), and whether an ask is out.
    pub asked: Ms,
    pub asking: bool,
    /// Items being cleaned now.
    pub cleaning: std::collections::HashSet<KitRef>,
    /// What each clean asked for did: what it said and freed, or why not.
    pub cleaned: HashMap<KitRef, Result<CleanupDone, String>>,
}

impl Kits {
    /// Every item, with the plugin it's from.
    pub fn all(&self) -> impl Iterator<Item = (&str, &CleanupItem)> {
        self.found
            .iter()
            .flat_map(|k| k.items.iter().map(move |i| (k.plugin.as_str(), i)))
    }

    /// The items in the worktree at path (target/, node_modules): those
    /// Projects shows under it.
    pub fn in_worktree<'a>(
        &'a self,
        path: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a CleanupItem)> + 'a {
        self.all().filter(move |(_, i)| i.worktree == path)
    }

    /// The items in no worktree: those Projects shows under This Mac.
    pub fn machine(&self) -> impl Iterator<Item = (&str, &CleanupItem)> {
        self.all().filter(|(_, i)| i.worktree.is_empty())
    }
}

/// The plugins' answers to cleanup.report as items, by plugin: one that
/// failed, said nothing readable or listed no items is left out.
pub fn kit_items_of(answers: Vec<Answer>) -> Vec<KitItems> {
    let mut out: Vec<KitItems> = answers
        .into_iter()
        .filter(|a| a.error.is_empty())
        .filter_map(|a| {
            let r: CleanupReport = serde_json::from_str(a.result.as_ref()?.get()).ok()?;
            if r.items.is_empty() {
                return None;
            }
            Some(KitItems {
                title: if r.title.is_empty() {
                    a.plugin.clone()
                } else {
                    r.title
                },
                plugin: a.plugin,
                items: r.items,
            })
        })
        .collect();
    out.sort_by(|a, b| a.plugin.cmp(&b.plugin));
    out
}

/// Asks every plugin that provides cleanup.report, through the broker.
/// Not for the UI thread.
pub fn ask_kit_answers() -> Vec<Answer> {
    let ctx = Ctx::background().with_timeout(SERVICE_TIMEOUT + std::time::Duration::from_secs(5));
    rush_plugin::ask_service(&ctx, SERVICE_CLEANUP_REPORT).unwrap_or_default()
}

fn d(m: &mut Model) -> &mut Data {
    m.kept::<Data>()
}

/// Asks the kits again when it's due (KITS_EVERY), while Projects shows
/// and there's a loop to bring the answer; never in tests (frozen).
pub fn refresh_kits(m: &mut Model) {
    if m.place != PlaceId::Projects || m.sender().is_none() || d(m).frozen {
        return;
    }
    let k = &data(m).kits;
    if k.asking || (k.asked != 0 && now() - k.asked < KITS_EVERY) {
        return;
    }
    ask_kits(m);
}

/// Asks every plugin providing cleanup.report for its items now.
pub fn ask_kits(m: &mut Model) {
    ask_kits_with(m, ask_kit_answers);
}

/// ask_kits with the answers ask brings, off the UI thread.
pub fn ask_kits_with(m: &mut Model, ask: impl FnOnce() -> Vec<Answer> + Send + 'static) {
    let k = &mut d(m).kits;
    if k.asking {
        return;
    }
    k.asking = true;
    m.later(
        move || kit_items_of(ask()),
        |m, found| {
            let k = &mut d(m).kits;
            k.asking = false;
            k.asked = now();
            if k.found == found {
                return Out::Idle;
            }
            k.found = found;
            Out::Draw
        },
    );
}

/// Has plugin clean its item id (cleanup.clean), off the UI thread: once
/// at a time each. What it did lands in kits.cleaned; one cleaned is
/// dropped from the rows, and the kits are asked again soon after.
pub fn clean_kit_item(m: &mut Model, plugin: &str, id: &str) {
    clean_kit_item_with(m, KitRef::new(plugin, id), |r| {
        rush_plugin::clean_item(&r.plugin, &r.id)
    });
}

/// clean_kit_item with clean doing the cleaning.
pub fn clean_kit_item_with(
    m: &mut Model,
    r: KitRef,
    clean: impl FnOnce(&KitRef) -> Result<CleanupDone, String> + Send + 'static,
) {
    let k = &mut d(m).kits;
    if !k.cleaning.insert(r.clone()) {
        return;
    }
    k.cleaned.remove(&r);
    m.later(
        move || {
            let got = clean(&r);
            (r, got)
        },
        |m, (r, got)| {
            let k = &mut d(m).kits;
            k.cleaning.remove(&r);
            if got.is_ok() {
                for f in &mut k.found {
                    if f.plugin == r.plugin {
                        f.items.retain(|i| i.id != r.id);
                    }
                }
                k.found.retain(|f| !f.items.is_empty());
                // Asked again once the kit has measured again.
                k.asked = now() - KITS_EVERY + KITS_AFTER_CLEAN;
            }
            k.cleaned.insert(r, got);
            Out::Draw
        },
    );
}
