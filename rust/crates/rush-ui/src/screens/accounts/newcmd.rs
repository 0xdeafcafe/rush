//! #new starts an agent on any route once, from the box:
//!
//! ```text
//! #new [harness][@provider[:account]] [model] [effort] [task…]
//! ```
//!
//! Each part is optional, the rest coming from the route's defaults; a
//! word is taken only when it names one exactly, and the first that
//! doesn't starts the task. With no task it sets the next session, as #use
//! does. Nothing it does changes a default. #with is the same, without a
//! task. After Go's internal/ui/newcmd.go (newCommand :35, startAs :51,
//! newPreview :68, parseNew :91, newTarget :147, routeStart :189,
//! routesInto :230, modelRoute :245, routeRow.word :280, newArgs :312,
//! readCatalogs :367, harnessWords :391, providerWords :399) and with.go:13
//! (withAgent).

use rush_core::event::Command;
use rush_core::text::Line;
use rush_state::profiles::BILLING_KEY;

use super::look::{glyph, pays_by_key, prov_kind, prov_label, prov_of};
use super::rows::{account_of, start_dir};
use super::setup::{
    RouteRow, StartOver, account_names, efforts_of, models, name_word, next_start, pick_setup,
    route_rows, setups, start_as, start_id, start_on, start_with_words, start_words,
};
use super::state::{accounts, cfg, ensure_store};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// #new's words, read: the start, the task, and what to know (newStart).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewStart {
    pub o: StartOver,
    pub task: String,
    pub warn: Vec<String>,
}

/// What #new needs read once: every installed agent's models.
#[derive(Default)]
struct Catalogs {
    asked: bool,
}

/// The usage line a #new that names nothing it knows says (errNoRoute).
pub const NO_ROUTE: &str = "#new [harness][@provider[:account]] [model] [effort] task";

/// #new (or #with) with arg (newCommand).
pub fn new_command(m: &mut Model, arg: &str) -> Out {
    ensure_store(m);
    read_catalogs(m);
    let n = match parse_new(m, arg) {
        Ok(n) => n,
        Err(e) => {
            m.fail(&e);
            return Out::Draw;
        }
    };
    if n.task.is_empty() {
        let mut words = vec![format!(
            "the next session starts as {}",
            start_words(m, &n.o).join(" · ")
        )];
        words.extend(n.warn.iter().cloned());
        accounts(m).start_over = Some(n.o);
        m.flash(&words.join(" · "));
        return Out::Draw;
    }
    start_as(m, n.o, &n.task)
}

/// #with: what the next session starts as, once, as #new without a task;
/// alone, it says what that is. It never changes a default: Settings ›
/// Providers does (withAgent).
pub fn with_command(m: &mut Model, arg: &str) -> Out {
    ensure_store(m);
    if !arg.trim().is_empty() {
        return new_command(m, arg);
    }
    let w = start_with_words(m, &start_dir(m));
    m.flash(&format!(
        "the next session starts as {w} · #with harness@provider picks another, once"
    ));
    Out::Draw
}

/// What #new or #with being typed starts, for the box's border: the
/// start, or why it can't. None for any other text (newPreview).
pub fn new_preview(m: &Model, text: &str) -> Option<Line> {
    let rest = text.strip_prefix('#')?;
    let (name, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    if name != "new" && name != "with" {
        return None;
    }
    let n = match parse_new(m, arg) {
        Ok(n) => n,
        Err(e) => {
            // The border drops what doesn't fit.
            let room = 24.max(m.w.saturating_sub(40));
            let t = rush_draw::truncate(&format!("✗ {e}"), room, "…").into_owned();
            return Some(line([sp(t, RED)]));
        }
    };
    let when = if n.task.is_empty() {
        " · next session"
    } else {
        " · once, defaults untouched"
    };
    let mut l = super::setup::setup_chip(m, &n.o);
    l.spans.push(sp(when, DIM));
    for w in &n.warn {
        l.spans.push(sp(format!(" · {w}"), YELLOW));
    }
    Some(l)
}

/// The word at rest's head.
fn head(rest: &str) -> &str {
    rest.split_whitespace().next().unwrap_or("")
}

/// rest after its head word.
fn eat(rest: &str) -> String {
    let h = head(rest);
    rest.trim_start()
        .strip_prefix(h)
        .unwrap_or(rest)
        .trim()
        .to_string()
}

fn has_model(m: &Model, kind: &str, w: &str) -> bool {
    models(m, kind).iter().any(|c| c.id.eq_ignore_ascii_case(w))
}

fn has_effort(m: &Model, kind: &str, w: &str) -> bool {
    efforts_of(&m.reg, kind)
        .iter()
        .any(|c| c.id.eq_ignore_ascii_case(w))
}

/// #new's argument, read (parseNew).
pub fn parse_new(m: &Model, arg: &str) -> Result<NewStart, String> {
    let mut rest = arg.trim().to_string();
    let (mut o, picked) = new_target(m, head(&rest))?;
    if picked {
        rest = eat(&rest);
    } else {
        o = next_start(m, &start_dir(m));
        let w = head(&rest).to_string();
        if !w.is_empty()
            && !has_model(m, &o.kind, &w)
            && let Some(mo) = model_route(m, &w)
        {
            o = mo; // a model alone picks the provider that serves it
            rest = eat(&rest);
        }
    }
    for _ in 0..2 {
        let w = head(&rest).to_string();
        if w.is_empty() {
            break;
        }
        if o.model != w && has_model(m, &o.kind, &w) {
            o.model = w;
            rest = eat(&rest);
            continue;
        }
        if has_effort(m, &o.kind, &w) {
            o.effort = w.to_lowercase();
            rest = eat(&rest);
            continue;
        }
        break;
    }
    if rest.len() > 1 && rest.starts_with('"') && rest.ends_with('"') {
        rest = rest[1..rest.len() - 1].to_string();
    }
    let mut n = NewStart {
        task: rest,
        ..NewStart::default()
    };
    let k = o.kind.clone();
    let (_, warn, _) = m
        .reg
        .compat(&start_id(&m.reg, &o), m.reg.harness_of(&k).as_str());
    if !warn.is_empty() {
        n.warn.push(warn);
    }
    if !o.account.is_empty() && o.account != account_of(m, &k) {
        let h = m.reg.harness_label(&k);
        n.warn.push(format!(
            "swaps {h}'s sign-in to {} for every {h} session",
            o.account
        ));
    }
    n.o = o;
    Ok(n)
}

/// #new's first word as a route: harness@provider[:account], harness
/// alone, @provider alone, or a name #use took. false when it names none,
/// and it's the task's (newTarget).
pub fn new_target(m: &Model, w: &str) -> Result<(StartOver, bool), String> {
    if w.is_empty() {
        return Ok((StartOver::default(), false));
    }
    let reg = &m.reg;
    let (hw, pw, at) = match w.split_once('@') {
        Some((h, p)) => (h, p, true),
        None => (w, "", false),
    };
    let h = reg.harness_by_word(hw).0;
    if !at {
        if !h.is_empty() {
            return route_start(m, "", &h, "").map(|o| (o, true));
        }
        if let Ok(o) = pick_setup(&setups(m), w, &|k| efforts_of(reg, k)) {
            return Ok((o, true));
        }
        return Ok((StartOver::default(), false));
    }
    if !hw.is_empty() && h.is_empty() {
        return Err(format!(
            "no harness called {hw} · {}",
            harness_words(m).join(", ")
        ));
    }
    let (pw, acct) = pw.split_once(':').unwrap_or((pw, ""));
    let id = match pw {
        "" => String::new(),
        "sub" | "api" => {
            if h.is_empty() {
                return Err(format!(
                    "@{pw} is the harness's own provider: name the harness, codex@{pw}"
                ));
            }
            let mut id = reg.provider_of(&h);
            if pw == "api" && reg.split(&id) {
                id = rush_core::adapter::key_of(&id);
            }
            id
        }
        _ => {
            let id = reg.provider_by_word(pw);
            if id.is_empty() {
                return Err(format!(
                    "no provider called {pw} · {}",
                    provider_words(m).join(", ")
                ));
            }
            id
        }
    };
    route_start(m, &id, &h, acct).map(|o| (o, true))
}

/// A start on provider id in harness h, either one empty for its default:
/// id's default harness, or h on the next session's provider when h runs
/// it, else the first whose ★ is h, else any h runs (routeStart).
pub fn route_start(m: &Model, id: &str, h: &str, acct: &str) -> Result<StartOver, String> {
    let reg = &m.reg;
    let c = cfg(m);
    let mut h = h.to_string();
    if !id.is_empty() && h.is_empty() {
        h = reg.harness_of(prov_kind(reg, c, id).as_str()).0;
    }
    let mut ids = vec![id.to_string()];
    if id.is_empty() {
        ids = vec![start_id(reg, &next_start(m, &start_dir(m)))];
        let mut others = reg.provider_ids();
        // Stable: those whose default harness is h first.
        others.sort_by_key(|id| reg.harness_of(prov_kind(reg, c, id).as_str()).0 != h);
        ids.extend(others);
    }
    let mut first: Option<String> = None;
    for id in &ids {
        let (k, _, mut why) = reg.compat(id, &h);
        if why.is_empty() && pays_by_key(reg, id) && !c.has_api_key(&prov_of(reg, id)) {
            why = format!(
                "no {} API key yet: $ adds one in Settings › Providers",
                reg.provider_label(&prov_of(reg, id))
            );
        }
        if !why.is_empty() {
            if first.is_none() {
                first = Some(format!("{why} · try {}", routes_into(m, id, &h)));
            }
            continue;
        }
        let mut o = start_on(m, id, k.as_str());
        if acct.is_empty() {
            return Ok(o);
        }
        if o.billing == BILLING_KEY {
            return Err(format!(
                "{} pays with its API key: it has no accounts",
                prov_label(reg, id)
            ));
        }
        let names = account_names(m, k.as_str());
        let Some(n) = names.iter().find(|n| name_word(n) == name_word(acct)) else {
            return Err(format!(
                "no {} account called {acct} · {}",
                reg.harness_label(k.as_str()),
                names.join(", ")
            ));
        };
        o.account = n.clone();
        return Ok(o);
    }
    Err(first.unwrap_or_else(|| format!("{} runs no provider here", reg.harness_label(&h))))
}

/// The route as #new takes it: codex@openai-sub (routeRow.word).
pub fn route_word(m: &Model, r: &RouteRow) -> String {
    format!(
        "{}@{}",
        m.reg.harness_word(r.harness.as_str()),
        m.reg.provider_word(&r.id)
    )
}

/// The routes worth trying instead of id in h, as words: h on another
/// provider, or id in another harness (routesInto).
pub fn routes_into(m: &Model, id: &str, h: &str) -> String {
    let out: Vec<String> = route_rows(m)
        .iter()
        .filter(|r| r.why.is_empty() && (r.id == id || r.harness.as_str() == h))
        .take(2)
        .map(|r| route_word(m, r))
        .collect();
    if out.is_empty() {
        return "#new alone for the list".into();
    }
    out.join(" or ")
}

/// The start of the first route that serves model, the next session's
/// provider first (modelRoute).
pub fn model_route(m: &Model, model: &str) -> Option<StartOver> {
    let mut rows = route_rows(m);
    let now = start_id(&m.reg, &next_start(m, &start_dir(m)));
    rows.sort_by_key(|r| r.id != now);
    let r = rows
        .iter()
        .find(|r| r.why.is_empty() && has_model(m, r.kind.as_str(), model))?;
    let mut o = start_on(m, &r.id, r.kind.as_str());
    o.model = model.to_string();
    Some(o)
}

/// The completions for #new's (or #with's) words in q, the word being
/// typed: routes first, then the route's models and efforts (newArgs).
pub fn new_args(m: &Model, name: &str, q: &str) -> Vec<Command> {
    let mut words: Vec<&str> = q.split_whitespace().collect();
    let mut typing = String::new();
    if !words.is_empty() && !q.ends_with(' ') {
        typing = words.pop().unwrap_or_default().to_lowercase();
    }
    let lead = format!("{name} {}", words.join(" ")).trim().to_string();
    let (mut first, mut rest) = (Vec::new(), Vec::new());
    let mut add = |arg: &str, note: String| {
        let c = Command {
            name: format!("{lead} {arg}"),
            description: note.clone(),
            argument_hint: "[next]".into(),
            ..Command::default()
        };
        if arg.to_lowercase().starts_with(&typing) {
            first.push(c);
        } else if !typing.is_empty() && format!("{arg} {note}").to_lowercase().contains(&typing) {
            rest.push(c);
        }
    };
    if words.is_empty() {
        for r in route_rows(m) {
            let mut note = format!(
                "{} {} · {}",
                glyph(&m.reg, r.kind.as_str()).text,
                m.reg.harness_label(r.harness.as_str()),
                prov_label(&m.reg, &r.id)
            );
            if !r.why.is_empty() {
                note += &format!("  ✗ {}", r.why);
            } else if r.def {
                note += &format!("  ★ {}'s default", m.reg.provider_word(&r.id));
            }
            if !r.warn.is_empty() {
                note += &format!("  · {}", r.warn);
            }
            add(&route_word(m, &r), note);
        }
        first.extend(rest);
        return first;
    }
    let Ok(n) = parse_new(m, &words.join(" ")) else {
        return Vec::new();
    };
    if !n.task.is_empty() {
        return Vec::new(); // past the route's words: the task is being typed
    }
    let k = n.o.kind.clone();
    let def = start_on(m, &start_id(&m.reg, &n.o), &k);
    if n.o.model.is_empty() || n.o.model == def.model {
        for c in models(m, &k) {
            let mut note = format!("{}  {}", c.id, one_line(&c.note));
            if c.id == def.model {
                note += "  ★ default here";
            }
            add(&c.id, note);
        }
    }
    if n.o.effort.is_empty() || n.o.effort == def.effort {
        for e in efforts_of(&m.reg, &k) {
            add(&e.id, format!("{} effort · {}", e.id, e.note));
        }
    }
    first.extend(rest);
    first
}

/// Reads every installed agent's models once, off the UI thread, for
/// #new's words; a frame after they land has them (readCatalogs).
pub fn read_catalogs(m: &mut Model) {
    if m.kept::<Catalogs>().asked {
        return;
    }
    m.kept::<Catalogs>().asked = true;
    super::newsheet::load_all_models(m);
}

/// Every harness's word (harnessWords).
pub fn harness_words(m: &Model) -> Vec<String> {
    m.reg
        .harness_kinds()
        .iter()
        .map(|h| m.reg.harness_word(h.as_str()))
        .collect()
}

/// Every installed provider's word (providerWords).
pub fn provider_words(m: &Model) -> Vec<String> {
    m.reg
        .provider_ids()
        .iter()
        .filter(|id| m.reg.provider_installed(&prov_of(&m.reg, id)))
        .map(|id| m.reg.provider_word(id))
        .collect()
}

/// s on one line.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Marks catalogs as read, for tests that list models by hand.
pub fn skip_catalogs(m: &mut Model) {
    m.kept::<Catalogs>().asked = true;
}
