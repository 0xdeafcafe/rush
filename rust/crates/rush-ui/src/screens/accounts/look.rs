//! What each provider is called and how it looks: its glyph and colour,
//! taken from its brand where it has one, so which one a session, the
//! header or the top bar is on can be told at a glance; and the words for
//! providers, harnesses and how they're paid for. After Go's
//! internal/ui/providers.go (look :21-45, lookOf :52, colour :71,
//! harnessBorder :95, glyph :106, spinOf :115, rowBadge :138,
//! providerName :163, runsOn :186, levelChip :215, chain :224,
//! notInstalled :245, runsInWords profiles.go:239), with.go (modelWord
//! :23, agentName :33, harnessName :37) and settings_providers.go
//! (provKind :116, provLabel :127, provName :138, billWord :151,
//! paysByKey :162), accounts.go:660 (kindName).

use rush_core::adapter::{Kind, Level, Registry};
use rush_core::text::{Color, Line, Span};
use rush_state::Profile;

use crate::frame::ground::{self, Rgb};
use crate::style::*;

/// The provider whose accounts are Config.logins, switched in its one
/// home by the vault rather than through its adapter (loginsKind).
pub const LOGINS_KIND: &str = rush_state::LOGINS_KIND;

/// A provider's glyph and colour, the colour as it is on rush's dark
/// ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub glyph: &'static str,
    pub rgb: Rgb,
}

/// Claude Code's: its starburst, in rush's orange.
pub const BUILTIN_LOOK: Look = Look {
    glyph: "✻",
    rgb: Rgb(217, 119, 87),
};
/// A provider without one of its own.
pub const OTHER_LOOK: Look = Look {
    glyph: "◇",
    rgb: Rgb(143, 179, 217),
};

/// The rest, by kind.
const LOOKS: &[(&str, Look)] = &[
    ("codex", look("◎", 94, 199, 160)),     // OpenAI's green
    ("copilot", look("◈", 178, 150, 230)),  // GitHub's purple
    ("deepseek", look("◆", 116, 146, 255)), // DeepSeek's blue
    ("glm", look("▲", 96, 196, 222)),       // Z.ai's cyan
    ("antigravity", look("✦", 138, 180, 248)),
    ("gemini", look("✦", 138, 180, 248)), // Gemini's sparkle
    ("kimi", look("◐", 200, 200, 214)),   // Moonshot's moon
    ("vibe", look("■", 245, 165, 60)),    // Mistral's amber
    ("ollama", look("◉", 232, 232, 226)), // Ollama's white llama
    ("opencode", look("▣", 186, 182, 176)),
    ("pi", look("π", 230, 190, 120)),
];

const fn look(glyph: &'static str, r: u8, g: u8, b: u8) -> Look {
    Look {
        glyph,
        rgb: Rgb(r, g, b),
    }
}

fn own_look(k: &str) -> Option<Look> {
    LOOKS.iter().find(|(n, _)| *n == k).map(|(_, l)| *l)
}

/// Whether provider k's rows go without its mark, as they were before
/// rush ran other agents: the one whose accounts are rush's logins
/// (unmarked).
pub fn unmarked(k: &str) -> bool {
    k == LOGINS_KIND
}

/// Provider k's look. A provider in another's harness looks like the
/// provider: Ollama in Pi is Ollama's, Anthropic's plan in Pi Claude
/// Code's (lookOf).
pub fn look_of(reg: &Registry, k: &str) -> Look {
    if unmarked(k) {
        return BUILTIN_LOOK;
    }
    if let Some(l) = own_look(k) {
        return l;
    }
    let p = reg.provider_of(k);
    if p == LOGINS_KIND {
        return BUILTIN_LOOK;
    }
    own_look(&p).unwrap_or(OTHER_LOOK)
}

impl Look {
    /// The look's colour on the terminal's ground, as an accent (colour).
    pub fn colour(self) -> Color {
        ground::DARK.accent(self.rgb).color()
    }
}

/// Provider k's glyph in its colour (glyph).
pub fn glyph(reg: &Registry, k: &str) -> Span {
    let l = look_of(reg, k);
    sp(l.glyph, l.colour())
}

/// A quiet grey carrying just enough of the harness's hue to tell the
/// session apart without making its frame another accent (harnessBorder).
pub fn harness_border(reg: &Registry, k: &str) -> Color {
    let grey = Rgb(94, 89, 82);
    let h = reg.harness_of(k);
    let tint = ground::mix(grey, look_of(reg, h.as_str()).rgb, 0.22);
    ground::DARK.ink(tint).color()
}

/// Codex's turning circle.
const CODEX_SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// Frame n of what a run of provider k shows while it works: its
/// harness's spinner (Claude Code's star, Codex's circle, another's glyph)
/// in the provider's colour (spinOf).
pub fn spin_of(reg: &Registry, k: &str, n: usize) -> Span {
    let h = reg.harness_of(k);
    let c = look_of(reg, k).colour();
    let f = match h.as_str() {
        LOGINS_KIND => SPINNER[n % SPINNER.len()],
        "codex" => CODEX_SPIN[n % CODEX_SPIN.len()],
        _ => look_of(reg, h.as_str()).glyph,
    };
    sp(f, c)
}

/// Names every harness; the provider too when it runs through another's
/// (Ollama). Unless full, only the harness's glyph (rowBadge).
pub fn row_badge(reg: &Registry, kind: &str, full: bool) -> Span {
    let k = rush_core::adapter::migrated(kind);
    let l = look_of(reg, k.as_str());
    if !full {
        return sp(l.glyph, l.colour());
    }
    let mut label = harness_name(reg, k.as_str());
    if reg.harness_of(k.as_str()) != k {
        label = format!(
            "{label} · {}",
            provider_name(reg, &reg.provider_of(k.as_str()))
        );
    }
    sp(format!("{} {label}", l.glyph), l.colour())
}

// ---- Words ----

/// Model id as the agent of kind names it: "Opus 5.5" (modelWord).
pub fn model_word(reg: &Registry, kind: &str, id: &str) -> String {
    let k = if kind.is_empty() {
        rush_core::adapter::LEGACY_KIND
    } else {
        kind
    };
    reg.model_name(k, id)
}

/// What runs sessions of kind, as rush names it: Provider (Harness),
/// OpenAI (Codex) (agentName).
pub fn agent_name(reg: &Registry, kind: &str) -> String {
    reg.label(kind)
}

/// The harness sessions of kind run in: Claude Code, Codex (harnessName).
pub fn harness_name(reg: &Registry, kind: &str) -> String {
    reg.harness_label(kind)
}

/// An agent's name, short enough for a table (kindName).
pub fn kind_name(reg: &Registry, k: &str) -> String {
    if let Some(a) = reg.get(k) {
        let n = a.name();
        if rush_core::text::width(&n) <= 9 {
            return n;
        }
    }
    let mut c = k.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Provider p by the company behind its models, or its agent's short name
/// when the adapter doesn't say (providerName).
pub fn provider_name(reg: &Registry, p: &str) -> String {
    let n = reg.provider_name(p);
    if !n.is_empty() {
        return n;
    }
    kind_name(reg, p)
}

/// " in Pi" when agent k is its provider in another's harness
/// (runsInWords).
pub fn runs_in_words(reg: &Registry, k: &str) -> String {
    let h = reg.harness_of(k);
    if h.as_str() != k {
        return format!(" in {}", agent_name(reg, h.as_str()));
    }
    String::new()
}

/// What an agent runs on, each part only when known: its provider unless
/// it's Claude Code, its model, its effort (runsOn).
pub fn runs_on(reg: &Registry, k: &str, model: &str, effort: &str) -> String {
    let mut words = Vec::new();
    let n = kind_name(reg, k);
    if !n.is_empty() && !unmarked(k) {
        words.push(n);
    }
    if !model.is_empty() && !model.starts_with('<') {
        words.push(model_word(reg, k, model));
    }
    if !effort.is_empty() {
        words.push(format!("{effort} effort"));
    }
    words.join(" · ")
}

/// What each support level means (levelWords).
pub fn level_words(l: Level) -> &'static str {
    match l {
        Level::Full => "everything rush does, used every day",
        Level::Tested => "tried against its real program",
        Level::Preview => "built, not yet tried against its real program",
    }
}

/// Provider k's support level, coloured: full green, tested blue, preview
/// yellow (levelChip).
pub fn level_chip(reg: &Registry, k: &str) -> Span {
    let lv = reg.level_of(k);
    let c = match lv {
        Level::Full => GREEN,
        Level::Tested => BLUE,
        Level::Preview => YELLOW,
    };
    sp(fit_str(lv.as_str(), 8), c)
}

/// The provider id is, whichever way it's paid (provOf).
pub fn prov_of(reg: &Registry, id: &str) -> String {
    reg.billed(id).0
}

/// Whether provider id is paid for with an API key alone (paysByKey).
pub fn pays_by_key(reg: &Registry, id: &str) -> bool {
    let (p, key) = reg.billed(id);
    key || !reg.split(&p) && !reg.key_env(&p).is_empty()
}

/// How provider id is paid for, where it's one of two ways or only by key
/// (billWord).
pub fn bill_word(reg: &Registry, id: &str) -> &'static str {
    let (p, _) = reg.billed(id);
    if pays_by_key(reg, id) {
        "API key"
    } else if reg.split(&p) {
        "subscription"
    } else {
        ""
    }
}

/// Whose models provider id serves: GitHub Copilot, where the company
/// isn't the agent's name, else the one that is (provName).
pub fn prov_name(reg: &Registry, id: &str) -> String {
    let (p, _) = reg.billed(id);
    let name = reg.provider_label(&p);
    let agent = agent_name(reg, &p);
    if !reg.split(&p) && reg.key_env(&p).is_empty() && name != agent {
        if agent.starts_with(&format!("{name} ")) {
            return agent;
        }
        return format!("{name} {agent}");
    }
    name
}

/// Provider id as you'd call it: whose models, and how they're paid for
/// where that's worth saying (provLabel).
pub fn prov_label(reg: &Registry, id: &str) -> String {
    let b = bill_word(reg, id);
    if b.is_empty() {
        prov_name(reg, id)
    } else {
        format!("{} · {b}", prov_name(reg, id))
    }
}

/// The agent provider id runs as: in its own harness, or the one chosen as
/// its default (provKind).
pub fn prov_kind(reg: &Registry, cfg: &rush_state::Config, id: &str) -> Kind {
    match cfg.profile_named(id) {
        Some(p) if !p.providers.is_empty() => Kind::from(p.kind_of(&p.providers[0]).as_str()),
        _ => {
            let _ = reg;
            Kind::from(id)
        }
    }
}

/// A profile's installed providers, glyph and name, in order: an arrow
/// between them when new sessions move on, "only" when they stay (chain).
pub fn chain(reg: &Registry, p: &Profile) -> Line {
    let inst = p.installed();
    if inst.is_empty() {
        return line([sp("none of its providers is installed", YELLOW)]);
    }
    let tag = |k: &str| -> Vec<Span> {
        let pr = reg.provider_of(k);
        let l = look_of(reg, &pr);
        vec![
            sp(
                format!("{} {}", l.glyph, prov_label(reg, &p.id(&pr))),
                l.colour(),
            ),
            sp(runs_in_words(reg, k), DIM),
        ]
    };
    if !p.mixes() || inst.len() == 1 {
        let mut l = line(tag(&inst[0]));
        l.spans.push(sp(" only", DIM));
        return l;
    }
    let mut l = Line::new();
    for (i, k) in inst.iter().enumerate() {
        if i > 0 {
            l.spans.push(sp(" → ", FAINT));
        }
        l.spans.extend(tag(k));
    }
    l
}

/// The providers rush has an adapter for that aren't installed here, each
/// with its support level (notInstalled).
pub fn not_installed(reg: &Registry) -> Line {
    let mut l = Line::new();
    for a in reg.all().iter() {
        let k = a.kind();
        if reg.current_kind(k.as_str()) != k || reg.installed(k.as_str()) {
            continue;
        }
        if !l.spans.is_empty() {
            l.spans.push(sp(" · ", FAINT));
        }
        l.spans.push(glyph(reg, k.as_str()));
        l.spans.push(sp(format!(" {}", a.name()), DIM));
        l.spans
            .push(sp(format!(" {}", reg.level_of(k.as_str()).as_str()), FAINT));
    }
    l
}
