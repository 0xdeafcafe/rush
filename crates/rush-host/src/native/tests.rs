use super::*;
use crate::agents::{Runner, picks_of};
use rush_core::adapter::Choice;
use rush_core::adapter::others::{CHEAP, LONG_CONTEXT, VISION};

fn r(k: &str, models: &[(&str, &str)], ready: bool) -> Runner {
    Runner {
        k: k.into(),
        prov: k.into(),
        name: k.into(),
        models: models
            .iter()
            .map(|(id, note)| Choice {
                id: (*id).into(),
                note: (*note).into(),
                ..Default::default()
            })
            .collect(),
        ready,
        ..Default::default()
    }
}

/// Claude Code's own, Codex's and Kimi's models, as a session of kind
/// sees them.
fn offered(kind: &str, ready: bool) -> Vec<Offer> {
    let ps = picks_of(&[
        r(
            "claude",
            &[
                ("opus", "The deepest thinker"),
                ("haiku", "Fastest and cheapest"),
            ],
            ready,
        ),
        r(
            "codex",
            &[
                ("gpt-6-astra", "Frontier intelligence"),
                ("gpt-6.2-sol", ""),
                ("gpt-6-luna", ""),
            ],
            ready,
        ),
        r("kimi", &[("kimi-code/k3", "K3")], ready),
    ]);
    offers_of(&claude_reg(), &Settings::default(), kind, ps)
}

/// A registry that knows Claude Code (whose own models are its own
/// subagent types), and no other.
fn claude_reg() -> Registry {
    let reg = Registry::new();
    reg.register(rush_claude::Claude::at(rush_claude::Account::home()));
    reg
}

fn cand(
    name: &str,
    provider: &str,
    tier: Tier,
    traits: &[&'static str],
    ready: bool,
    quota: bool,
) -> Candidate {
    Candidate {
        name: name.into(),
        provider: provider.into(),
        place: Placement {
            tier,
            traits: traits.to_vec(),
        },
        ready,
        quota,
    }
}

/// A tier runs on a model of that tier, with the traits asked, that's
/// signed in and has quota, of the provider fewest of the session's
/// agents are at work on, the first listed among equals.
#[test]
fn a_tier_picks_a_ready_model_spreading_across_providers() {
    let cands = [
        cand("opus", "claude", Tier::Top, &[VISION], true, true),
        cand("astra", "codex", Tier::Top, &[VISION], true, true),
        cand(
            "fable",
            "claude",
            Tier::Top,
            &[VISION, LONG_CONTEXT],
            true,
            true,
        ),
        cand("haiku", "claude", Tier::Quick, &[CHEAP], true, false),
        cand("luna", "codex", Tier::Quick, &[CHEAP], false, true),
        cand(
            "flash",
            "glm",
            Tier::Quick,
            &[CHEAP, LONG_CONTEXT],
            true,
            true,
        ),
    ];
    let busy = |b: &[(&str, usize)]| -> BTreeMap<String, usize> {
        b.iter().map(|(p, n)| (p.to_string(), *n)).collect()
    };
    for (tier, traits, b, want) in [
        (Tier::Top, vec![], busy(&[]), Some("opus")),
        (Tier::Top, vec![], busy(&[("claude", 1)]), Some("astra")),
        (
            Tier::Top,
            vec![],
            busy(&[("claude", 1), ("codex", 1)]),
            Some("opus"),
        ),
        (
            Tier::Top,
            vec![LONG_CONTEXT],
            busy(&[("claude", 3)]),
            Some("fable"),
        ),
        // haiku is out of quota, luna isn't signed in.
        (Tier::Quick, vec![], busy(&[]), Some("flash")),
        (Tier::Quick, vec![VISION], busy(&[]), None),
        (Tier::Workhorse, vec![], busy(&[]), None),
    ] {
        let got = tier_pick(&cands, tier, &traits, &b).map(|i| cands[i].name.as_str());
        assert_eq!(got, want, "{tier:?} {traits:?} {b:?}");
    }
    assert_eq!(
        no_tier(Tier::Quick, &[VISION], &cands),
        "rush has no model in the quick-vision tier here; name a model, or another tier"
    );
    let none = [
        cand("haiku", "claude", Tier::Quick, &[], true, false),
        cand("luna", "codex", Tier::Quick, &[], false, true),
    ];
    assert_eq!(
        no_tier(Tier::Quick, &[], &none),
        "no model in the quick tier can take work now: haiku (out of quota), luna (not signed in). Name another tier or model"
    );
}

/// Calls for one tier made at once each hold their place, so they go to
/// different providers; one let go frees its provider again.
#[test]
fn calls_at_once_spread_across_providers() {
    let cands = [
        cand("opus", "claude", Tier::Top, &[], true, true),
        cand("astra", "codex", Tier::Top, &[], true, true),
    ];
    let parent = "spread-parent";
    let (a, ta) = hold_pick(parent, &cands, Tier::Top, &[]).unwrap();
    let (b, tb) = hold_pick(parent, &cands, Tier::Top, &[]).unwrap();
    assert_eq!(
        (cands[a].name.as_str(), cands[b].name.as_str()),
        ("opus", "astra")
    );
    // Another session's calls don't count against this one's.
    let (o, to) = hold_pick("spread-other", &cands, Tier::Top, &[]).unwrap();
    assert_eq!(cands[o].name, "opus");
    release(ta);
    let (c, tc) = hold_pick(parent, &cands, Tier::Top, &[]).unwrap();
    assert_eq!(cands[c].name, "opus");
    for t in [tb, to, tc] {
        release(t);
    }
}

/// What each harness is offered: the models it doesn't run itself, by
/// their plain names, what they're for, whose and their tier; then the
/// tiers, and a trait where it narrows one. No harness's name in it.
#[test]
fn every_harness_sees_the_others_as_its_own() {
    // Claude Code: its own models are its own types, the rest are rush's.
    let names: Vec<String> = others_of(&offered("claude", true))
        .into_iter()
        .map(|o| o.name)
        .collect();
    assert_eq!(
        names,
        [
            "astra",
            "sol",
            "luna",
            "kimi",
            "top",
            "workhorse",
            "workhorse-vision",
            "quick"
        ]
    );
    // Codex: every model is one of run_agent's, Claude's too.
    let codex = others_of(&offered("codex", true));
    let names: Vec<&str> = codex.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "opus",
            "haiku",
            "astra",
            "sol",
            "luna",
            "kimi",
            "top",
            "workhorse",
            "workhorse-vision",
            "quick"
        ]
    );
    let astra = codex.iter().find(|o| o.name == "astra").unwrap();
    assert_eq!(
        astra.description,
        "Frontier intelligence. codex's gpt-6-astra; top tier, vision."
    );
    let top = codex.iter().find(|o| o.name == "top").unwrap();
    assert_eq!(
        top.description,
        "The top tier, for hard reasoning, design and final review: rush picks one of opus, astra that's signed in with quota left, spreading parallel calls across providers."
    );
    for o in &codex {
        assert!(!o.description.contains(" in Codex"), "{}", o.description);
    }
    // A harness that can't take them as its own: spawn_agent's list.
    let cat = catalogue(&offered("kimi", true));
    assert!(
        cat.starts_with("- opus: The deepest thinker. Anthropic's; top tier, vision.\n"),
        "{cat}"
    );
    assert!(
        cat.ends_with("\nOr name a tier and rush picks a model in it that's signed in with quota left, spreading parallel calls across providers: top (opus, astra), workhorse (sol, kimi), quick (haiku, luna). Add long-context, vision or cheap to narrow it (\"quick long-context\")."),
        "{cat}"
    );
    let cat = catalogue(&offered("claude", true));
    assert!(
        cat.contains("\nYour own models (opus, haiku) aren't listed"),
        "{cat}"
    );
}

/// What's offered is cached word for word by the provider, so it's the
/// same whatever the sign-ins.
#[test]
fn what_is_offered_ignores_sign_ins() {
    for kind in ["claude", "codex", "kimi"] {
        assert_eq!(
            others_of(&offered(kind, true)),
            others_of(&offered(kind, false)),
            "{kind}"
        );
        assert_eq!(
            catalogue(&offered(kind, true)),
            catalogue(&offered(kind, false)),
            "{kind}"
        );
    }
}

/// Settings › Config moves a model to another tier.
#[test]
fn a_model_moved_in_settings_sits_there() {
    let mut cfg = Settings::default();
    cfg.model_tiers.insert("kimi-code/k3".into(), "top".into());
    let ps = picks_of(&[r("kimi", &[("kimi-code/k3", "K3")], true)]);
    let o = offers_of(&claude_reg(), &cfg, "codex", ps);
    assert_eq!(o[0].place.tier, Tier::Top);
}

/// A rush quota reading says whether a plan has room; none read is room,
/// and one account with room is enough.
#[test]
fn quota_is_room_unless_every_reading_is_full() {
    use rush_core::usage::{Quota, Window};
    let now = rush_core::time::now();
    let q = |pct: f64| Quota {
        windows: vec![Window {
            id: "five_hour".into(),
            percent: pct,
            resets_at: now + 3_600_000,
            ..Window::default()
        }],
        fetched_at: now,
        ..Quota::default()
    };
    let rs = |v: &[(&str, f64)]| -> BTreeMap<String, rush_core::usage::Quota> {
        v.iter().map(|(k, p)| (k.to_string(), q(*p))).collect()
    };
    for (readings, want) in [
        (rs(&[]), true),
        (rs(&[("codex:/h", 40.0)]), true),
        (rs(&[("codex:/h", 100.0)]), false),
        (rs(&[("codex:/h", 100.0), ("codex:/h2", 10.0)]), true),
        (rs(&[("claude:/h", 100.0)]), true),
    ] {
        assert_eq!(
            has_quota(&readings, "codex", "codex", now),
            want,
            "{readings:?}"
        );
    }
}
