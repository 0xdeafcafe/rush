//! A plugin with the sidebar capability may arrange rush's agent list: its
//! own sections, and a name for each agent it knows. It only labels agents
//! rush already shows, so it grants the plugin nothing it could not see.
//! The broker checks what it sends and writes it to sidebar_path, a file
//! the plugin can't reach; the UI reads it from there.
//! Ported from internal/plugin/sidebar.go.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::SystemTime;

use rush_state::Time;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::jsonshape::{self, Shape};
use crate::manifest::name_ok;
use crate::plugin::{Approval, approvals};
use crate::text::{clean_label, go_q};

// Limits on a sidebar.
pub const MAX_SIDEBAR_SECTIONS: usize = 32;
pub const MAX_SIDEBAR_AGENTS: usize = 2000;
const MAX_SIDEBAR_TITLE: usize = 64;
const MAX_SIDEBAR_NAME: usize = 200;

/// How a plugin arranges the agent list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidebar {
    pub plugin: String,
    pub title: String,
    #[serde(serialize_with = "nil_if_empty")]
    pub sections: Vec<SidebarSection>,
    /// By Claude Code session id; None where Go's map is nil.
    pub agents: Option<BTreeMap<String, SidebarAgent>>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Time,
}

fn nil_if_empty<S: serde::Serializer, T: Serialize>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        return s.serialize_none();
    }
    v.serialize(s)
}

/// One section of the list, in the order given.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarSection {
    pub title: String,
}

/// Places one agent, known by its Claude Code session id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarAgent {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub section: String,
    pub order: i64,
}

impl Sidebar {
    /// Whether the sidebar arranges nothing.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// What the list's group-by mode is called: the title, or the plugin's
    /// name.
    pub fn label(&self) -> &str {
        if self.title.is_empty() {
            &self.plugin
        } else {
            &self.title
        }
    }

    /// Validates and cleans s in place.
    fn check(&mut self) -> Result<(), String> {
        if self.sections.is_empty() {
            self.title.clear();
            self.sections = Vec::new();
            self.agents = None;
            return Ok(());
        }
        self.title = clean_label("title", &self.title, MAX_SIDEBAR_TITLE)?;
        if self.sections.len() > MAX_SIDEBAR_SECTIONS {
            return Err(format!(
                "{} sections: at most {MAX_SIDEBAR_SECTIONS}",
                self.sections.len()
            ));
        }
        let mut seen = HashSet::new();
        for s in &mut self.sections {
            let t = clean_label("section title", &s.title, MAX_SIDEBAR_TITLE)?;
            if t.is_empty() {
                return Err("a section has no title".into());
            }
            if !seen.insert(t.clone()) {
                return Err(format!("section {} appears twice", go_q(&t)));
            }
            s.title = t;
        }
        if let Some(agents) = &mut self.agents {
            if agents.len() > MAX_SIDEBAR_AGENTS {
                return Err(format!(
                    "{} agents: at most {MAX_SIDEBAR_AGENTS}",
                    agents.len()
                ));
            }
            for (id, a) in agents.iter_mut() {
                if !sidebar_id_ok(id) {
                    return Err(format!("agent id {} is not a session id", go_q(id)));
                }
                a.name = clean_label("agent name", &a.name, MAX_SIDEBAR_NAME)?;
                a.section = clean_label("section", &a.section, MAX_SIDEBAR_TITLE)?;
                if !seen.contains(&a.section) {
                    return Err(format!(
                        "agent {id} is in section {}, which isn't in sections",
                        go_q(&a.section)
                    ));
                }
            }
        }
        Ok(())
    }
}

/// ^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$: a Claude Code session id, or
/// anything as safe.
fn sidebar_id_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
}

const SECTION_SHAPE: Shape = Shape::Struct("plugin.SidebarSection", &[("title", Shape::Any)]);
const AGENT_SHAPE: Shape = Shape::Struct(
    "plugin.SidebarAgent",
    &[
        ("name", Shape::Any),
        ("section", Shape::Any),
        ("order", Shape::Any),
    ],
);
const PARAMS_SHAPE: Shape = Shape::Struct(
    "struct { Title string \"json:\\\"title\\\"\"; Sections []plugin.SidebarSection \"json:\\\"sections\\\"\"; Agents map[string]plugin.SidebarAgent \"json:\\\"agents\\\"\" }",
    &[
        ("title", Shape::Any),
        ("sections", Shape::Seq(&SECTION_SHAPE)),
        ("agents", Shape::Map(&AGENT_SHAPE)),
    ],
);
const SIDEBAR_SHAPE: Shape = Shape::Struct(
    "plugin.Sidebar",
    &[
        ("plugin", Shape::Any),
        ("title", Shape::Any),
        ("sections", Shape::Seq(&SECTION_SHAPE)),
        ("agents", Shape::Map(&AGENT_SHAPE)),
        ("updatedAt", Shape::Any),
    ],
);

/// Reads and checks what a plugin sent with sidebar.set, cleaning every
/// string of what a terminal would act on. Empty params, or no sections,
/// clear it.
pub fn parse_sidebar(plugin: &str, params: Option<&RawValue>) -> Result<Sidebar, String> {
    let mut s = Sidebar {
        plugin: plugin.to_string(),
        ..Default::default()
    };
    let p = params.map_or("", |p| p.get()).trim();
    if !p.is_empty() && p != "null" {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct In {
            title: String,
            sections: Vec<SidebarSection>,
            agents: Option<BTreeMap<String, SidebarAgent>>,
        }
        let i: In = jsonshape::decode(p.as_bytes(), &PARAMS_SHAPE, true)?;
        s.title = i.title;
        s.sections = i.sections;
        s.agents = i.agents;
    }
    s.check()?;
    Ok(s)
}

/// Holds the sidebars plugins set, one file each. It is apart from the
/// plugins' own folders and data folders, so no plugin writes it.
pub fn sidebar_root() -> PathBuf {
    rush_state::dir().join("plugin-sidebar")
}

/// Where a plugin's sidebar is kept.
pub fn sidebar_path(name: &str) -> PathBuf {
    sidebar_root().join(format!("{name}.json"))
}

/// Keeps a plugin's sidebar, or removes it when it's empty.
pub fn save_sidebar(mut s: Sidebar) -> Result<(), String> {
    if !name_ok(&s.plugin) {
        return Err(format!("plugin name {}", go_q(&s.plugin)));
    }
    if s.is_empty() {
        return remove_sidebar(&s.plugin);
    }
    if s.updated_at.is_zero() {
        s.updated_at = Time::now();
    }
    let root = sidebar_root();
    crate::mkdir_all(&root)?;
    let b = rush_state::json::to_vec(&s).map_err(|e| e.to_string())?;
    let tmp = root.join(format!(".{}-{}.tmp", s.plugin, crate::uisettings::unique()));
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&b)?;
        drop(f);
        std::fs::rename(&tmp, sidebar_path(&s.plugin))
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::gopath::errno_text(&e));
    }
    Ok(())
}

/// Removes a plugin's sidebar, if it has one.
pub fn remove_sidebar(name: &str) -> Result<(), String> {
    if !name_ok(name) {
        return Ok(());
    }
    let p = sidebar_path(name);
    match std::fs::remove_file(&p) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(crate::gopath::os_err("remove", &p, &e))
        }
        _ => Ok(()),
    }
}

/// Removes the sidebars of plugins that aren't approved, or were approved
/// without the capability.
pub fn prune_sidebars(approved: &BTreeMap<String, Approval>) {
    for (name, _) in crate::plugin::read_dir_sorted(&sidebar_root()) {
        let Some(n) = name.strip_suffix(".json") else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if !approved.get(n).is_some_and(|a| a.manifest.sidebar) {
            let _ = remove_sidebar(n);
        }
    }
}

/// Reads the sidebars of approved plugins with the capability, cheaply
/// enough to call every second: a file is read again only when its
/// modification time or size changes. One that doesn't parse is left out.
#[derive(Default)]
pub struct Sidebars {
    approved_mod: Option<SystemTime>,
    approved: Option<BTreeMap<String, Approval>>,
    files: BTreeMap<String, SidebarFile>,
}

struct SidebarFile {
    modified: Option<SystemTime>,
    size: u64,
    s: Option<Sidebar>,
}

impl Sidebars {
    /// Every sidebar in use, by plugin name order.
    pub fn load(&mut self) -> Vec<Sidebar> {
        match std::fs::metadata(crate::root().join("approved.json")) {
            Err(_) => {
                self.approved = None;
                self.approved_mod = None;
            }
            Ok(md) => {
                let m = md.modified().ok();
                if m != self.approved_mod || self.approved.is_none() {
                    self.approved = Some(approvals());
                    self.approved_mod = m;
                }
            }
        }
        let mut out = Vec::new();
        let Some(approved) = &self.approved else {
            self.files.clear();
            return out;
        };
        self.files.retain(|n, _| approved.contains_key(n));
        for (name, a) in approved {
            if !a.manifest.sidebar {
                self.files.remove(name);
                continue;
            }
            let Ok(md) = std::fs::metadata(sidebar_path(name)) else {
                self.files.remove(name);
                continue;
            };
            let (m, size) = (md.modified().ok(), md.len());
            let f = self.files.entry(name.clone()).or_insert(SidebarFile {
                modified: None,
                size: u64::MAX,
                s: None,
            });
            if f.modified != m || f.size != size {
                *f = SidebarFile {
                    modified: m,
                    size,
                    s: read_sidebar(name),
                };
            }
            if let Some(s) = &f.s
                && !s.is_empty()
            {
                out.push(s.clone());
            }
        }
        out
    }
}

fn read_sidebar(name: &str) -> Option<Sidebar> {
    let b = std::fs::read(sidebar_path(name)).ok()?;
    if b.len() > 8 << 20 {
        return None;
    }
    let mut s: Sidebar = jsonshape::decode(&b, &SIDEBAR_SHAPE, false).ok()?;
    if s.plugin != name || s.check().is_err() {
        return None;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::tests::{Home, install_plugin, manifest};
    use crate::plugin::{Plugin, approve, revoke};
    use std::os::unix::fs::PermissionsExt;

    const UUID: &str = "8c76706f-1c00-4aed-9c6d-7509f3033943";

    fn raw(s: &str) -> Box<RawValue> {
        RawValue::from_string(s.to_string()).unwrap()
    }

    fn parse(s: &str) -> Result<Sidebar, String> {
        if s.is_empty() {
            return parse_sidebar("kanban", None);
        }
        parse_sidebar("kanban", Some(&raw(s)))
    }

    #[test]
    fn parse_sidebar_cleans_and_checks() {
        let s = parse(&format!(
            r#"{{"title": "Kan\u001b[31mban",
            "sections": [{{"title": "In Progress"}}, {{"title": "Wait{rlo}ing\n"}}],
            "agents": {{"{UUID}": {{"name": "Fix \u001b]0;pwned\u0007login\tbug", "section": "In Progress", "order": 2}}}}}}"#,
            rlo = "\u{202e}"
        ))
        .unwrap();
        assert!(
            s.plugin == "kanban" && s.title == "Kanban" && s.sections[1].title == "Waiting",
            "not cleaned: {s:?}"
        );
        let a = &s.agents.as_ref().unwrap()[UUID];
        assert!(a.name == "Fix login bug" && a.order == 2, "agent = {a:?}");

        for clear in ["", "null", "{}", r#"{"sections": []}"#] {
            let s = parse(clear).unwrap();
            assert!(s.is_empty(), "{clear:?}: {s:?}");
        }

        let many: Vec<SidebarSection> = (0..=MAX_SIDEBAR_SECTIONS)
            .map(|i| SidebarSection {
                title: "s".repeat(i + 1),
            })
            .collect();
        let many = serde_json::json!({ "sections": many }).to_string();
        let agents: BTreeMap<String, SidebarAgent> = (0..=MAX_SIDEBAR_AGENTS)
            .map(|i| {
                (
                    format!("a{i}"),
                    SidebarAgent {
                        section: "A".into(),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let agents =
            serde_json::json!({ "sections": [{"title": "A"}], "agents": agents }).to_string();
        let long_title = format!(
            r#"{{"title": "{}", "sections": [{{"title": "A"}}]}}"#,
            "t".repeat(65)
        );
        let long_section = format!(r#"{{"sections": [{{"title": "{}"}}]}}"#, "t".repeat(65));
        let long_name = format!(
            r#"{{"sections": [{{"title": "A"}}], "agents": {{"s1": {{"name": "{}", "section": "A"}}}}}}"#,
            "n".repeat(201)
        );
        let bad = [
            ("too many sections", many.as_str()),
            ("too many agents", agents.as_str()),
            ("long title", long_title.as_str()),
            ("long section", long_section.as_str()),
            ("long name", long_name.as_str()),
            (
                "id as a path",
                r#"{"sections": [{"title": "A"}], "agents": {"../x": {"section": "A"}}}"#,
            ),
            (
                "unknown section",
                r#"{"sections": [{"title": "A"}], "agents": {"s1": {"section": "B"}}}"#,
            ),
            ("twice", r#"{"sections": [{"title": "A"}, {"title": "A"}]}"#),
            ("untitled", r#"{"sections": [{"title": "\u001b[0m"}]}"#),
            (
                "unknown field",
                r#"{"sections": [{"title": "A"}], "color": "red"}"#,
            ),
        ];
        for (name, params) in bad {
            assert!(parse(params).is_err(), "{name}: accepted");
        }
    }

    fn approve_kanban(sidebar: bool) {
        let mut m = manifest("kanban", "kanban");
        m.sidebar = sidebar;
        let dir = install_plugin(&m, &[]);
        approve(&Plugin {
            manifest: m,
            dir: crate::gopath::s(&dir),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn sidebars_follow_approvals() {
        let _h = Home::new();
        approve_kanban(true);
        let mut s = parse(r#"{"title": "Kanban", "sections": [{"title": "A"}], "agents": {"s1": {"name": "one", "section": "A"}}}"#).unwrap();
        save_sidebar(s.clone()).unwrap();
        let md = std::fs::metadata(sidebar_path("kanban")).unwrap();
        assert_eq!(md.permissions().mode() & 0o777, 0o600);
        let d = std::fs::metadata(sidebar_root()).unwrap();
        assert_eq!(d.permissions().mode() & 0o777, 0o700);

        let mut w = Sidebars::default();
        let got = w.load();
        assert!(
            got.len() == 1
                && got[0].label() == "Kanban"
                && got[0].agents.as_ref().unwrap()["s1"].name == "one"
                && !got[0].updated_at.is_zero(),
            "load = {got:?}"
        );

        // Changed on disk, it's read again; broken, it's left out. The new
        // name is longer, so the size tells it changed whatever the clock.
        s.agents.as_mut().unwrap().insert(
            "s1".into(),
            SidebarAgent {
                name: "two two".into(),
                section: "A".into(),
                order: 0,
            },
        );
        save_sidebar(s.clone()).unwrap();
        let got = w.load();
        assert!(
            got.len() == 1 && got[0].agents.as_ref().unwrap()["s1"].name == "two two",
            "after a change = {got:?}"
        );
        std::fs::write(sidebar_path("kanban"), r#"{"plugin": "kanban", "sections": [{"title": "A"}], "agents": {"s1": {"section": "Z"}}}"#).unwrap();
        assert!(w.load().is_empty(), "a malformed file was used");

        // A file under another plugin's name, not approved, is ignored, and
        // pruned.
        save_sidebar(Sidebar {
            plugin: "other".into(),
            sections: vec![SidebarSection { title: "X".into() }],
            ..Default::default()
        })
        .unwrap();
        assert!(
            w.load().is_empty(),
            "an unapproved plugin's sidebar was used"
        );
        prune_sidebars(&approvals());
        assert!(
            !sidebar_path("other").exists(),
            "prune left an unapproved plugin's sidebar"
        );

        // Clearing removes the file; so does revoking.
        save_sidebar(s.clone()).unwrap();
        save_sidebar(Sidebar {
            plugin: "kanban".into(),
            ..Default::default()
        })
        .unwrap();
        assert!(
            !sidebar_path("kanban").exists(),
            "an empty sidebar left its file"
        );
        save_sidebar(s).unwrap();
        revoke("kanban").unwrap();
        assert!(
            !sidebar_path("kanban").exists(),
            "revoking left the sidebar"
        );
        assert!(w.load().is_empty(), "after revoke");
    }

    #[test]
    fn sidebar_needs_the_capability() {
        let _h = Home::new();
        approve_kanban(false);
        save_sidebar(Sidebar {
            plugin: "kanban".into(),
            sections: vec![SidebarSection { title: "A".into() }],
            ..Default::default()
        })
        .unwrap();
        assert!(
            Sidebars::default().load().is_empty(),
            "a plugin approved without the sidebar arranged the list"
        );
    }

    #[test]
    fn file_bytes_are_gos() {
        let _h = Home::new();
        let s = Sidebar {
            plugin: "kanban".into(),
            title: "K".into(),
            sections: vec![SidebarSection { title: "A".into() }],
            agents: None,
            updated_at: Time::parse("2026-10-29T10:00:00.5+01:00").unwrap(),
        };
        save_sidebar(s).unwrap();
        assert_eq!(
            std::fs::read_to_string(sidebar_path("kanban")).unwrap(),
            r#"{"plugin":"kanban","title":"K","sections":[{"title":"A"}],"agents":null,"updatedAt":"2026-10-29T10:00:00.5+01:00"}"#
        );
    }
}
