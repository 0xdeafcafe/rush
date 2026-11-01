//! A repository's address typed in the Agents box: what it's read as,
//! the clone it's matched to by remote, where a clone would go, and that
//! nothing moves or is cloned without tab. Temp dirs only.

use super::gitlink::{self, Offer, Repo, find, layout_base, offer_for, parse};
use super::tests::model_of;
use crate::frame::Press;

fn repo(host: &str, owner: &str, name: &str, ssh: bool) -> Repo {
    Repo {
        host: host.into(),
        owner: owner.into(),
        name: name.into(),
        ssh,
    }
}

/// Every way an address is written, and words that aren't one.
#[test]
fn addresses_are_read() {
    let gh = |o: &str, n: &str, ssh| Some(repo("github.com", o, n, ssh));
    let table: &[(&str, Option<Repo>)] = &[
        (
            "https://github.com/acme/widgets",
            gh("acme", "widgets", false),
        ),
        (
            "https://github.com/acme/widgets.git",
            gh("acme", "widgets", false),
        ),
        (
            "http://github.com/acme/widgets/",
            gh("acme", "widgets", false),
        ),
        ("github.com/acme/widgets", gh("acme", "widgets", false)),
        ("GitHub.com/acme/widgets", gh("acme", "widgets", false)),
        (
            "git@github.com:acme/widgets.git",
            gh("acme", "widgets", true),
        ),
        ("git@github.com:acme/widgets", gh("acme", "widgets", true)),
        (
            "ssh://git@github.com/acme/widgets.git",
            gh("acme", "widgets", true),
        ),
        (
            "ssh://git@github.com:22/acme/widgets",
            gh("acme", "widgets", true),
        ),
        (
            "https://github.com/acme/widgets/tree/feat/x",
            gh("acme", "widgets", false),
        ),
        (
            "https://github.com/acme/widgets/pull/12",
            gh("acme", "widgets", false),
        ),
        (
            "github.com/acme/widgets/pull/12/files",
            gh("acme", "widgets", false),
        ),
        (
            "https://github.com/acme/widgets#readme",
            gh("acme", "widgets", false),
        ),
        (
            "(https://github.com/acme/widgets).",
            gh("acme", "widgets", false),
        ),
        (
            "https://gitlab.com/team/app/-/tree/main",
            Some(repo("gitlab.com", "team", "app", false)),
        ),
        (
            "git@git.example.org:ops/infra.git",
            Some(repo("git.example.org", "ops", "infra", true)),
        ),
        (
            "https://git.example.org/ops/infra.git",
            Some(repo("git.example.org", "ops", "infra", false)),
        ),
        (
            "codeberg.org/ann/tool",
            Some(repo("codeberg.org", "ann", "tool", false)),
        ),
        // Not an address.
        ("https://docs.rs/serde/latest", None),
        ("https://github.com/acme", None),
        ("https://github.com/orgs/acme/people", None),
        ("src/main.rs", None),
        ("crates/rush-ui/src", None),
        ("me@example.com", None),
        ("http://localhost:3000/api/v1", None),
        ("example.org/a/b", None),
        ("ftp://github.com/acme/widgets", None),
    ];
    for (w, want) in table {
        assert_eq!(&parse(w, &[]), want, "{w}");
    }
    // A host the projects here come from counts without https://.
    assert_eq!(
        parse("git.example.org/ops/infra", &["git.example.org".into()]),
        Some(repo("git.example.org", "ops", "infra", false))
    );
    assert_eq!(
        find("fix the bug in github.com/acme/widgets/pull/3 please", &[]),
        gh("acme", "widgets", false)
    );
    assert_eq!(find("fix the bug in src/lib.rs", &[]), None);
}

/// The clone is found by its remote, whatever its folder's called; with
/// none, a clone goes where the others are, the way they were cloned.
#[test]
fn clone_found_by_remote() {
    let widgets = repo("github.com", "acme", "widgets", true);
    let gadgets = repo("github.com", "acme", "gadgets", true);
    let projects = vec![
        (
            "/u/Source/github.com/acme/widgets".to_string(),
            vec![widgets.clone()],
        ),
        ("/u/work/renamed".to_string(), vec![gadgets.clone()]),
    ];
    assert_eq!(layout_base(&projects).as_deref(), Some("/u/Source"));
    let none = |_: &str| None;
    let typed = repo("github.com", "Acme", "Gadgets", false);
    assert_eq!(
        offer_for(
            &typed,
            &projects,
            "/u/Source/github.com/acme/widgets",
            "/u",
            none
        ),
        Some(Offer::Move {
            repo: typed.clone(),
            dir: "/u/work/renamed".into()
        })
    );
    // Already there: nothing to offer.
    assert_eq!(
        offer_for(&typed, &projects, "/u/work/renamed", "/u", none),
        None
    );
    // None here: cloned beside the others, over ssh as they were.
    let new = repo("github.com", "acme", "newthing", false);
    assert_eq!(
        offer_for(&new, &projects, "/u/work/renamed", "/u", none),
        Some(Offer::Clone {
            repo: new.clone(),
            url: "git@github.com:acme/newthing.git".into(),
            dir: "/u/Source/github.com/acme/newthing".into()
        })
    );
    // Where it would go is taken by something else: no offer.
    assert_eq!(
        offer_for(&new, &projects, "/u/work/renamed", "/u", |_| Some(
            Vec::new()
        )),
        None
    );
    // No layout to go by: ~/Source, https.
    assert_eq!(
        offer_for(&new, &[], "", "/u", none),
        Some(Offer::Clone {
            repo: new.clone(),
            url: "https://github.com/acme/newthing.git".into(),
            dir: "/u/Source/github.com/acme/newthing".into()
        })
    );
}

fn checkout(dir: &std::path::Path, url: &str) {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".git/config"),
        format!(
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[submodule \"x\"]\n\turl = https://github.com/other/sub\n"
        ),
    )
    .unwrap();
}

/// Typed in the box, an address of a clone here offers it on the box's
/// edge; only tab moves the box there, the address staying in the task.
/// One with no clone offers to clone it, and nothing is made without tab.
#[test]
fn offer_needs_the_key() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("Source");
    let widgets = base.join("github.com/acme/widgets");
    let renamed = tmp.path().join("work/renamed");
    checkout(&widgets, "git@github.com:acme/widgets.git");
    checkout(&renamed, "git@github.com:acme/gadgets.git");
    let (w, r) = (
        widgets.to_string_lossy().to_string(),
        renamed.to_string_lossy().to_string(),
    );
    // Temp folders aren't where agents' projects come from: these are
    // known by a setting of their own, the box opened in widgets.
    let mut m = model_of(Vec::new(), 160, 40);
    m.launch_dir = w.clone();
    for p in [&w, &r] {
        crate::accounts::cfg_mut(&mut m)
            .own_worktrees
            .insert(p.clone(), true);
    }
    m.cfg.list_only = true;
    let start = crate::startdir::start_dir(&m);
    let text = "review github.com/acme/gadgets/pull/12";
    m.input.set(text);
    let f: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    let foot = f
        .iter()
        .find(|l| l.contains("tab moves there"))
        .unwrap_or_else(|| panic!("no offer on the box: {f:#?}"));
    assert!(foot.contains("renamed"), "{foot}");
    assert!(f.iter().any(|l| l.contains("tab move there")), "{f:#?}");
    assert_eq!(crate::startdir::start_dir(&m), start, "not moved yet");
    m.press_key(&Press::of("tab").unwrap());
    assert_eq!(crate::startdir::start_dir(&m), r);
    assert_eq!(m.input.string(), text, "the address stays in the task");
    // Moved: nothing more to offer.
    assert_eq!(gitlink::offer(&mut m), None);

    // No clone here: offered, and nothing made without tab.
    m.input.set("port github.com/acme/newthing to rust");
    let want = base.join("github.com/acme/newthing");
    match gitlink::offer(&mut m) {
        Some(Offer::Clone { dir, url, .. }) => {
            assert_eq!(dir, want.to_string_lossy());
            assert_eq!(
                url, "git@github.com:acme/newthing.git",
                "as the others were"
            );
        }
        o => panic!("{o:?}"),
    }
    let f: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(
        f.iter()
            .any(|l| l.contains("newthing isn't here · tab clones it to")),
        "{f:#?}"
    );
    assert!(!want.exists(), "nothing cloned without tab");
    assert!(!base.join("github.com/acme").join("newthing").exists());
}

/// The own worktree chip shows on the box's edge in a repository, and a
/// click switches it for that project.
#[test]
fn worktree_chip_switches() {
    use super::prompt::{EdgeAct, edge_hits};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("app");
    checkout(&dir, "git@github.com:acme/app.git");
    let d = dir.to_string_lossy().to_string();
    let mut m = model_of(Vec::new(), 160, 40);
    m.launch_dir = d.clone();
    m.cfg.list_only = true;
    let f: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    let (y, hits) = edge_hits(&mut m);
    assert!(f[y].contains(" own worktree "), "{:?}", f[y]);
    let (x0, _, _) = hits
        .iter()
        .find(|h| matches!(&h.2, EdgeAct::Worktree(p) if *p == d))
        .cloned()
        .expect("the chip takes a click");
    m.mouse_event(&crate::frame::Mouse {
        kind: rush_draw::MouseKind::Down(rush_draw::Button::Left),
        x: x0 as u16 + 1,
        y: y as u16,
        mods: rush_draw::KeyMods::NONE,
    });
    assert!(!crate::accounts::cfg(&m).own_worktree(&d));
    let f: Vec<String> = m.frame().iter().map(|l| l.plain()).collect();
    assert!(f[y].contains(" in the checkout "), "{:?}", f[y]);
}
