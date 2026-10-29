//! A project's memory as Projects shows it: its notes by kind, and how
//! many are worth a look, on a row enter reviews. Go: internal/ui/
//! projectmem.go (memSummary :15, memSummaryOf :29, projectMemory :72).
//! Read off the UI, again at most every half minute while the place
//! shows.

use std::collections::BTreeMap;

use rush_core::adapter::{Feature, Profile, Registry, migrated};
use rush_core::text::Line;
use rush_core::time::Ms;

use super::data::*;
use super::memory::mem_type_colour;
use super::rows::*;
use crate::model::Model;
use crate::style::*;

/// How long a project's memory, as read, stands.
const MEM_EVERY: Ms = 30_000;

/// A project's memory as Projects shows it (memSummary).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MemSummary {
    pub p: Profile,
    pub root: String,
    pub notes: usize,
    pub kinds: BTreeMap<String, usize>,
    pub review: usize,
    pub at: Ms,
}

/// The profile a project's memory is read through: the first of its
/// agents' whose agent keeps memory rush can read, else Claude Code's
/// first (memSummaryOf's account).
fn mem_profile(reg: &Registry, agents: &[(String, String)]) -> Option<Profile> {
    for (kind, account) in agents {
        let k = migrated(kind);
        let Some(ad) = reg.get(k.as_str()) else {
            continue;
        };
        if ad.memory_reader().is_none() || !reg.supports(k.as_str(), Feature::Memory) {
            continue;
        }
        let ps = reg.profiles_of(&ad);
        if let Some(p) = ps.iter().find(|p| &p.name == account && !p.dir.is_empty()) {
            return Some(p.clone());
        }
    }
    let ad = reg.get("claude")?;
    reg.profiles_of(&ad).into_iter().next()
}

/// Reads the memory of each repository on the page that's due, off the
/// UI: at most every half minute (memSummaryOf).
pub(crate) fn refresh_memory(m: &mut Model) {
    let now = m.now;
    let due: Vec<(String, Vec<(String, String)>)> = {
        let d = data(m);
        projects(m)
            .into_iter()
            .filter(|p| p.repo && p.key.starts_with('/'))
            .filter(|p| !d.mem_reading.contains(&p.key))
            .filter(|p| {
                d.mem_counts
                    .get(&p.key)
                    .is_none_or(|s| now - s.at >= MEM_EVERY)
            })
            .map(|p| {
                let ags = p
                    .agents
                    .iter()
                    .map(|g| (g.a.kind.to_string(), g.a.job.account.clone()))
                    .collect();
                (p.key, ags)
            })
            .collect()
    };
    for (root, ags) in due {
        m.kept::<Data>().mem_reading.insert(root.clone());
        let reg = m.reg.clone();
        m.later(
            move || read_summary(&reg, &ags, &root).map_err(|_| root),
            |m, r| {
                let d = m.kept::<Data>();
                match r {
                    Ok(s) => {
                        d.mem_reading.remove(&s.root);
                        let changed = d.mem_counts.get(&s.root).is_none_or(|o| {
                            (o.notes, &o.kinds, o.review) != (s.notes, &s.kinds, s.review)
                        });
                        d.mem_counts.insert(s.root.clone(), s);
                        if changed {
                            crate::frame::Out::Draw
                        } else {
                            crate::frame::Out::Idle
                        }
                    }
                    Err(root) => {
                        d.mem_reading.remove(&root);
                        crate::frame::Out::Idle
                    }
                }
            },
        );
    }
}

/// Reads root's memory, and judges which notes are worth a look. It reads
/// the disk.
fn read_summary(reg: &Registry, ags: &[(String, String)], root: &str) -> Result<MemSummary, ()> {
    let p = mem_profile(reg, ags).ok_or(())?;
    let ad = reg.get(p.kind.as_str()).ok_or(())?;
    let mr = ad.memory_reader().ok_or(())?;
    let mem = mr.memory(&p, root, "");
    let at = rush_core::time::now();
    let mut out = MemSummary {
        p,
        root: root.to_string(),
        at,
        ..MemSummary::default()
    };
    for f in &mem.files {
        if f.group != "Auto memory" || f.name == "MEMORY.md" || f.missing {
            continue;
        }
        out.notes += 1;
        let k = if f.kind.is_empty() {
            "untyped"
        } else {
            &f.kind
        };
        *out.kinds.entry(k.to_string()).or_default() += 1;
        if !rush_memory::why(f, at).is_empty() {
            out.review += 1;
        }
    }
    Ok(out)
}

/// A project's Memory section: how many notes of each kind, and how many
/// to review, on a row enter reviews (projectMemory).
pub(crate) fn project_memory(m: &Model, p: &Project, w: usize, rows: &mut Vec<WorkRow>) {
    if !p.key.starts_with('/') || !p.repo {
        return;
    }
    let Some(s) = data(m).mem_counts.get(&p.key) else {
        return;
    };
    if s.notes == 0 {
        return;
    }
    let text = |l: Line| WorkRow {
        line: l,
        owner: p.key.clone(),
        ..WorkRow::default()
    };
    rows.push(text(Line::new()));
    let meta = match s.notes {
        1 => "1 note".to_string(),
        n => format!("{n} notes"),
    };
    rows.push(text(psection("Memory", &meta, w)));
    let mut l = ln([plain(" ")]);
    let mut first = true;
    for k in ["user", "feedback", "project", "reference", "untyped"] {
        if let Some(n) = s.kinds.get(k).filter(|n| **n > 0) {
            if !first {
                l.spans.push(sp(" · ", DIM));
            }
            first = false;
            l.spans.push(sp(format!("{n} {k}"), mem_type_colour(k)));
        }
    }
    l.spans.push(plain("   "));
    if s.review > 0 {
        l.spans
            .push(sp(format!("! {} to review", s.review), YELLOW));
        l.spans.push(sp(" · enter", DIM));
    } else {
        l.spans.push(sp("✓ nothing to review", DIM));
    }
    rows.push(WorkRow {
        id: format!("m{}", p.key),
        line: l,
        what: What::Mem(Box::new(s.clone())),
        owner: p.key.clone(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Projects shows a project's memory by kind and how many to review,
    /// on a row picked to review it (TestProjectMemoryRow).
    #[test]
    fn project_memory_row() {
        let mut m = Model::new("t");
        let key = "/src/app";
        let now = m.now;
        m.kept::<Data>().mem_counts.insert(
            key.into(),
            MemSummary {
                root: key.into(),
                notes: 3,
                kinds: [("feedback".into(), 2), ("project".into(), 1)].into(),
                review: 2,
                at: now,
                ..MemSummary::default()
            },
        );
        let p = Project {
            key: key.into(),
            title: "app".into(),
            agents: Vec::new(),
            open: false,
            repo: true,
        };
        let mut rows = Vec::new();
        project_memory(&m, &p, 100, &mut rows);
        let text: Vec<String> = rows.iter().map(|r| r.line.plain()).collect();
        let text = text.join("\n");
        for want in ["MEMORY  3 notes", "2 feedback · 1 project", "2 to review"] {
            assert!(text.contains(want), "rows lack {want:?}:\n{text}");
        }
        let last = rows.last().unwrap();
        assert!(last.pickable() && matches!(last.what, What::Mem(_)));
    }
}
