//! Settings › Providers: a list and, beside it, everything about the line
//! picked: each installed provider, the ones set up first, Anthropic's
//! and OpenAI's subscriptions apart from their API keys. A provider shows
//! its accounts or key and its limits, and under it what rush can do
//! with it and its models. After Go's internal/ui/settings_providers.go
//! (providersPage :29, provItems :60, provSetUp :85, provPicked :97,
//! openItem :107, provKind :116, providersKey :235, listKeys :279,
//! providersBody :286, providerHead :358, canDo :435, providerForm :541
//! (its Ollama sections :550 are ollama.rs's),
//! configLink :558, harnessLink :579) and settings.go:63
//! (openAgentSettings).

use std::rc::Rc;

use rush_core::adapter::{self, Feature, Support};
use rush_core::text::Line;

use super::acct::{self, accounts, health_lines, use_of, window_lines};
use super::catalog;
use super::common::{
    agent_models, agent_order, feature_summary, frame_cursor, glyph, load_models, model_line,
    model_takes, models, pays_by_key, prov_label, prov_of, reg, start_on,
};
use super::form::{self, Section, Setting, flat, row_at};
use super::workspace::Geometry;
use super::{
    PAGE_CONFIG, PAGE_PROVIDERS, Page, config, config_mut, dialog, dialog_mut, open_dialog,
    set_settings_page,
};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// The page.
pub fn page() -> Page {
    Page {
        name: "Providers",
        pre: Some(|m, s| {
            let d = dialog_mut(m);
            if d.inside && matches!(s, "esc" | "q" | "backspace") {
                d.inside = false;
                d.cursor = d.pick;
                d.catalog_scroll.set(0);
                return Out::Draw;
            }
            let inside = d.inside;
            catalog::key(m, s, inside)
        }),
        body: Some(body),
        key: Some(key),
        rows: Some(|m| {
            if dialog(m).inside {
                flat(&provider_form(m, &picked(m))).len()
            } else {
                items(m).len()
            }
        }),
        ..Page::default()
    }
}

/// Providers' lines, by id: the providers set up, then the rest
/// (provItems).
pub fn items(m: &Model) -> Vec<String> {
    let (mut ready, mut rest): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let r = reg();
    for a in agent_order(m) {
        let pr = r.provider_of(a.kind().as_str());
        let mut ids = vec![pr.clone()];
        if r.split(&pr) {
            ids.push(adapter::key_of(&pr));
        }
        for id in ids {
            if ready.contains(&id) || rest.contains(&id) {
                continue;
            }
            if set_up(m, &id) {
                ready.push(id);
            } else {
                rest.push(id);
            }
        }
    }
    ready.extend(rest);
    ready
}

/// Whether provider id is ready to use: its harness runs here, and its
/// key is kept or it's signed in (provSetUp).
pub fn set_up(m: &Model, id: &str) -> bool {
    let k = prov_kind(m, id);
    if !reg().runs(&k) {
        return false;
    }
    if pays_by_key(id) {
        return config(m).has_api_key(&prov_of(id));
    }
    (accounts().why)(m, &k).is_empty()
}

/// The list's line shown: the cursor's, or the one gone into
/// (provPicked).
pub fn picked(m: &Model) -> String {
    let d = dialog(m);
    let items = items(m);
    let i = if d.inside { d.pick } else { d.cursor };
    items
        .get(i.min(items.len().saturating_sub(1)))
        .cloned()
        .unwrap_or_default()
}

/// Goes into provider id on Providers, when it's there (openItem).
pub fn open_item(m: &mut Model, id: &str) {
    if let Some(i) = items(m).iter().position(|x| x == id) {
        let d = dialog_mut(m);
        d.pick = i;
        d.cursor = 0;
        d.inside = true;
    }
}

/// Shows agent k's provider on Settings › Providers, open: its API key's
/// when only that runs it (openAgentSettings).
pub fn open_agent_settings(m: &mut Model, k: &str) {
    set_settings_page(m, PAGE_PROVIDERS);
    let (id, _) = reg().route_of(k, false);
    open_item(m, &id);
}

/// The agent provider id runs as: in its own harness, or the one chosen
/// as its default (provKind).
pub fn prov_kind(m: &Model, id: &str) -> String {
    match config(m).profile_named(id) {
        Some(p) if !p.providers.is_empty() => p.kind_of(&p.providers[0]),
        _ => id.to_string(),
    }
}

/// A key on the page (providersKey).
pub fn key(m: &mut Model, s: &str) -> Out {
    if dialog(m).inside {
        let secs = provider_form(m, &picked(m));
        if s == "left" || s == "h" {
            // Back out, as esc: going back never changes a setting; → and
            // space do.
            let d = dialog_mut(m);
            d.inside = false;
            d.cursor = d.pick;
            return Out::Draw;
        }
        return form::form_key(m, &secs, s);
    }
    let items = items(m);
    let it = picked(m);
    if s.len() == 1
        && let Some(n) = s.chars().next().and_then(|c| c.to_digit(10))
        && n >= 1
    {
        if n as usize <= items.len() {
            dialog_mut(m).cursor = n as usize - 1;
        }
        return Out::Draw;
    }
    match s {
        "enter" | "right" | "l" => {
            let d = dialog_mut(m);
            d.pick = d.cursor;
            d.cursor = 0;
            d.inside = true;
            // For its Models, off the UI.
            let k = prov_kind(m, &it);
            load_models(m, &k);
            Out::Draw
        }
        "*" | "space" => {
            config_mut(m).set_default_provider(&it);
            super::changed(m);
            (accounts().spill_to)(m);
            m.flash(&format!(
                "{} is the default: new sessions start on it",
                prov_label(&it)
            ));
            Out::Draw
        }
        "$" => {
            if pays_by_key(&it) {
                let (pr, _) = reg().billed(&it);
                return acct::ask_api_key(m, &pr);
            }
            Out::Idle
        }
        "a" => {
            if !pays_by_key(&it) {
                let k = prov_kind(m, &it);
                return (accounts().add)(m, &k);
            }
            Out::Idle
        }
        "r" => {
            m.flash("reading every provider's limits again…");
            (accounts().fetch)(m);
            Out::Draw
        }
        _ => Out::Idle,
    }
}

/// The keys of the list's line it (listKeys).
pub fn list_keys(it: &str) -> Vec<String> {
    let k: &[&str] = if pays_by_key(it) {
        &[
            "enter",
            "open",
            "1-9",
            "provider",
            "*",
            "make default",
            "$",
            "API key",
        ]
    } else {
        &[
            "enter",
            "open",
            "1-9",
            "provider",
            "*",
            "make default",
            "a",
            "add account",
            "r",
            "read limits",
        ]
    };
    k.iter().map(|s| s.to_string()).collect()
}

/// The page drawn (providersBody).
pub fn body(m: &Model, geo: &mut Geometry, w: usize, _h: usize) -> Vec<Line> {
    let d = dialog(m);
    let items = items(m);
    if items.is_empty() {
        return Vec::new();
    }
    let mut cat: Vec<Section> = Vec::new();
    for it in &items {
        let (pr, _) = reg().billed(it);
        let group = if set_up(m, it) {
            "Set up"
        } else {
            "Not set up"
        };
        let label = format!("{} {}", glyph(&pr).text, prov_label(it));
        if cat.last().is_none_or(|s| s.title != group) {
            cat.push(Section::new(group, Vec::new()));
        }
        let mut row = Setting {
            label,
            ..Setting::default()
        };
        row.tag = Line::new();
        cat.last_mut().expect("a section").rows.push(row);
    }
    let picked = if d.inside { d.pick } else { d.cursor }.min(items.len() - 1);
    let it = items[picked].clone();
    let inside = d.inside;
    let cursor = d.cursor;
    let controls = if inside {
        let mut k = row_at(&provider_form(m, &it), cursor).keys;
        k.extend(["←→".to_string(), "change".to_string()]);
        k
    } else {
        list_keys(&it)
    };
    let mut detail = |dw: usize| {
        let secs = provider_form(m, &it);
        let cur = if inside { cursor } else { usize::MAX };
        let mut rows = head(m, &it, dw);
        let head_len = rows.len();
        rows.extend(form::form_rows(&secs, cur, dw));
        let at = frame_cursor(&rows);
        let mut indices = catalog::line_indices(&secs, head_len, rows.len());
        if inside {
            rows.extend(form::about(&row_at(&secs, cur), dw));
        }
        rows.extend(can_do(m, &it, dw));
        indices.resize(rows.len(), None);
        (rows, at, indices)
    };
    catalog::body(
        m,
        geo,
        &cat,
        picked,
        inside,
        true,
        w,
        &mut detail,
        Some(controls),
    )
}

/// Provider id's name, whether it can run, the account or key it's paid
/// with, and its limits (providerHead).
pub fn head(m: &Model, id: &str, w: usize) -> Vec<Line> {
    let cfg = config(m);
    let r = reg();
    let (pr, key) = r.billed(id);
    let k = prov_kind(m, id);
    let u = use_of(m, &k);
    let label = |s: &str| line([sp(fit_str(s, 10), DIM)]);
    let mut name = line([glyph(&pr), plain(" "), spb(prov_label(id), TEXT)]);
    if id.eq_ignore_ascii_case(&cfg.default_profile().name) {
        name.spans.push(sp("  ★ the default", ORANGE));
    }
    let mut out = vec![name];
    let hint = r.hint(&k);
    if !hint.is_empty() && !r.runs(&k) {
        let mut l = label("can't run");
        l.spans.push(sp(hint, FAINT));
        out.push(l);
    }
    if pays_by_key(id) {
        let mut l = label("API key");
        if cfg.has_api_key(&pr) {
            l.spans.push(sp("✓ kept", GREEN));
            l.spans.push(sp(" · $ changes it", FAINT));
        } else {
            l.spans.push(sp("none yet", YELLOW));
            l.spans.push(sp(" · $ adds one", FAINT));
        }
        out.push(l);
    }
    out.extend(health_lines(m, &k, &label));
    if key {
        return out; // no account or limits: every token is paid for
    }
    let why = (accounts().why)(m, &k);
    if !why.is_empty() {
        let mut l = label("account");
        l.spans.push(sp("✗ not signed in", RED));
        l.spans.push(sp(format!(" · {why}"), DIM));
        out.push(l);
    } else if !super::common::switches(&k) && !u.in_use.q.email.is_empty() {
        let mut l = label("account");
        l.spans.push(sp(u.in_use.q.email.clone(), DIM));
        out.push(l);
    }
    let q = &u.in_use.q;
    if q.windows.is_empty() && q.balance.is_empty() && q.problem.is_empty() {
        let mut l = label("limits");
        join(&mut l, acct::limits(m, &u.in_use, w.saturating_sub(10), 0));
        out.push(l);
    }
    out.extend(window_lines(q, m.now, &label));
    out
}

/// What rush can do with provider id in its default harness, feature by
/// feature, and what the model it starts with takes (canDo).
pub fn can_do(m: &Model, id: &str, w: usize) -> Vec<Line> {
    let r = reg();
    let k = prov_kind(m, id);
    let mut model = start_on(m, id, &k).model;
    if let Some(p) = config(m).profile_named(id)
        && !p.model.is_empty()
    {
        model = p.model;
    }
    let (_, key) = r.billed(id);
    let mut out = vec![
        Line::new(),
        rule(
            "What it can do",
            &format!("in {} · {}", r.harness_label(&k), r.level_of(&k)),
            w,
        ),
    ];
    out.extend(feature_summary(
        |f| {
            if key && matches!(f, Feature::Switch | Feature::SignIn | Feature::Quota) {
                // Paid with a key: no accounts to switch, sign in to or read.
                return Support::NO;
            }
            r.feature_of(&k, f)
        },
        w,
    ));
    if !model.is_empty() {
        let mut l = line([plain("  "), sp(model.clone(), TEXT), plain("  ")]);
        join(&mut l, model_takes(&k, &model));
        out.push(l);
    }
    let ms = models(m, &k);
    if !ms.is_empty() {
        out.push(Line::new());
        out.push(rule(
            "Models",
            "context · price per million tokens in / out",
            w,
        ));
        for c in &ms {
            out.push(model_line(&k, c, w));
        }
        let seen = agent_models(m, &k).len() as isize - ms.len() as isize;
        if seen > 0 {
            out.push(line([
                plain("  "),
                sp(
                    format!("+ {seen} more your sessions have run, by their full names"),
                    FAINT,
                ),
            ]));
        }
    }
    out
}

/// Provider id's accounts, its sign-in, then where it runs and its
/// harness's own settings (providerForm).
pub fn provider_form(m: &Model, id: &str) -> Vec<Section> {
    let k = prov_kind(m, id);
    let mut secs = Vec::new();
    if !pays_by_key(id)
        && let Some(a) = acct::account_section(m, &k)
    {
        secs.push(a);
    }
    if let Some(auth) = super::harnesses::native_auth_section(m, &k) {
        secs.push(auth);
    }
    if reg().billed(id).0 == "ollama" {
        let l = super::ollama::local();
        secs.push(Section::new(
            "Ollama",
            super::ollama::model_settings(&l, &start_on(m, id, &k).model),
        ));
        secs.push(super::ollama::decision_section(m, &l));
    }
    secs.push(config_link(id));
    secs.push(harness_link(&k));
    secs
}

/// A row that opens Settings › Config on provider id's column: which
/// harnesses it runs in, and the models it calls there (configLink).
fn config_link(id: &str) -> Section {
    let label = "Where it runs, and its models";
    let id2 = id.to_string();
    Section {
        rows: vec![Setting {
            label: label.into(),
            line: Some(Rc::new(move |_| link(label, "Config"))),
            what: format!(
                "Which harnesses {} runs in, the one new sessions start in (★), and the models it may call in each: Settings › Config.",
                prov_label(id)
            ),
            keys: vec!["enter".into(), "open".into()],
            key: Some(Rc::new(move |m, s| {
                if s != "enter" && s != "right" {
                    return Out::Pass;
                }
                open_dialog(m, PAGE_CONFIG);
                let col = super::configpage::providers(m)
                    .iter()
                    .position(|p| *p == id2)
                    .unwrap_or(0);
                dialog_mut(m).config_col = col;
                Out::Draw
            })),
            ..Setting::default()
        }],
        ..Section::default()
    }
}

/// "→ label · on page".
pub fn link(label: &str, page: &str) -> Line {
    line([
        sp("→ ", DIM),
        sp(label.to_string(), TEXT),
        sp(format!(" · on {page}"), FAINT),
    ])
}

/// A row that opens k's harness on Harnesses: what the program itself
/// reads is there, once, not again on every provider it runs
/// (harnessLink).
fn harness_link(k: &str) -> Section {
    let hn = reg().harness_label(k);
    let label = format!("{hn}'s own settings");
    let (l2, k2) = (label.clone(), k.to_string());
    Section {
        rows: vec![Setting {
            label,
            line: Some(Rc::new(move |_| link(&l2, "Harnesses"))),
            what: format!(
                "Its settings file, environment, agent definitions and how rush runs it, on Harnesses ▸ {hn}."
            ),
            keys: vec!["enter".into(), "open".into()],
            key: Some(Rc::new(move |m, s| {
                if s != "enter" && s != "right" {
                    return Out::Pass;
                }
                super::harnesses::open_harness(m, &k2);
                Out::Draw
            })),
            ..Setting::default()
        }],
        ..Section::default()
    }
}
