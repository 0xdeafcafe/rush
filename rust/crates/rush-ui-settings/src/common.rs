//! What the agents' pages (Providers, Harnesses, Config) share: a
//! provider's glyph and names, how it's paid for, which models an agent
//! offers and what one takes, and what rush can do with it. After Go's
//! internal/ui/providers.go:19-112 (look, lookOf, glyph),
//! settings_providers.go:118-170 (provLabel, provName, billWord,
//! paysByKey), :443-520 (modelLine, featureSummary), settings_agent.go:73
//! (modelTakes, agentModels), settings_models.go (readSettingsModels,
//! applySettingsModels), startsheet.go:74-89, 131, 269-300, 334 (billingOf,
//! startID, startOn, loadModels, models, fits), accounts.go:110, 137, 236
//! (agentOrder, profileOf, switches), with.go:23-37 (modelWord,
//! agentName) and newcmd.go:389 (provOf).

use std::collections::HashMap;

use rush_core::adapter::{self, AdapterRef, Choice, Feature, Has, Kind, Media, Profile, Support};
use rush_core::text::{Color, Line};
use rush_core::usage::TokenUsage;

use super::paint::trunc;
use super::{State, config};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// The registry every agent is asked through.
pub fn reg() -> &'static adapter::Registry {
    adapter::global()
}

/// The kind whose accounts are rush's logins: Claude Code (loginsKind).
pub const LOGINS_KIND: &str = rush_state::LOGINS_KIND;

/// A provider's glyph and colour (look).
fn look(k: &str) -> (&'static str, Color) {
    const BUILTIN: (&str, Color) = ("✻", Color::Rgb(217, 119, 87));
    const OTHER: (&str, Color) = ("◇", Color::Rgb(143, 179, 217));
    let of = |k: &str| -> Option<(&'static str, Color)> {
        Some(match k {
            "codex" => ("◎", Color::Rgb(94, 199, 160)),
            "copilot" => ("◈", Color::Rgb(178, 150, 230)),
            "deepseek" => ("◆", Color::Rgb(116, 146, 255)),
            "glm" => ("▲", Color::Rgb(96, 196, 222)),
            "antigravity" | "gemini" => ("✦", Color::Rgb(138, 180, 248)),
            "kimi" => ("◐", Color::Rgb(200, 200, 214)),
            "vibe" => ("■", Color::Rgb(245, 165, 60)),
            "ollama" => ("◉", Color::Rgb(232, 232, 226)),
            "opencode" => ("▣", Color::Rgb(186, 182, 176)),
            "pi" => ("π", Color::Rgb(230, 190, 120)),
            _ => return None,
        })
    };
    if k == LOGINS_KIND {
        return BUILTIN;
    }
    if let Some(l) = of(k) {
        return l;
    }
    // A provider in another's harness looks like the provider.
    let p = reg().provider_of(k);
    if p == LOGINS_KIND {
        return BUILTIN;
    }
    of(&p).unwrap_or(OTHER)
}

/// Provider k's glyph in its colour (glyph).
pub fn glyph(k: &str) -> rush_core::text::Span {
    let (g, c) = look(k);
    sp(g, c)
}

/// What runs sessions of kind, as rush names it (agentName).
pub fn agent_name(k: &str) -> String {
    reg().label(k)
}

/// The harness sessions of kind run in (harnessName).
pub fn harness_name(k: &str) -> String {
    reg().harness_label(k)
}

/// Model id as agent kind names it (modelWord).
pub fn model_word(kind: &str, id: &str) -> String {
    let k = if kind.is_empty() { LOGINS_KIND } else { kind };
    reg().model_name(k, id)
}

/// The provider id is paid through (provOf).
pub fn prov_of(id: &str) -> String {
    reg().billed(id).0
}

/// Provider id as you'd call it: whose models, and how they're paid for
/// where that's worth saying (provLabel).
pub fn prov_label(id: &str) -> String {
    let b = bill_word(id);
    if b.is_empty() {
        prov_name(id)
    } else {
        format!("{} · {b}", prov_name(id))
    }
}

/// Whose models provider id serves (provName).
pub fn prov_name(id: &str) -> String {
    let r = reg();
    let (p, _) = r.billed(id);
    let name = r.provider_label(&p);
    let an = agent_name(&p);
    if !r.split(&p) && r.key_env(&p).is_empty() && name != an {
        if an.starts_with(&format!("{name} ")) {
            return an;
        }
        return format!("{name} {an}");
    }
    name
}

/// How provider id is paid for, where it's one of two ways or only by
/// key (billWord).
pub fn bill_word(id: &str) -> &'static str {
    let (p, _) = reg().billed(id);
    if pays_by_key(id) {
        "API key"
    } else if reg().split(&p) {
        "subscription"
    } else {
        ""
    }
}

/// Whether provider id is paid for with an API key alone (paysByKey).
pub fn pays_by_key(id: &str) -> bool {
    let r = reg();
    let (p, key) = r.billed(id);
    key || !r.split(&p) && !r.key_env(&p).is_empty()
}

/// Whether rush can switch agent k between accounts (switches).
pub fn switches(k: &str) -> bool {
    k == LOGINS_KIND || reg().get(k).is_some_and(|a| a.accounts().is_some())
}

/// The profile agent a's sessions run in (profileOf).
pub fn profile_of(m: &Model, a: &AdapterRef) -> Option<Profile> {
    if a.kind().as_str() == LOGINS_KIND {
        let acct = config(m).active_account();
        return Some(Profile {
            kind: Kind::new(LOGINS_KIND),
            name: acct.name,
            dir: acct.config_dir,
        });
    }
    reg().profiles_of(a).into_iter().next()
}

/// Every installed agent, in the default profile's order; the rest
/// follow, Claude Code first (agentOrder).
pub fn agent_order(m: &Model) -> Vec<AdapterRef> {
    let r = reg();
    let mut by: Vec<AdapterRef> = r
        .installed_all()
        .into_iter()
        .filter(|a| r.current_kind(a.kind().as_str()) == a.kind())
        .collect();
    let mut out = Vec::new();
    for k in config(m).default_profile().kinds() {
        if let Some(i) = by.iter().position(|a| a.kind().as_str() == k) {
            out.push(by.remove(i));
        }
    }
    by.sort_by_key(|a| a.kind().as_str() != LOGINS_KIND);
    out.extend(by);
    out
}

/// The billing a start on provider id in agent k goes by (billingOf).
pub fn billing_of(id: &str, k: &str) -> &'static str {
    let (_, key) = reg().billed(id);
    if key || reg().key_only(k) {
        rush_state::profiles::BILLING_KEY
    } else {
        ""
    }
}

/// Agent k on provider id as Settings says that route starts: its model
/// one Config lets it call there (startOn).
pub fn start_on(m: &Model, id: &str, k: &str) -> rush_state::Start {
    let r = reg();
    let mut id = id.to_string();
    if id.is_empty() {
        let p = r.provider_of(k);
        id = if billing_of("", k) == rush_state::profiles::BILLING_KEY && r.split(&p) {
            adapter::key_of(&p)
        } else {
            p
        };
    }
    let cfg = config(m);
    let mut st = cfg.dispatch.resolved(&id, k);
    let h = r.harness_of(k);
    if !cfg.calls(&id, h.as_str(), &st.model)
        && let Some(first) = cfg.cell_models_of(&id, h.as_str()).into_iter().next()
    {
        st.model = first;
    }
    st
}

/// Whether provider id runs in harness h here, and why not: the agent
/// that runs it, a warning, and why it can't (fits).
pub fn fits(m: &Model, id: &str, h: &str) -> (String, String, String) {
    let (k, warn, mut why) = reg().compat(id, h);
    if why.is_empty() && pays_by_key(id) && !config(m).has_api_key(&prov_of(id)) {
        why = "no API key yet".into();
    }
    (k.0, warn, why)
}

/// Reads agent k's models off the UI, for its pages (loadModels).
pub fn load_models(m: &mut Model, k: &str) -> Out {
    let Some(a) = reg().get(k) else {
        return Out::Idle;
    };
    if a.model_lister().is_none() {
        return Out::Idle;
    }
    let k = k.to_string();
    m.later(
        move || {
            let ps = reg().profiles_of(&a);
            match (a.model_lister(), ps.first()) {
                (Some(l), Some(p)) => l.list_models(p),
                _ => Vec::new(),
            }
        },
        move |m, v| {
            m.kept::<State>().listed.insert(k, v);
            Out::Draw
        },
    );
    Out::Idle
}

/// Every installed agent's models, read off the UI as the agents' pages
/// open (readSettingsModels, applySettingsModels).
pub fn read_settings_models(m: &mut Model) {
    m.later(
        || {
            let r = reg();
            let mut out: HashMap<String, Vec<Choice>> = HashMap::new();
            for a in r.installed_all() {
                if r.current_kind(a.kind().as_str()) != a.kind() {
                    continue;
                }
                if let Some(l) = a.model_lister()
                    && let Some(p) = r.profiles_of(&a).first()
                {
                    out.insert(a.kind().0, l.list_models(p));
                }
            }
            out
        },
        |m, found| {
            m.kept::<State>().listed.extend(found);
            Out::Draw
        },
    );
}

/// What agent k can start on: what was read from its home, then its
/// adapter's list (models).
pub fn models(m: &Model, k: &str) -> Vec<Choice> {
    let mut out: Vec<Choice> = m
        .kept
        .peek::<State>()
        .and_then(|s| s.listed.get(k).cloned())
        .unwrap_or_default();
    if let Some(ch) = reg().choices_of(k) {
        for c in ch.models {
            if !out.iter().any(|v| v.id == c.id) {
                out.push(c);
            }
        }
    }
    out
}

/// The models agent k offers, then any your sessions have run that it
/// didn't list (agentModels).
pub fn agent_models(m: &Model, k: &str) -> Vec<Choice> {
    let mut out = models(m, k);
    for a in &m.agents {
        let model = &a.spend.model;
        if !model.is_empty()
            && a.kind.as_str() == k
            && !model.starts_with('<')
            && !out.iter().any(|c| c.id == *model)
        {
            out.push(Choice {
                id: model.clone(),
                note: format!("one your {} sessions have run.", agent_name(k)),
                context: 0,
            });
        }
    }
    out
}

/// What model takes in agent k: images and PDFs, and its context window,
/// each only when known (modelTakes).
pub fn model_takes(k: &str, model: &str) -> Line {
    let mut out: Vec<Line> = Vec::new();
    if let Some(a) = reg().get(k)
        && let Some(r) = a.media_reader()
        && let Some(has) = r.reads(model)
    {
        for (bit, name) in [(Media::IMAGE, "images"), (Media::PDF, "PDFs")] {
            if has.has(bit) {
                out.push(line([sp("✓ ", GREEN), sp(name, DIM)]));
            } else {
                out.push(line([sp(format!("– {name}"), FAINT)]));
            }
        }
    }
    let n = windower(k, model);
    if n > 0 {
        out.push(line([sp(format!("{} context", tokens(n)), DIM)]));
    }
    if out.is_empty() {
        return line([sp(
            "what it takes is unknown to rush until it's used",
            FAINT,
        )]);
    }
    let mut l = Line::new();
    for (i, o) in out.into_iter().enumerate() {
        if i > 0 {
            l.spans.push(sp(" · ", FAINT));
        }
        join(&mut l, o);
    }
    l
}

/// model's context window as agent k's own ContextWindower says; 0 when
/// it has none.
fn windower(k: &str, model: &str) -> i64 {
    reg()
        .get(k)
        .and_then(|a| a.context_windower().map(|w| w.context_window(model)))
        .unwrap_or(0)
}

/// One model agent k offers: its name, context window and price, then
/// what it's for (modelLine).
pub fn model_line(k: &str, c: &Choice, w: usize) -> Line {
    let mut facts = Vec::new();
    let mut n = c.context;
    if n == 0 {
        n = windower(k, &c.id);
    }
    if n > 0 {
        facts.push(tokens(n));
    }
    let r = reg();
    let input = r.price(
        k,
        &c.id,
        &TokenUsage {
            input: 1_000_000,
            ..TokenUsage::default()
        },
    );
    let output = r.price(
        k,
        &c.id,
        &TokenUsage {
            output: 1_000_000,
            ..TokenUsage::default()
        },
    );
    match (input, output) {
        (Some(i), Some(o)) if i == 0.0 && o == 0.0 => facts.push("free".into()),
        (Some(i), Some(o)) => facts.push(format!("${} / ${}", go_g(i), go_g(o))),
        _ => {}
    }
    let mut l = line([
        plain("  "),
        sp(fit_str(&c.id, 24), TEXT),
        plain(" "),
        sp(fit_str(&facts.join(" · "), 22), DIM),
    ]);
    let room = w as isize - 50;
    if room > 12 && !c.note.is_empty() {
        l.spans.push(plain(" "));
        join(
            &mut l,
            trunc(line([sp(c.note.clone(), FAINT)]), room as usize),
        );
    }
    l
}

/// A number as Go's %g writes it: no trailing zeros.
pub fn go_g(f: f64) -> String {
    let s = format!("{f}");
    if s.contains('e') {
        return s;
    }
    s
}

/// What rush can do with an agent, said short: how many of its
/// features, then by name those it can't and those planned, and what's
/// said of the ones it can (featureSummary).
pub fn feature_summary(support: impl Fn(Feature) -> Support, w: usize) -> Vec<Line> {
    let all = adapter::all_features();
    let (mut no, mut planned, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    for f in &all {
        let st = support(f.feature);
        match st.is {
            Has::Yes => {
                if !st.note.is_empty() {
                    notes.push(format!("{}: {}", f.label, st.note));
                }
            }
            Has::Planned => planned.push(f.label),
            Has::No => no.push(f.label),
        }
    }
    let yes = all.len() - no.len() - planned.len();
    let head = if yes == all.len() {
        "everything rush does".to_string()
    } else {
        format!("{yes} of rush's {} features", all.len())
    };
    let mut out = vec![line([plain("  "), sp("✓ ", GREEN), sp(head, TEXT)])];
    let mut list = |mark: rush_core::text::Span, word: &str, names: &[&str]| {
        let text = format!("{word}{}", names.join(", "));
        for (i, l) in wrap(&text, 10.max(w as isize - 6) as usize)
            .into_iter()
            .enumerate()
        {
            if i == 0 {
                out.push(line([plain("  "), mark.clone(), sp(l, DIM)]));
            } else {
                out.push(line([plain("    "), sp(l, DIM)]));
            }
        }
    };
    if !no.is_empty() {
        list(sp("– ", FAINT), "can't: ", &no);
    }
    if !planned.is_empty() {
        list(sp("◌ ", YELLOW), "planned: ", &planned);
    }
    for n in notes {
        let mut l = line([plain("  ")]);
        join(
            &mut l,
            trunc(line([sp(n, FAINT)]), 10.max(w as isize - 2) as usize),
        );
        out.push(l);
    }
    out
}

/// A pairing's support for feature f: no accounts for a key-only agent,
/// no images for a model that doesn't read them (pairingSupport).
pub fn pairing_support(k: &str, model: &str, f: Feature) -> Support {
    let r = reg();
    let support = r.feature_of(k, f);
    if r.key_only(k) && matches!(f, Feature::Switch | Feature::SignIn | Feature::Quota) {
        return Support::NO;
    }
    if f == Feature::Images
        && !model.is_empty()
        && let Some(a) = r.get(k)
        && let Some(rd) = a.media_reader()
        && let Some(media) = rd.reads(model)
        && !media.has(Media::IMAGE)
    {
        return Support::NO;
    }
    support
}

/// The row on the selection's ground, if any (frameCursor).
pub fn frame_cursor(rows: &[Line]) -> usize {
    rows.iter()
        .position(|l| l.spans.iter().any(|s| s.style.bg == SEL_BG))
        .unwrap_or(0)
}
