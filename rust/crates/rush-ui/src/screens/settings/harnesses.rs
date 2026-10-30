//! Settings › Harnesses: Every harness, then each harness: what every one
//! starts with, said once, then what one changes from it, with its
//! sign-in and its own settings. Which providers each runs, and their
//! models, are Settings › Config. After Go's
//! internal/ui/settings_harnesses.go (harnessesPage :17, harnessesBody
//! :37, everyDetail :62, harnessesSections :80, catalogHarnesses :90,
//! everySections :116, harnessChanges :143, harnessDetailSections :156,
//! changesSection :172, harnessConfigLink :241, harnessCatalogSections
//! :262), settings_agent.go (agentSections :21), settings_auth.go
//! (nativeAuthSection :11, nativeSignIn :32, nativeAccountRows :62),
//! settings_claude.go (claudeSections :9, claudeRunSections :18) and
//! settings_general.go:243 (attributionSetting).

use std::rc::Rc;

use rush_core::adapter::Feature;
use rush_core::text::Line;
use rush_state::Start;
use rush_state::setup::MODE_APPROVE_ALL;

use super::acct::accounts;
use super::catalog;
use super::common::{LOGINS_KIND, frame_cursor, harness_name, load_models, reg};
use super::form::{self, Section, Setting, choice, flat, row_at, set};
use super::paint::on_off;
use super::workspace::Geometry;
use super::{
    PAGE_CONFIG, PAGE_HARNESSES, Page, config, config_mut, dialog, dialog_mut, open_dialog,
};
use crate::frame::Out;
use crate::model::Model;
use crate::style::*;

/// The page.
pub fn page() -> Page {
    Page {
        name: "Harnesses",
        pre: Some(|m, s| {
            let d = dialog_mut(m);
            if d.harness_inside && matches!(s, "esc" | "backspace") {
                d.harness_inside = false;
                d.cursor = d.harness_return;
                d.catalog_scroll.set(0);
                return Out::Draw;
            }
            let inside = d.harness_inside;
            catalog::key(m, s, inside)
        }),
        rows: Some(|m| flat(&sections(m)).len()),
        key: Some(|m, s| {
            if s == "right" || s == "enter" {
                dialog_mut(m).catalog_scroll.set(0);
            }
            let secs = sections(m);
            form::form_key(m, &secs, s)
        }),
        body: Some(body),
        prep: Some(prep),
        ..Page::default()
    }
}

/// The list's lines: Every harness (""), then each harness
/// (harnessItems).
pub fn items() -> Vec<String> {
    let mut v = vec![String::new()];
    v.extend(catalog_harnesses());
    v
}

/// What harnessesBody settles before drawing: the line picked kept in
/// the list, and the harness it shows.
fn prep(m: &mut Model) {
    let n = items().len();
    let d = dialog_mut(m);
    let picked = if d.harness_inside {
        d.harness_return
    } else {
        d.cursor
    }
    .min(n.saturating_sub(1));
    if d.harness_inside {
        d.harness_return = picked;
    } else {
        d.cursor = picked;
    }
    let h = items()[picked].clone();
    let r = reg();
    if h.is_empty() || r.harness_of(&d.model_kind) != r.harness_of(&h) {
        d.model_kind = h;
    }
    if d.harness_inside {
        let n = flat(&sections(m)).len();
        let d = dialog_mut(m);
        d.cursor = d.cursor.min(n.saturating_sub(1));
    }
}

/// The page drawn (harnessesBody).
fn body(m: &Model, geo: &mut Geometry, w: usize, _h: usize) -> Vec<Line> {
    let d = dialog(m);
    let cat = catalog_sections(m);
    let n = items().len();
    let picked = if d.harness_inside {
        d.harness_return
    } else {
        d.cursor
    }
    .min(n.saturating_sub(1));
    let inside = d.harness_inside;
    let k = d.model_kind.clone();
    let mut detail = |dw: usize| {
        if k.is_empty() {
            every_detail(m, dw)
        } else {
            catalog::detail(m, &detail_sections(m), &k, "", inside, dw)
        }
    };
    catalog::body(m, geo, &cat, picked, inside, false, w, &mut detail, None)
}

/// Every harness's side of the page (everyDetail), and the setting on
/// each of its lines. Go leaves those out, so a click on Every harness's
/// rows did nothing there; here it gives them the keys, as on a
/// harness's own (catalogDetail's indices).
fn every_detail(m: &Model, w: usize) -> (Vec<Line>, usize, Vec<Option<usize>>) {
    let d = dialog(m);
    let secs = every_sections(m);
    let cur = if d.harness_inside {
        d.cursor.min(flat(&secs).len().saturating_sub(1))
    } else {
        usize::MAX
    };
    let mut rows = vec![
        line([spb("Every harness", TEXT)]),
        line([sp(
            "What every harness starts with; each one shows only what it changes.",
            DIM,
        )]),
    ];
    let head = rows.len();
    rows.extend(form::form_rows(&secs, cur, w));
    if d.harness_inside {
        rows.extend(form::about(&row_at(&secs, cur), w));
    }
    let at = frame_cursor(&rows);
    let indices = catalog::line_indices(&secs, head, rows.len());
    (rows, at, indices)
}

/// The rows the cursor goes through: the list, or what's open
/// (harnessesSections).
pub fn sections(m: &Model) -> Vec<Section> {
    let d = dialog(m);
    if !d.harness_inside {
        return catalog_sections(m);
    }
    if d.model_kind.is_empty() {
        return every_sections(m);
    }
    detail_sections(m)
}

/// Each harness once, by its own kind where it has one
/// (catalogHarnesses).
pub fn catalog_harnesses() -> Vec<String> {
    let r = reg();
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for a in r.all().iter() {
        let k = a.kind();
        if r.current_kind(k.as_str()) != k {
            continue;
        }
        let h = r.harness_of(k.as_str());
        if seen.contains(&h) {
            continue;
        }
        seen.push(h.clone());
        out.push(if r.get(h.as_str()).is_some() {
            h.0
        } else {
            k.0.clone()
        });
    }
    out
}

/// The efforts Every harness can name (everyEfforts).
const EVERY_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Every harness's rows: effort and permissions, then attribution
/// (everySections).
fn every_sections(m: &Model) -> Vec<Section> {
    let every = &config(m).dispatch.every;
    let mut effort: Vec<(String, String)> = vec![(
        String::new(),
        "each harness thinks as its own settings say.".into(),
    )];
    for e in EVERY_EFFORTS {
        effort.push((
            e.into(),
            format!("every harness that offers {e} starts with it; the rest keep their own."),
        ));
    }
    let effort: Vec<(&str, &str)> = effort
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let mut er = choice(
        "Effort",
        &every.effort,
        "How hard a new session thinks before acting: more is slower and spends more.",
        &effort,
        set(|m, v| config_mut(m).dispatch.every.effort = v.into()),
    );
    er.unset = "each harness's own".into();
    let mut pr = choice(
        "Permissions",
        &every.mode,
        "What a new session may do without asking you.",
        &[
            ("", "each harness asks as its own settings say."),
            (
                MODE_APPROVE_ALL,
                "every harness that can approves every tool without asking: bypassPermissions, full-access, yolo, as each calls it.",
            ),
        ],
        set(|m, v| config_mut(m).dispatch.every.mode = v.into()),
    );
    pr.unset = "each harness's own".into();
    pr.names
        .insert(MODE_APPROVE_ALL.into(), "approve everything".into());
    vec![
        Section {
            title: "New sessions start with".into(),
            note: "sessions rush starts; running ones keep theirs".into(),
            rows: vec![er, pr],
            ..Section::default()
        },
        Section {
            title: "Every agent".into(),
            note: "set once here, kept in each agent's own settings".into(),
            rows: vec![attribution_setting(m)],
            ..Section::default()
        },
    ]
}

/// Whether agents credit themselves, said once for every harness
/// (attributionSetting).
pub fn attribution_setting(m: &Model) -> Setting {
    let c = config(m);
    let mut a = choice(
        "Credit in commits and PRs",
        &c.everywhere.attribution,
        &format!(
            "Whether agents add themselves to the commits and pull requests they make (a Co-Authored-By line, a \"Generated with\" note). Set here, rush writes it into every agent's own settings, on every account, and again for any account or agent that turns up later: {}. Codex credits itself as your OpenAI account decides, which no setting of its own changes; Gemini, Antigravity, Pi and Kimi never credit themselves.",
            crate::frame::everywhere::attribution_agents(&m.reg)
        ),
        &[
            ("", "each agent does as its own settings say."),
            ("off", "no agent credits itself."),
            ("on", "every agent credits itself, as it does by default."),
        ],
        None,
    );
    a.unset = "as each agent is set".into();
    a.run = Some(Rc::new(|m: &mut Model, v: &str| {
        config_mut(m).everywhere.attribution = v.into();
        m.apply_everywhere(v, true);
        Out::Draw
    }));
    a
}

/// How many of Every harness's settings harness h changes
/// (harnessChanges).
fn changes(m: &Model, h: &str) -> usize {
    let st = config(m)
        .dispatch
        .harness_start(reg().harness_of(h).as_str());
    usize::from(!st.effort.is_empty()) + usize::from(!st.mode.is_empty())
}

/// What an open harness shows (harnessDetailSections).
pub fn detail_sections(m: &Model) -> Vec<Section> {
    let k = dialog(m).model_kind.clone();
    let mut secs = vec![changes_section(m, &k)];
    if let Some(auth) = native_auth_section(m, &k) {
        secs.push(auth);
    } else if !reg().key_only(&k)
        && let Some(a) = super::acct::account_section(m, &k)
    {
        secs.push(a);
    }
    secs.extend(agent_sections(m, &k));
    secs.push(config_link(&k));
    secs
}

/// What harness k's sessions start with where it differs from Every
/// harness: unset is Every harness's (changesSection).
fn changes_section(m: &Model, k: &str) -> Section {
    let r = reg();
    let name = r.harness_label(k);
    let h = r.harness_of(k).0;
    let d = &config(m).dispatch;
    let mut sec = Section {
        title: name.clone(),
        note: "what it changes from every harness".into(),
        ..Section::default()
    };
    if !r.supports(k, Feature::Run) {
        let n = name.clone();
        sec.rows.push(Setting {
            label: "rush can't start its sessions yet".into(),
            line: Some(Rc::new(move |_| {
                line([sp(
                    format!(
                        "rush can't start {n} sessions yet, so there's nothing to choose here."
                    ),
                    FAINT,
                )])
            })),
            ..Setting::default()
        });
        return sec;
    }
    let ch = r.choices_of(k).unwrap_or_default();
    let st = d.harness_start(&h);
    let was = |every: &str| {
        if every.is_empty() {
            "as every harness".to_string()
        } else {
            format!("as every harness · {every}")
        }
    };
    let row = |label: &str,
               value: &str,
               every: &str,
               what: String,
               list: &[rush_core::adapter::Choice],
               f: fn(&mut Start, &str)| {
        let mut pairs: Vec<(&str, &str)> = vec![("", "as every harness says.")];
        for c in list {
            pairs.push((&c.id, &c.note));
        }
        let h2 = h.clone();
        let mut r = choice(
            label,
            value,
            &what,
            &pairs,
            set(move |m, v| {
                let d = &mut config_mut(m).dispatch;
                let mut s = d.harness_start(&h2);
                f(&mut s, v);
                d.set_harness_start(&h2, s);
            }),
        );
        r.unset = was(every);
        if !value.is_empty() && !every.is_empty() {
            r.names
                .insert(value.into(), format!("{value} · was {every}"));
        }
        r
    };
    if r.supports(k, Feature::Effort) {
        let mut x = row(
            "Effort",
            &st.effort,
            &d.every.effort,
            format!("How hard a new {name} session thinks before acting."),
            &ch.efforts,
            |s, v| s.effort = v.into(),
        );
        x.typed = ch.efforts.is_empty();
        sec.rows.push(x);
    }
    if r.supports(k, Feature::Modes) {
        let every = if d.every.mode == MODE_APPROVE_ALL {
            "approve everything".to_string()
        } else {
            d.every.mode.clone()
        };
        let mut x = row(
            "Permissions",
            &st.mode,
            &every,
            format!("What a new {name} session may do without asking you."),
            &ch.modes,
            |s, v| s.mode = v.into(),
        );
        x.typed = ch.modes.is_empty();
        sec.rows.push(x);
    }
    if sec.rows.is_empty() {
        sec.rows.push(Setting {
            label: "nothing to change".into(),
            line: Some(Rc::new(move |_| {
                line([sp(
                    format!("{name} has no effort or permissions of its own to set."),
                    FAINT,
                )])
            })),
            ..Setting::default()
        });
    }
    sec
}

/// Opens Settings › Config on harness k's row (harnessConfigLink).
fn config_link(k: &str) -> Section {
    let label = "Providers it runs, and their models";
    let k2 = k.to_string();
    Section {
        rows: vec![Setting {
            label: label.into(),
            line: Some(Rc::new(move |_| super::providers::link(label, "Config"))),
            what: format!(
                "Which providers {} runs, whether it's on, and the models each may call in it: Settings › Config.",
                reg().harness_label(k)
            ),
            keys: vec!["enter".into(), "open".into()],
            key: Some(Rc::new(move |m, s| {
                if s != "enter" && s != "right" {
                    return Out::Pass;
                }
                open_dialog(m, PAGE_CONFIG);
                let h = reg().harness_of(&k2);
                let row = super::configpage::harnesses(m)
                    .iter()
                    .position(|x| *x == h.0)
                    .unwrap_or(0);
                dialog_mut(m).cursor = row;
                Out::Draw
            })),
            ..Setting::default()
        }],
        ..Section::default()
    }
}

/// The list: Every harness, then each harness with what it changes
/// (harnessCatalogSections).
pub fn catalog_sections(m: &Model) -> Vec<Section> {
    fn enter(i: usize, k: String) -> form::KeyFn {
        Rc::new(move |m, s| {
            if s != "enter" && s != "right" {
                return Out::Pass;
            }
            let d = dialog_mut(m);
            d.harness_inside = true;
            d.harness_return = i;
            d.cursor = 0;
            d.model_kind = k.clone();
            if !k.is_empty() {
                load_models(m, &k);
            }
            Out::Draw
        })
    }
    let r = reg();
    let hs = catalog_harnesses();
    let mut rows = vec![Setting {
        label: "Every harness".into(),
        what: "What every harness starts with: effort, permissions, credit in commits.".into(),
        key: Some(enter(0, String::new())),
        keys: vec!["right".into(), "open".into()],
        ..Setting::default()
    }];
    for (i, k) in hs.into_iter().enumerate() {
        let name = r.harness_label(&k);
        let n = changes(m, &k);
        let tag = if !r.runs(&k) {
            line([sp("not installed", FAINT)])
        } else if !config(m).harness_on(r.harness_of(&k).as_str()) {
            line([sp("off", FAINT)])
        } else if n == 1 {
            line([sp("1 change", ORANGE)])
        } else if n > 1 {
            line([sp(format!("{n} changes"), ORANGE)])
        } else {
            Line::new()
        };
        rows.push(Setting {
            label: name.clone(),
            tag,
            what: format!(
                "{name} runs the model. What it changes from every harness, its sign-in and its own settings are beside the list."
            ),
            key: Some(enter(i + 1, k)),
            keys: vec!["right".into(), "open".into()],
            ..Setting::default()
        });
    }
    vec![Section {
        rows,
        ..Section::default()
    }]
}

/// Opens harness k on Harnesses (harnessLink's key, configKey's enter).
pub fn open_harness(m: &mut Model, k: &str) {
    open_dialog(m, PAGE_HARNESSES);
    let h = reg().harness_of(k);
    let i = catalog_harnesses()
        .iter()
        .position(|x| reg().harness_of(x) == h)
        .map_or(0, |i| i + 1);
    let d = dialog_mut(m);
    d.harness_return = i;
    d.harness_inside = true;
    d.cursor = 0;
    d.model_kind = k.to_string();
    load_models(m, k);
}

/// The sections an agent adds to its page, by kind (agentExtras).
fn extras(m: &Model, k: &str) -> Vec<Section> {
    if k == LOGINS_KIND {
        return claude_sections(m);
    }
    Vec::new()
}

/// Agent k's own sections, the advanced ones folded under a line of
/// their own (agentSections).
pub fn agent_sections(m: &Model, k: &str) -> Vec<Section> {
    let mut secs = Vec::new();
    let mut advanced = Vec::new();
    let mut all = extras(m, k);
    all.extend(super::files::file_sections(m, k));
    for s in all {
        if s.advanced {
            advanced.push(s);
        } else {
            secs.push(s);
        }
    }
    if advanced.is_empty() {
        return secs;
    }
    let names: Vec<String> = advanced.iter().map(|s| s.title.clone()).collect();
    let open = dialog(m).advanced;
    let joined = names.join(", ");
    let (j2, k2) = (joined.clone(), k.to_string());
    let toggle = Setting {
        label: "Advanced".into(),
        line: Some(Rc::new(move |_| {
            let mark = if open { "▾ " } else { "▸ " };
            line([
                sp(format!("{mark}Advanced"), DIM),
                sp(format!(" · {joined}"), FAINT),
            ])
        })),
        key: Some(Rc::new(|m, s| {
            if matches!(s, "enter" | "right" | "left" | "space") {
                let d = dialog_mut(m);
                d.advanced = !d.advanced;
                return Out::Draw;
            }
            Out::Pass
        })),
        keys: vec!["enter".into(), "show or hide".into()],
        about: Some(Rc::new(move || {
            (
                "Advanced".into(),
                format!(
                    "What {} itself reads, beyond what rush starts it with: {j2}. Most people never need these.",
                    harness_name(&k2)
                ),
                String::new(),
            )
        })),
        ..Setting::default()
    };
    secs.push(Section {
        rows: vec![toggle],
        ..Section::default()
    });
    if open {
        secs.extend(advanced);
    }
    secs
}

/// The signed-in agent's own sections: how rush runs it, and the
/// definitions its sessions can start as (claudeSections).
fn claude_sections(m: &Model) -> Vec<Section> {
    let (run, tuning) = claude_run_sections(m);
    let mut secs = vec![run, tuning];
    if let Some(def) = super::files::definitions_section(m, LOGINS_KIND) {
        secs.push(def);
    }
    secs
}

/// How rush runs Claude Code, and its tuning (claudeRunSections).
fn claude_run_sections(m: &Model) -> (Section, Section) {
    let cfg = config(m);
    let d = &cfg.dispatch;
    let mut run_in = choice(
        "Run new sessions in",
        &d.run_in,
        "Where new Claude Code sessions run. rush mode runs Claude Code headless in rush's own host and draws the conversation here; the daemon is Claude Code's own background service and its terminal screen.",
        &[
            (
                "",
                "rush's conversation view, queue, approvals and overview. /rush moves a daemon session over.",
            ),
            (
                "daemon",
                "Claude Code's background service, shown through its own terminal screen.",
            ),
        ],
        set(|m, v| config_mut(m).dispatch.run_in = v.into()),
    );
    run_in.unset = "rush mode".into();
    let mut on_limit = choice(
        "When a usage limit hits",
        &d.on_limit,
        "What a rush-mode session does when a 5-hour or weekly usage limit stops it.",
        &[
            (
                "",
                "it asks once whether to continue by itself when the limit resets.",
            ),
            (
                "auto",
                "every session continues by itself at the reset, a few seconds apart.",
            ),
            ("off", "sessions wait for you after a limit."),
        ],
        set(|m, v| config_mut(m).dispatch.on_limit = v.into()),
    );
    on_limit.unset = "ask each session".into();
    let quick = choice(
        "Quick start",
        on_off(cfg.dispatch.quick_start()),
        "Starts rush-mode sessions without Claude Code's non-essential network traffic (CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC), for them alone. Claude Code is ready in about 0.3s instead of 0.65s, which you feel on every new session and every message after one has rested.",
        &[
            (
                "on",
                "quicker starts, but no DesignSync, Projects, plugin downloads or live preview in them. Your telemetry settings are unaffected.",
            ),
            ("off", "sessions start with everything Claude Code has."),
        ],
        set(|m, v| config_mut(m).dispatch.lean = Some(v == "on")),
    );
    let compress = choice(
        "Compress idle transcripts",
        on_off(!cfg.keep_transcripts_plain),
        "Transcripts untouched for two days are stored compressed the way macOS stores its own system files: same names, same contents, and Claude Code, grep, your editor and rush read them exactly as before; the system decompresses as they're read (about 30 ms for a 33 MB one). A transcript written to again is stored plainly again. Each is checked byte for byte before it replaces the original.",
        &[
            (
                "on",
                "about a quarter of the disk they took (33 MB → 8 MB for the biggest).",
            ),
            ("off", "transcripts stay as Claude Code writes them."),
        ],
        set(|m, v| config_mut(m).keep_transcripts_plain = v == "off"),
    );
    (
        Section::new("How rush runs it", vec![run_in, on_limit]),
        Section {
            title: "Tuning".into(),
            advanced: true,
            rows: vec![quick, compress],
            ..Section::default()
        },
    )
}

/// An agent's own sign-in, where it has one: its status and who, then a
/// row that runs its sign-in (nativeAuthSection).
pub fn native_auth_section(m: &Model, k: &str) -> Option<Section> {
    let a = reg().get(k)?;
    a.authenticator()?;
    let mut rows = native_account_rows(m, k);
    let k2 = k.to_string();
    rows.push(Setting {
        label: "Sign in".into(),
        what: format!(
            "Open {}'s own sign-in/setup in the terminal. Finish sign-in, then exit that program to return to rush.",
            reg().harness_label(k)
        ),
        keys: vec!["enter".into(), "sign in".into()],
        key: Some(Rc::new(move |m, s| {
            if s != "enter" {
                return Out::Pass;
            }
            native_sign_in(m, &k2)
        })),
        ..Setting::default()
    });
    Some(Section::new("Account", rows))
}

/// Runs k's own sign-in with the terminal handed to it; once it ends
/// well, who it's signed in as and its models are read again
/// (nativeSignIn, settings_auth.go:32).
pub fn native_sign_in(m: &mut Model, k: &str) -> Out {
    match reg().get(k) {
        Some(a) => native_sign_in_with(m, a, k),
        None => Out::Idle,
    }
}

/// [`native_sign_in`] with agent a.
pub fn native_sign_in_with(m: &mut Model, a: rush_core::adapter::AdapterRef, k: &str) -> Out {
    if a.authenticator().is_none() {
        return Out::Idle;
    }
    let p = super::common::profile_of(m, &a).unwrap_or_else(|| rush_core::adapter::Profile {
        kind: rush_core::adapter::Kind::new(k),
        ..Default::default()
    });
    let k = k.to_string();
    // Making its command may touch the disk: off the UI thread
    // (inTerminal).
    m.later(
        move || {
            a.authenticator()
                .map(|au| au.sign_in_command(&p).map_err(|e| e.to_string()))
        },
        move |m, cmd| {
            let cmd = match cmd {
                Some(Ok(c)) => c,
                Some(Err(e)) => {
                    m.fail(&format!("Sign-in failed: {e}"));
                    return Out::Draw;
                }
                None => return Out::Idle,
            };
            m.exec(
                cmd,
                Box::new(move |m, r| {
                    if let Err(e) = r {
                        m.fail(&format!("Sign-in failed: {e}"));
                        return Out::Draw;
                    }
                    let h = rush_core::adapter::migrated(&k).0;
                    m.later(move || rush_state::health::reset(&h, ""), |_, ()| Out::Idle);
                    m.flash("Sign-in finished · checking credentials");
                    (accounts().find_sign_ins)(m);
                    load_models(m, &k);
                    Out::Draw
                }),
            );
            Out::Draw
        },
    );
    Out::Draw
}

/// k's account as its own sign-in says (nativeAccountRows).
pub(crate) fn native_account_rows(m: &Model, k: &str) -> Vec<Setting> {
    let a = accounts();
    let info = (a.summary)(m, k);
    let mut status = if (a.signed_in)(m, k) {
        "credentials available".to_string()
    } else {
        "not checked".to_string()
    };
    let why = (a.why)(m, k);
    if !why.is_empty() {
        status = why;
    }
    let info = match info {
        Some(info) => {
            status = if !info.status.is_empty() && info.status != "unknown" {
                info.status.clone()
            } else if info.signed_in || (a.signed_in)(m, k) {
                "credentials available".into()
            } else {
                "unknown".into()
            };
            info
        }
        None => Default::default(),
    };
    let mut rows = vec![Setting {
        label: "Status".into(),
        value: status,
        what: info.note.clone(),
        ..Setting::default()
    }];
    for (label, value) in [
        ("Account", &info.name),
        ("Email", &info.email),
        ("Organization", &info.org),
        ("Plan", &info.plan),
        ("Sign-in method", &info.method),
        ("Endpoint", &info.endpoint),
    ] {
        if !value.is_empty() {
            rows.push(Setting {
                label: label.into(),
                value: value.clone(),
                ..Setting::default()
            });
        }
    }
    let k2 = k.to_string();
    rows.push(Setting {
        label: "Refresh account and models".into(),
        what:
            "Read account information and the current model catalog without changing this session."
                .into(),
        keys: vec!["enter".into(), "refresh".into()],
        key: Some(Rc::new(move |m, s| {
            if s != "enter" {
                return Out::Pass;
            }
            (accounts().find_sign_ins)(m);
            load_models(m, &k2);
            Out::Draw
        })),
        ..Setting::default()
    });
    rows
}
