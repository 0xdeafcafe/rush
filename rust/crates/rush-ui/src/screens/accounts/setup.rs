//! A setup is what a session runs as: whose models and how they're paid
//! for, the harness that runs them, the account, the model and the
//! effort. rush shows one as Provider (Harness) · account (OpenAI (Codex)
//! · alex), and #use takes it as <provider>-<harness>:<account>
//! (openai-codex:alex), an effort after it if you like. After Go's
//! internal/ui/setup.go (harnessWord :24, setupWord :34, setupName :44,
//! setupLabel :57, oldSetupName :70, nameWord :88, setups :101,
//! pickSetup :127, effortsOf :167, setupNote :229, startName :236,
//! startWords :246, setupChip :259, accountNames :268, accountName :277,
//! useSetup :485, setupCommand :512, afterWords :537), startsheet.go
//! (startOver :24, routes :43, billingOf :74, startID :83, startRoutes
//! :93, nextStart :105, byProfile :114, startDefaults :126, startOn :131,
//! profileSetup :146, newWords :552, models :234), newcmd.go (routeRow
//! :272, routeRows :286), providers.go:205 (startWith) and
//! newcmd.go:51 (startAs).

use rush_core::adapter::{Choice, Kind, Registry};
use rush_core::text::{Line, Span};
use rush_state::profiles::BILLING_KEY;

use super::look::{glyph, model_word, pays_by_key, prov_kind, prov_label, prov_of, runs_in_words};
use super::rows::{account_of, account_rows, accounts_of, start_dir, start_kind_in, start_profile};
use super::state::{accounts, cfg, peek};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// What a session starts as, over what its folder's profile and Settings
/// say (startOver).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartOver {
    pub kind: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub mode_set: bool,
    /// BILLING_KEY pays with the provider's API key; "" as it's signed in.
    pub billing: String,
    /// The account's name; "" is the one in use.
    pub account: String,
    /// The profile of yours it is; "" none.
    pub profile: String,
}

/// A provider id (claude, claude-key, ollama) and the agents here that
/// run it (route).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub id: String,
    pub kinds: Vec<String>,
}

/// The valid ways to start here, of kinds (the agents that run here): a
/// split provider's subscription in its own harness only, its API key in
/// any that speaks its API, and anything paid by key only once the key is
/// kept (routes).
pub fn routes(reg: &Registry, kinds: &[String], has_key: &dyn Fn(&str) -> bool) -> Vec<Route> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for k in kinds {
        let p = reg.provider_of(k);
        if seen.contains(&p) {
            continue;
        }
        seen.push(p.clone());
        let mut ids = vec![p.clone()];
        if reg.split(&p) {
            ids.push(rush_core::adapter::key_of(&p));
        }
        for id in ids {
            let (_, key) = reg.billed(&id);
            let ks: Vec<String> = reg
                .runs_for(&id)
                .into_iter()
                .filter(|r| {
                    kinds.contains(&r.0) && (!key && !reg.key_only(r.as_str()) || has_key(&p))
                })
                .map(|r| r.0)
                .collect();
            if !ks.is_empty() {
                out.push(Route { id, kinds: ks });
            }
        }
    }
    out
}

/// How agent k is paid for as provider id: by key when id is a key, or k
/// runs only on one (billingOf).
pub fn billing_of(reg: &Registry, id: &str, k: &str) -> String {
    if reg.billed(id).1 || reg.key_only(k) {
        BILLING_KEY.into()
    } else {
        String::new()
    }
}

/// The provider o is paid through: a split provider's key one when it
/// goes by the key (startID).
pub fn start_id(reg: &Registry, o: &StartOver) -> String {
    let p = reg.provider_of(&o.kind);
    if o.billing == BILLING_KEY && reg.split(&p) {
        return rush_core::adapter::key_of(&p);
    }
    p
}

/// The ways a session can start here now (startRoutes).
pub fn start_routes(m: &Model) -> Vec<Route> {
    let reg = &m.reg;
    let kinds: Vec<String> = reg
        .all()
        .iter()
        .filter(|a| {
            let k = a.kind();
            a.driver().is_some() && reg.current_kind(k.as_str()) == k && reg.runs(k.as_str())
        })
        .map(|a| a.kind().0)
        .collect();
    let c = cfg(m);
    routes(reg, &kinds, &|p| c.has_api_key(p))
}

/// Agent k on provider id as Settings says that route starts: id's key
/// pays when id is one; empty is k's own way. Its model is one Config lets
/// it call there (startOn).
pub fn start_on(m: &Model, id: &str, k: &str) -> StartOver {
    let reg = &m.reg;
    let billing = billing_of(reg, id, k);
    let id = if id.is_empty() {
        start_id(
            reg,
            &StartOver {
                kind: k.into(),
                billing: billing.clone(),
                ..StartOver::default()
            },
        )
    } else {
        id.to_string()
    };
    let c = cfg(m);
    let mut st = c.dispatch.resolved(&id, k);
    let h = reg.harness_of(k);
    if !c.calls(&id, h.as_str(), &st.model) {
        // Config keeps it to others.
        st.model = c
            .cell_models_of(&id, h.as_str())
            .into_iter()
            .next()
            .unwrap_or_default();
    }
    StartOver {
        kind: k.into(),
        model: st.model,
        effort: st.effort,
        mode: st.mode,
        mode_set: true,
        billing,
        ..StartOver::default()
    }
}

/// Agent k as Settings says it starts, paid as it is (startDefaults).
pub fn start_defaults(m: &Model, k: &str) -> StartOver {
    start_on(m, "", k)
}

/// Profile p as a start on agent k: its model, effort, billing and
/// account over what Settings starts k with (profileSetup).
pub fn profile_setup(m: &Model, p: &rush_state::Profile, k: &str) -> StartOver {
    let mut o = start_defaults(m, k);
    if !p.model.is_empty() {
        o.model = p.model.clone();
    }
    if !p.effort.is_empty() {
        o.effort = p.effort.clone();
    }
    if !p.billing.is_empty() {
        o.billing = p.billing.clone(); // with no key kept yet, starting says where to add one
    }
    o.account = account_name(m, k, &p.account);
    if !p.builtin {
        o.profile = p.name.clone();
    }
    o
}

/// What a new session in dir starts as by its profile, whatever was
/// picked for the next one (byProfile).
pub fn by_profile(m: &Model, dir: &str) -> StartOver {
    let k = start_kind_in(m, dir);
    let p = start_profile(m, dir);
    if p.providers
        .first()
        .is_some_and(|f| *f == m.reg.provider_of(&k))
    {
        let mut o = profile_setup(m, &p, &k);
        o.account.clear(); // the one in use: a profile's account is where it starts while that has room
        return o;
    }
    start_defaults(m, &k)
}

/// What the next session in dir starts as: one picked on the sheet, else
/// what its profile says (nextStart).
pub fn next_start(m: &Model, dir: &str) -> StartOver {
    if let Some(o) = &peek(m).start_over {
        return o.clone();
    }
    by_profile(m, dir)
}

/// What agent k can start on: what was read from its home, then its
/// adapter's list (models).
pub fn models(m: &Model, k: &str) -> Vec<Choice> {
    let mut out: Vec<Choice> = peek(m).listed.get(k).cloned().unwrap_or_default();
    if let Some(ch) = m.reg.choices_of(k) {
        for c in ch.models {
            if !out.iter().any(|v| v.id == c.id) {
                out.push(c);
            }
        }
    }
    out
}

/// The efforts agent kind starts with (effortsOf).
pub fn efforts_of(reg: &Registry, kind: &str) -> Vec<Choice> {
    reg.choices_of(kind).map(|c| c.efforts).unwrap_or_default()
}

// ---- Names ----

/// The harness agent k runs in, as a setup's name starts: claudecode,
/// codex, pi (harnessWord).
pub fn harness_word(reg: &Registry, k: &str) -> String {
    let h = reg.harness_of(k);
    if h.as_str() == super::look::LOGINS_KIND {
        return "claudecode".into(); // its program's name, not its models'
    }
    h.0
}

/// A name as one word, to type: "Alex Work" is alex-work (nameWord).
pub fn name_word(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
}

/// Agent k as one word to type: openai-codex, ollama-pi, opencode
/// (setupWord).
pub fn setup_word(reg: &Registry, k: &str) -> String {
    let p = name_word(&reg.provider_label(&reg.provider_of(k)));
    let h = harness_word(reg, k);
    if p == h {
        return p;
    }
    format!("{p}-{h}")
}

/// What #use calls a setup: <provider>-<harness>, then :<account>, or :key
/// for its provider's API key (setupName).
pub fn setup_name(reg: &Registry, k: &str, billing: &str, account: &str) -> String {
    let w = setup_word(reg, k);
    if billing == BILLING_KEY || reg.key_only(k) {
        return format!("{w}:key");
    }
    if !account.is_empty() && reg.harness_of(k).as_str() == k {
        return format!("{w}:{}", name_word(account));
    }
    w
}

/// A setup as it's shown: OpenAI (Codex) · alex, Anthropic (Pi) · API key
/// (setupLabel).
pub fn setup_label(reg: &Registry, k: &str, billing: &str, account: &str) -> String {
    if billing == BILLING_KEY || reg.key_only(k) {
        return format!("{} · API key", reg.label(k));
    }
    if !account.is_empty() && reg.harness_of(k).as_str() == k {
        return format!("{} · {account}", reg.label(k));
    }
    reg.label(k)
}

/// A setup's name before provider words: <harness>:<account>, pi:ollama.
/// #use still takes it (oldSetupName).
pub fn old_setup_name(reg: &Registry, k: &str, billing: &str, account: &str) -> String {
    let w = harness_word(reg, k);
    let mut p = reg.provider_of(k);
    if reg.harness_of(k).as_str() != k {
        if reg.key_only(k) && reg.split(&p) {
            p = rush_core::adapter::key_of(&p);
        }
        return format!("{w}:{p}");
    }
    if billing == BILLING_KEY {
        return format!("{w}:key");
    }
    if !account.is_empty() {
        return format!("{w}:{}", name_word(account));
    }
    w
}

/// One way a session can run here: its name, the other names #use takes
/// for it, and the start (setup).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    pub name: String,
    pub alias: Vec<String>,
    pub o: StartOver,
}

/// Every way a session can run here: each harness on the account in use,
/// on each of its accounts and on its provider's key, and each provider in
/// another's harness (setups).
pub fn setups(m: &Model) -> Vec<Setup> {
    let reg = &m.reg;
    let rows = account_rows(m);
    let mut out = Vec::new();
    for r in start_routes(m) {
        let (p, key) = reg.billed(&r.id);
        for k in &r.kinds {
            let mut o = start_defaults(m, k);
            o.billing = billing_of(reg, &r.id, k);
            if key || reg.harness_of(k).as_str() != k {
                out.push(Setup {
                    name: setup_name(reg, k, &o.billing, ""),
                    alias: vec![
                        r.id.clone(),
                        k.clone(),
                        old_setup_name(reg, k, &o.billing, ""),
                    ],
                    o,
                });
                continue;
            }
            out.push(Setup {
                name: setup_name(reg, k, "", ""),
                alias: vec![p.clone(), old_setup_name(reg, k, "", "")],
                o: o.clone(),
            });
            for a in accounts_of(&rows, k) {
                o.account = a.name(m);
                out.push(Setup {
                    name: setup_name(reg, k, "", &o.account),
                    alias: vec![
                        format!("{p}:{}", name_word(&o.account)),
                        old_setup_name(reg, k, "", &o.account),
                    ],
                    o: o.clone(),
                });
            }
        }
    }
    out
}

/// #use's argument as one of ss: a setup's name, else another name for
/// one, then an effort of efforts(kind) if you like (pickSetup).
pub fn pick_setup(
    ss: &[Setup],
    arg: &str,
    efforts: &dyn Fn(&str) -> Vec<Choice>,
) -> Result<StartOver, String> {
    let arg = arg.trim().to_lowercase();
    let find = |name: &str| -> Option<StartOver> {
        ss.iter()
            .find(|s| s.name == name)
            .or_else(|| ss.iter().find(|s| s.alias.iter().any(|a| a == name)))
            .map(|s| s.o.clone())
    };
    if let Some(o) = find(&arg) {
        return Ok(o);
    }
    let i = arg.rfind(':').unwrap_or(0);
    let found = if i > 0 { find(&arg[..i]) } else { None };
    let Some(mut o) = found else {
        return Err(format!(
            "nothing here called {arg} · #use <harness>:<account>[:<effort>]"
        ));
    };
    let (name, effort) = (&arg[..i], &arg[i + 1..]);
    let ids: Vec<String> = efforts(&o.kind).into_iter().map(|e| e.id).collect();
    if ids.is_empty() {
        return Err(format!("{name} takes no effort"));
    }
    if !ids.iter().any(|e| e == effort) {
        return Err(format!("{name} takes effort {}", ids.join(", ")));
    }
    o.effort = effort.to_string();
    Ok(o)
}

/// Whose models a setup runs, and how they're paid for: OpenAI ·
/// subscription, Ollama in Pi (setupNote).
pub fn setup_note(m: &Model, o: &StartOver) -> String {
    format!(
        "{}{}",
        prov_label(&m.reg, &start_id(&m.reg, o)),
        runs_in_words(&m.reg, &o.kind)
    )
}

/// What o is called: the profile of yours it starts under, else its
/// setup, on the account its provider is signed in as when it names none
/// (startName).
pub fn start_name(m: &Model, o: &StartOver) -> String {
    if !o.profile.is_empty() {
        return o.profile.clone();
    }
    let acct = if o.account.is_empty() {
        account_of(m, &o.kind)
    } else {
        o.account.clone()
    };
    setup_label(&m.reg, &o.kind, &o.billing, &acct)
}

/// o in words: its name, model and effort (startWords).
pub fn start_words(m: &Model, o: &StartOver) -> Vec<String> {
    let model = if o.model.is_empty() {
        "default model".to_string()
    } else {
        model_word(&m.reg, &o.kind, &o.model)
    };
    let mut words = vec![start_name(m, o), model];
    if !o.effort.is_empty() {
        words.push(format!("{} effort", o.effort));
    }
    words
}

/// The chip's ground (barChip).
pub const BAR_CHIP: rush_core::text::Color = rush_core::text::Color::Rgb(64, 45, 37);

/// o as a chip: its provider's glyph, its name, model and effort
/// (setupChip).
pub fn setup_chip(m: &Model, o: &StartOver) -> Line {
    let w = start_words(m, o);
    let on = |mut s: Span| {
        s.style.bg = BAR_CHIP;
        s
    };
    line([
        on(plain(" ")),
        on(glyph(&m.reg, &o.kind)),
        on(plain(" ")),
        on(spb(w[0].clone(), TEXT)),
        on(sp(format!(" · {}", w[1..].join(" · ")), DIM)),
        on(plain(" ")),
    ])
}

/// What a new session in dir starts as, as the box's chip says it; plain
/// is the words alone, for text that's matched (startWith).
pub fn start_with(m: &Model, dir: &str) -> Line {
    setup_chip(m, &next_start(m, dir))
}

/// start_with's words alone.
pub fn start_with_words(m: &Model, dir: &str) -> String {
    start_words(m, &next_start(m, dir)).join(" · ")
}

/// The accounts agent k can start on, by name (accountNames).
pub fn account_names(m: &Model, k: &str) -> Vec<String> {
    accounts_of(&account_rows(m), k)
        .iter()
        .map(|r| r.name(m))
        .collect()
}

/// The name of agent k's account id, or "" when rush keeps no such
/// account (accountName).
pub fn account_name(m: &Model, k: &str, id: &str) -> String {
    if id.is_empty() {
        return String::new();
    }
    accounts_of(&account_rows(m), k)
        .iter()
        .find(|r| r.id() == id)
        .map(|r| r.name(m))
        .unwrap_or_default()
}

/// o as #new takes it: codex@openai-sub:alex gpt-6-astra high (newWords).
pub fn new_words(reg: &Registry, o: &StartOver) -> String {
    let h = reg.harness_of(&o.kind);
    let mut out = format!(
        "#new {}@{}",
        reg.harness_word(h.as_str()),
        reg.provider_word(&start_id(reg, o))
    );
    if !o.account.is_empty() {
        out += &format!(":{}", name_word(&o.account));
    }
    for v in [&o.model, &o.effort] {
        if !v.is_empty() {
            out += &format!(" {v}");
        }
    }
    out
}

/// One provider in one harness, as #new and the sheets list it
/// (routeRow).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteRow {
    pub id: String,
    pub harness: Kind,
    /// The agent that runs it; empty when nothing does.
    pub kind: Kind,
    /// The provider's default harness.
    pub def: bool,
    pub warn: String,
    pub why: String,
}

/// Every route of every installed provider, those that can run first,
/// each provider's default before its others (routeRows).
pub fn route_rows(m: &Model) -> Vec<RouteRow> {
    let reg = &m.reg;
    let c = cfg(m);
    let mut out = Vec::new();
    for id in reg.provider_ids() {
        if !reg.provider_installed(&prov_of(reg, &id)) {
            continue;
        }
        let def = reg.harness_of(prov_kind(reg, c, &id).as_str());
        for h in reg.harness_kinds() {
            let (k, warn, mut why) = reg.compat(&id, h.as_str());
            if k.is_empty() {
                continue;
            }
            if why.is_empty() && pays_by_key(reg, &id) && !c.has_api_key(&prov_of(reg, &id)) {
                why = "no API key yet: $ in Settings › Providers".into();
            }
            out.push(RouteRow {
                id: id.clone(),
                def: h == def,
                harness: h,
                kind: k,
                warn,
                why,
            });
        }
    }
    out.sort_by_key(|r| (!r.why.is_empty(), !r.def));
    out
}

/// Whether provider id runs in harness h here, and why not (fits).
pub fn fits(m: &Model, id: &str, h: &str) -> (Kind, String, String) {
    let (k, warn, mut why) = m.reg.compat(id, h);
    if why.is_empty() && pays_by_key(&m.reg, id) && !cfg(m).has_api_key(&prov_of(&m.reg, id)) {
        why = "no API key yet".into();
    }
    (k, warn, why)
}

// ---- Starting ----

/// Starts a session on o with msg in the folder new ones start in
/// (startAs): through the screens that host sessions, which aren't in this
/// rush yet when none has said it starts them.
pub fn start_as(m: &mut Model, o: StartOver, msg: &str) -> Out {
    accounts(m).start_over = Some(o.clone());
    let dir = start_dir(m);
    start_in(m, &o, msg, &dir)
}

/// Starts a session on o with msg in dir.
pub fn start_in(m: &mut Model, o: &StartOver, msg: &str, dir: &str) -> Out {
    match accounts(m).starter {
        Some(f) => f(m, o, msg, dir),
        None => {
            m.flash_err("rush can't start sessions here yet");
            Out::Draw
        }
    }
}

/// #use with arg in the box: what the next session starts as, started at
/// once with msg when there is one; alone, the sheet (useSetup).
pub fn use_setup(m: &mut Model, arg: &str, msg: &str) -> Out {
    if arg.is_empty() {
        return super::newsheet::open_composer(m);
    }
    let reg = m.reg.clone();
    match pick_setup(&setups(m), arg, &|k| efforts_of(&reg, k)) {
        Err(e) => {
            m.flash_err(&e);
            Out::Draw
        }
        Ok(o) => {
            accounts(m).start_over = Some(o);
            if !msg.is_empty() {
                let o = peek(m).start_over.clone().unwrap_or_default();
                let dir = start_dir(m);
                return start_in(m, &o, msg, &dir);
            }
            let w = start_with_words(m, &start_dir(m));
            m.flash(&format!("the next session starts as {w}"));
            Out::Draw
        }
    }
}

/// s after its first n words (afterWords).
pub fn after_words(s: &str, n: usize) -> &str {
    let mut s = s;
    for _ in 0..n {
        s = s.trim_start_matches([' ', '\t', '\n']);
        match s.find([' ', '\t', '\n']) {
            Some(i) => s = &s[i..],
            None => return "",
        }
    }
    s
}
