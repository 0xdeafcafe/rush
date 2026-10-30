//! ⌥w: which provider new sessions run, as a picker of the installed
//! providers' own profiles, the default starred, then Settings › Config.
//! After Go's internal/ui/profiles.go:202 (openProfilePicker, and its
//! spillTo :222) and agentquota.go:163-195 (spillTo: new sessions move on
//! to the next provider while the first's accounts are all nearly out,
//! and back, said as news when rush does it on its own, in the rush that
//! leads, agentquota.go:95-113).

use rush_state::profiles::Profile;

use super::look::{agent_name, chain};
use super::rows::{profile_words, room};
use super::state::{accounts, cfg, cfg_mut, ensure_store, save};
use crate::frame::{Out, Pick, Picker, PlaceId};
use crate::model::Model;
use crate::style::*;

/// The value of the picker's last row.
const CONFIG: &str = "\u{0}config";

/// Opens the picker of what new sessions run (openProfilePicker).
pub fn open_profile_picker(m: &mut Model) -> Out {
    ensure_store(m);
    let c = cfg(m);
    let def = c.default_profile();
    let mut rows = Vec::new();
    let mut cursor = 0;
    for p in c.builtins() {
        let mut label = if p.name.eq_ignore_ascii_case(&def.name) {
            cursor = rows.len();
            line([sp("★ ", ORANGE)])
        } else {
            line([plain("  ")])
        };
        join(&mut label, chain(&m.reg, &p));
        rows.push(Pick {
            label,
            value: p.name.clone(),
            ..Pick::default()
        });
    }
    rows.push(Pick {
        label: line([sp("  Settings › Config…", FAINT)]),
        value: CONFIG.into(),
        ..Pick::default()
    });
    let mut pk = Picker::new(
        "New sessions run",
        rows,
        Box::new(|m, pick, _| {
            let Some(p) = pick else { return Out::Idle };
            if p.value == CONFIG {
                return open_config(m);
            }
            let name = p.value.clone();
            cfg_mut(m).set_default_profile(&name);
            save(m);
            let words = profile_words(m, &cfg(m).default_profile());
            m.flash(&format!("{name} is the default profile · {words}"));
            spill_to(m, true);
            Out::Draw
        }),
    );
    pk.cursor = cursor;
    pk.keys = vec![
        ("↑↓".into(), "choose".into()),
        ("enter".into(), "do it".into()),
        ("esc".into(), "close".into()),
    ];
    m.open_sheet(Box::new(pk));
    Out::Draw
}

/// Settings › Config: the settings place, on its page of that name.
fn open_config(m: &mut Model) -> Out {
    m.set_place(PlaceId::Settings);
    let page = m
        .with_place(PlaceId::Settings, |p, m| {
            p.pages(m).iter().position(|n| n == "Config")
        })
        .flatten();
    if let Some(i) = page {
        m.set_page(i);
    }
    Out::Draw
}

/// Says when new sessions move on from the default profile's first
/// provider, because its accounts are all nearly out and the profile
/// mixes, and when they come back; asked, as an answer, else as news.
/// Running sessions stay where they are (spillTo).
pub fn spill_to(m: &mut Model, asked: bool) {
    let p = cfg(m).default_profile();
    spill_to_for(m, &p, asked);
}

/// spill_to on its own, after a reading of the accounts' limits: only in
/// the rush that leads (agentquota.go:95 autoSwitch's following, :113).
pub fn auto_spill(m: &mut Model) {
    if m.leads() {
        spill_to(m, false);
    }
}

/// spill_to for profile p: Settings hands its own config's, just changed.
pub fn spill_to_for(m: &mut Model, p: &Profile, asked: bool) {
    let was = std::mem::take(&mut accounts(m).spill);
    let inst = p.installed();
    let Some(pick) = p.pick(&room(m)) else {
        return;
    };
    let def = inst.first().cloned().unwrap_or_default();
    let spill = if pick.kind != def {
        pick.kind.clone()
    } else {
        String::new()
    };
    accounts(m).spill = spill.clone();
    if spill == was {
        return;
    }
    let first = agent_name(&m.reg, &def);
    if !spill.is_empty() {
        let to = agent_name(&m.reg, &spill);
        if asked {
            m.flash(&format!(
                "{first}'s accounts are all nearly out · new sessions run {to} until they reset"
            ));
        } else {
            m.news(
                &format!(
                    "{first}'s accounts are all nearly out: new sessions run {to} until they reset"
                ),
                "",
            );
        }
    } else if asked {
        m.flash(&format!("new sessions run {first} again"));
    } else {
        m.news(
            &format!("new sessions run {first} again: its accounts have room"),
            "",
        );
    }
}
