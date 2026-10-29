//! ⌥w: which provider new sessions run, as a picker of the installed
//! providers' own profiles, the default starred, then Settings › Config.
//! After Go's internal/ui/profiles.go:202 (openProfilePicker) and
//! agentquota.go:166 (spillTo's flash).

use super::look::chain;
use super::rows::profile_words;
use super::state::{cfg, cfg_mut, ensure_store, save};
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
