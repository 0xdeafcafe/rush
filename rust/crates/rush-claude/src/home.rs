//! A login's home: a config folder of its own that rush keeps for it, so a
//! session runs as the login whatever ~/.claude is signed in as; and which
//! login new sessions run as. Port of
//! internal/adapters/claude/claude/home.go and homes.go.
//!
//! Only the sign-in and the state file are a home's own: everything else
//! in it links to ~/.claude, so transcripts, settings, plugins and memory
//! are the same whichever login a session runs as, and a conversation
//! carries on under another login by resuming it there.
//!
//! A sign-in lives in one place only. Claude Code replaces its tokens as
//! it refreshes them and the old ones stop working, so a copy left
//! elsewhere soon signs nothing in: the home takes the vault's sign-in
//! once, and from then on the vault keeps a copy of the home's (keep),
//! never the other way.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use rush_core::time::{Ms, now};
use rush_state::{Config, Login};
use serde_json::json;
use serde_json::value::RawValue;

use crate::account::Account;
use crate::files::{copy_new, mkdir_private, walk_files, write_file_atomic};
use crate::login::{
    delete_creds, indent, read_creds, signed_in_as, the_vault, usable, with_mcp_logins,
    write_creds, write_profile,
};
use crate::oauth;

/// The entries of a home that aren't ~/.claude's.
const OWN_HOME: &[&str] = &[".credentials.json", ".claude.json"];

/// Made in ~/.claude before it's linked to, so a session in a home writes
/// them there rather than into a folder of the home's own.
const SHARED_DIRS: &[&str] = &[
    "projects",
    "todos",
    "plans",
    "shell-snapshots",
    "file-history",
    "session-env",
    "sessions",
];

/// The parts of ~/.claude.json a home keeps up with: the rest (who it's
/// signed in as, its usage) is the home's own.
const SHARED_STATE: &[&str] = &["mcpServers"];

/// Makes home a login's config folder: each of root's entries linked into
/// it, new ones as root gains them. An entry the home has of its own
/// already (a file Claude Code rewrote in place of its link) is left
/// alone.
pub fn link_home(home: &Account, root: &Account) -> io::Result<()> {
    mkdir_private(&home.config_dir)?;
    for d in SHARED_DIRS {
        mkdir_private(&root.config_dir.join(d))?;
        adopt_stray(&home.config_dir.join(d), &root.config_dir.join(d))?;
    }
    let mut ents: Vec<_> = fs::read_dir(&root.config_dir)?
        .flatten()
        .map(|e| e.file_name())
        .collect();
    ents.sort();
    for name in ents {
        if OWN_HOME.iter().any(|o| name == *o) {
            continue;
        }
        let at = home.config_dir.join(&name);
        if fs::symlink_metadata(&at).is_ok() {
            continue;
        }
        if let Err(e) = symlink(root.config_dir.join(&name), &at)
            && e.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(e);
        }
    }
    sync_state(home, root)
}

/// Gives shared, ~/.claude's folder, what stray holds when stray is a real
/// folder in a home rather than its link: written there before the home
/// was linked, it would never be linked, and rush would never see the
/// transcripts in it. Its files are hard linked, not copied, so a session
/// still writing one by its old path writes the same file. A file in both
/// is kept whole: the longer when one starts with all of the other, else
/// stray's goes on the end of shared's, as a conversation copied into the
/// home and carried on there holds only what came after the copy. Then
/// stray becomes the link.
fn adopt_stray(stray: &Path, shared: &Path) -> io::Result<()> {
    match fs::symlink_metadata(stray) {
        Ok(m) if m.is_dir() => {}
        _ => return Ok(()), // no folder of its own: nothing to adopt
    }
    adopt_dir(stray, shared)?;
    let mut old = stray.as_os_str().to_owned();
    old.push(".adopted");
    let old = PathBuf::from(old);
    fs::rename(stray, &old)?;
    symlink(shared, stray)?;
    fs::remove_dir_all(old)
}

fn adopt_dir(stray: &Path, shared: &Path) -> io::Result<()> {
    mkdir_private(shared)?;
    walk_files(stray, &mut |p| {
        if !fs::symlink_metadata(p).is_ok_and(|m| m.is_file()) {
            return Ok(());
        }
        let rel = p.strip_prefix(stray).map_err(io::Error::other)?;
        let dst = shared.join(rel);
        let dir = dst.parent().unwrap_or(shared);
        mkdir_private(dir)?;
        if dst.exists() {
            return fold(p, &dst);
        }
        let base = dst.file_name().unwrap_or_default().to_string_lossy();
        let tmp = dir.join(format!(".{base}.adopt"));
        let _ = fs::remove_file(&tmp);
        if fs::hard_link(p, &tmp).is_err() {
            return copy_new(p, &dst); // another disk: a copy, kept only where shared has none
        }
        fs::rename(&tmp, &dst)
    })
}

/// Keeps both of two copies of one file in dst: the longer when one
/// starts with all of the other, else src's after dst's.
/// shortcut: a line written to src between this and the link is lost; a
/// lock with Claude Code would close that, if it's ever seen.
fn fold(src: &Path, dst: &Path) -> io::Result<()> {
    let mut theirs = fs::read(dst)?;
    let own = fs::read(src)?;
    if theirs.starts_with(&own) {
        return Ok(());
    }
    if own.starts_with(&theirs) {
        return write_file_atomic(dst, &own, 0o600);
    }
    if theirs.last().is_some_and(|&c| c != b'\n') {
        theirs.push(b'\n');
    }
    theirs.extend_from_slice(&own);
    write_file_atomic(dst, &theirs, 0o600)
}

/// Copies SHARED_STATE from root's state file into the home's, once the
/// home has one.
fn sync_state(home: &Account, root: &Account) -> io::Result<()> {
    let Ok(b) = fs::read(home.state_path()) else {
        return Ok(()); // not seeded yet: seed_home copies it whole
    };
    let Ok(mut own) = serde_json::from_slice::<BTreeMap<String, Box<RawValue>>>(&b) else {
        return Ok(());
    };
    let Some(theirs) = fs::read(root.state_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, Box<RawValue>>>(&b).ok())
    else {
        return Ok(()); // nothing of root's to keep up with
    };
    let mut changed = false;
    for k in SHARED_STATE {
        if let Some(v) = theirs.get(*k)
            && own.get(*k).map(|o| o.get()) != Some(v.get())
        {
            own.insert(k.to_string(), v.clone());
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    write_file_atomic(&home.state_path(), &indent(&own)?, 0o600)
}

/// Gives a home l's sign-in, when it has none that's l's yet, and a state
/// file saying it's l: ~/.claude's, for everything else it holds (projects
/// trusted, onboarding done). cred is the vault's.
pub fn seed_home(home: &Account, root: &Account, l: &Login, cred: Option<&[u8]>) -> io::Result<()> {
    if !home.state_path().exists() {
        let mut b = match fs::read(root.state_path()) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        if b.is_empty() {
            b = b"{}".to_vec();
        }
        write_file_atomic(&home.state_path(), &b, 0o600)?;
    }
    if signed_in_as(home) != l.id
        && let Some(p) = &l.profile
    {
        write_profile(&home.state_path(), p)?;
    }
    if read_creds(home).is_ok_and(|have| usable(&have)) {
        return Ok(());
    }
    let Some(cred) = cred.filter(|c| usable(c)) else {
        return Err(io::Error::other(format!(
            "rush has no sign-in for {}; sign in to it again",
            l.name
        )));
    };
    // The vault's copy has the MCP servers' logins from when it was put
    // away: the ones ~/.claude holds now are the ones to carry on with.
    let cred = match read_creds(root) {
        Ok(now) => with_mcp_logins(cred, &now),
        Err(_) => cred.to_vec(),
    };
    write_creds(home, &cred)
}

/// Puts a sign-in just made for l in its home, over whatever it had:
/// signing in to a login again.
pub fn sign_home(home: &Account, l: &Login, cred: &[u8]) -> io::Result<()> {
    if !home.config_dir.exists() {
        return Ok(()); // no home yet: it's made from the vault when it's used
    }
    write_creds(home, cred)?;
    match &l.profile {
        Some(p) => write_profile(&home.state_path(), p),
        None => Ok(()),
    }
}

/// Whether a home has a sign-in of its own.
pub fn has_home(home: &Account) -> bool {
    read_creds(home).is_ok_and(|c| usable(&c))
}

/// Drops a login's home and its sign-in; ~/.claude's entries it linked to
/// stay.
pub fn remove_home(home: &Account) -> io::Result<()> {
    let _ = delete_creds(home);
    match fs::remove_dir_all(&home.config_dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

// ---- Which login new sessions run as (homes.go) ----

/// Where rush keeps each Claude login's home.
fn homes_dir() -> PathBuf {
    rush_state::dir().join(crate::adapter::KIND)
}

/// The config folder rush keeps for the login with id.
pub fn home_of(id: &str) -> Account {
    Account::at("", homes_dir().join(id))
}

/// ~/.claude, where every session runs, signed in as whichever login is
/// in use, named as cfg names it.
pub fn active(cfg: &Config) -> Account {
    let mut root = Account::home();
    for f in &cfg.folders {
        if Path::new(&f.config_dir) == root.config_dir && !f.name.is_empty() {
            root.name = f.name.clone();
        }
    }
    root
}

/// The login new Claude Code sessions run as, in its home; "" runs them in
/// ~/.claude as whoever it's signed in as. It's a file of its own, written
/// as the switch is made, so a session that rests for a switch finds it
/// when it starts again.
pub fn using() -> String {
    fs::read_to_string(homes_dir().join("using"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// How long using_shown's answer is kept.
const USING_FOR: Ms = 5_000;

/// using_shown's last answer: the folder, the login, when.
static USING_MEMO: LazyLock<Mutex<(PathBuf, String, Ms)>> =
    LazyLock::new(|| Mutex::new((PathBuf::new(), String::new(), 0)));

/// using for showing, not for starting a session: the fleet asks it every
/// reading.
/// shortcut: another rush process's switch shows up to USING_FOR late; a
/// watch on the homes folder if that ever matters.
pub fn using_shown() -> String {
    let mut m = USING_MEMO.lock().unwrap_or_else(|e| e.into_inner());
    let dir = homes_dir();
    if m.0 == dir && m.2 != 0 && now() - m.2 < USING_FOR {
        return m.1.clone();
    }
    *m = (dir, using(), now());
    m.1.clone()
}

/// Makes the next using_shown read the disk.
fn forget_using() {
    USING_MEMO.lock().unwrap_or_else(|e| e.into_inner()).2 = 0;
}

/// Makes id the login new sessions run as; "" is ~/.claude.
pub fn set_using(id: &str) -> io::Result<()> {
    let res = set_using_file(id);
    forget_using();
    res
}

fn set_using_file(id: &str) -> io::Result<()> {
    let path = homes_dir().join("using");
    if id.is_empty() {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    mkdir_private(&homes_dir())?;
    let tmp = homes_dir().join("using.tmp");
    fs::write(&tmp, format!("{id}\n"))?;
    fs::rename(tmp, path)
}

/// The config folder new Claude Code sessions run in: the home of the
/// login in use, or ~/.claude. It reads the disk.
pub fn run_account(c: &Config) -> Account {
    let root = active(c);
    let id = using();
    match c.login(&id) {
        Some(l) if !id.is_empty() => {
            let mut h = home_of(&id);
            h.name = l.name;
            h
        }
        _ => root,
    }
}

/// Readies l's home and makes it the one new sessions run in; the login
/// ~/.claude is signed in as runs there instead, so its sign-in is never
/// in two places.
pub fn use_login(root: &Account, l: &Login) -> io::Result<()> {
    let from = using();
    let res = use_login_now(root, l);
    let mut f = json!({"from": from, "to": l.id, "name": l.name});
    if let Err(e) = &res {
        f["err"] = json!(e.to_string());
    }
    rush_sys::events::log("login-switched", f);
    res
}

fn use_login_now(root: &Account, l: &Login) -> io::Result<()> {
    if signed_in_as(root) == l.id {
        return set_using("");
    }
    let h = home_of(&l.id);
    link_home(&h, root)?;
    if !has_home(&h) {
        let Ok(cred) = rush_state::vault().get(&l.id) else {
            return Err(io::Error::other(format!(
                "rush has no sign-in for {}; sign in to it again",
                l.name
            )));
        };
        if let Ok(id) = oauth::owner(&cred)
            && id != l.id
        {
            return Err(io::Error::other(format!(
                "rush's sign-in for {} is another account's; sign in to it again",
                l.name
            )));
        }
        seed_home(&h, root, l, Some(&cred))?;
    } else {
        seed_home(&h, root, l, None)?;
    }
    set_using(&l.id)
}

/// Keeps the sign-in just made in scratch as its login's, in the vault
/// and, when the login has a home already, there too: signing in to it
/// again.
pub fn adopt_login(scratch: &Account) -> io::Result<Login> {
    let l = the_vault().adopt(scratch)?;
    if let Ok(cred) = rush_state::vault().get(&l.id) {
        let _ = sign_home(&home_of(&l.id), &l, &cred);
    }
    Ok(l)
}

/// Drops what rush keeps of a login: its vault sign-in and its home.
/// Sessions it's the login in use for go back to ~/.claude.
pub fn forget_login(id: &str) -> io::Result<()> {
    if using() == id {
        let _ = set_using("");
    }
    let _ = remove_home(&home_of(id));
    rush_state::vault().forget(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    fn write(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    // home_test.go:14 (TestLinkHome): a home links to everything of
    // ~/.claude's but the sign-in and state file, makes the folders
    // sessions write in ~/.claude first so they land there, and leaves
    // alone what the home has of its own.
    #[test]
    fn link_home_links_all_but_its_own() {
        let t = tempfile::tempdir().unwrap();
        let root = Account::at("", t.path().join(".claude"));
        let home = Account::at("", t.path().join("home"));
        for f in ["settings.json", ".credentials.json", ".claude.json"] {
            write(&root.config_dir.join(f), "{}");
        }
        write(&home.config_dir.join("CLAUDE.md"), "own");
        write(&root.config_dir.join("CLAUDE.md"), "root's");
        link_home(&home, &root).unwrap();
        for name in ["settings.json", "projects", "todos"] {
            let to = fs::read_link(home.config_dir.join(name)).unwrap();
            assert_eq!(
                to,
                root.config_dir.join(name),
                "{name} should link to ~/.claude's"
            );
        }
        // root's own state file is .claude.json inside it here (not the
        // default), so only the sign-in is the home's own of these.
        assert!(fs::symlink_metadata(home.config_dir.join(".credentials.json")).is_err());
        assert!(fs::symlink_metadata(home.config_dir.join(".claude.json")).is_err());
        assert_eq!(
            fs::read_to_string(home.config_dir.join("CLAUDE.md")).unwrap(),
            "own"
        );
        // A session in the home writes its transcript into ~/.claude.
        fs::write(home.config_dir.join("projects/t.jsonl"), "").unwrap();
        assert!(root.config_dir.join("projects/t.jsonl").exists());
        // Its state file keeps up with ~/.claude's MCP servers, and keeps
        // who it's signed in as.
        fs::write(
            home.state_path(),
            r#"{"oauthAccount":{"accountUuid":"me"}}"#,
        )
        .unwrap();
        fs::write(
            root.state_path(),
            r#"{"oauthAccount":{"accountUuid":"other"},"mcpServers":{"x":{}}}"#,
        )
        .unwrap();
        // Linking again, once ~/.claude has something new, adds it.
        fs::write(root.config_dir.join("keybindings.json"), "").unwrap();
        link_home(&home, &root).unwrap();
        assert!(fs::read_link(home.config_dir.join("keybindings.json")).is_ok());
        let b = fs::read_to_string(home.state_path()).unwrap();
        assert!(b.contains(r#""x""#) && signed_in_as(&home) == "me", "{b}");
    }

    // home_test.go:83 (TestLinkHomeAdoptsStray)
    #[test]
    fn link_home_adopts_stray() {
        let t = tempfile::tempdir().unwrap();
        let root = Account::at("", t.path().join(".claude"));
        let home = Account::at("", t.path().join("home"));
        let w = |p: PathBuf, body: &str, age: Duration| {
            write(&p, body);
            File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(SystemTime::now() - age)
                .unwrap();
        };
        let (min, hour) = (Duration::from_secs(60), Duration::from_secs(3600));
        let rp = root.config_dir.join("projects/p");
        let hp = home.config_dir.join("projects/p");
        w(rp.join("moved.jsonl"), "old", hour);
        w(rp.join("kept.jsonl"), "a\nb\n", Duration::ZERO);
        w(rp.join("carried.jsonl"), "before\n", hour);
        w(hp.join("moved.jsonl"), "old+more", min);
        w(hp.join("kept.jsonl"), "a\n", hour);
        w(hp.join("carried.jsonl"), "after\n", Duration::ZERO);
        w(hp.join("new.jsonl"), "only here", Duration::ZERO);
        let mut open = File::options()
            .append(true)
            .open(hp.join("new.jsonl"))
            .unwrap();

        link_home(&home, &root).unwrap();
        assert_eq!(
            fs::read_link(home.config_dir.join("projects")).unwrap(),
            root.config_dir.join("projects")
        );
        open.write_all(b", still").unwrap();
        for (name, want) in [
            ("moved.jsonl", "old+more"),
            ("kept.jsonl", "a\nb\n"),
            ("carried.jsonl", "before\nafter\n"),
            ("new.jsonl", "only here, still"),
        ] {
            assert_eq!(fs::read_to_string(rp.join(name)).unwrap(), want, "{name}");
        }
        assert!(fs::symlink_metadata(home.config_dir.join("projects.adopted")).is_err());
    }

    // homes_test.go (TestRunAccount): new sessions run in the home of the
    // login in use, and in ~/.claude when none is, or it's one rush no
    // longer keeps.
    #[test]
    fn run_account_is_the_login_in_use() {
        let t = tempfile::tempdir().unwrap();
        rush_state::set_thread_dir(Some(t.path().to_path_buf()));
        let cfg = Config {
            logins: vec![Login {
                id: "a".into(),
                name: "work".into(),
                ..Login::default()
            }],
            ..Config::default()
        };
        assert!(
            run_account(&cfg).is_default(),
            "with no login in use, sessions run in ~/.claude"
        );
        set_using("a").unwrap();
        let got = run_account(&cfg);
        assert_eq!(
            (got.config_dir.clone(), got.name.as_str()),
            (home_of("a").config_dir, "work")
        );
        assert_eq!(using_shown(), "a");
        set_using("gone").unwrap();
        assert!(
            run_account(&cfg).is_default(),
            "a login rush doesn't keep runs nothing: ~/.claude"
        );
        set_using("").unwrap();
        assert_eq!(using(), "");
        rush_state::set_thread_dir(None);
    }

    #[test]
    fn seed_home_takes_the_vaults_sign_in_once() {
        crate::login::creds_in_files();
        let t = tempfile::tempdir().unwrap();
        let root = Account::at("", t.path().join(".claude"));
        let home = Account::at("", t.path().join("home"));
        fs::create_dir_all(&root.config_dir).unwrap();
        fs::create_dir_all(&home.config_dir).unwrap();
        fs::write(
            root.state_path(),
            r#"{"hasCompletedOnboarding":true,"oauthAccount":{"accountUuid":"other"}}"#,
        )
        .unwrap();
        write_creds(
            &root,
            br#"{"claudeAiOauth":{"refreshToken":"root"},"mcpOAuth":{"m":{}}}"#,
        )
        .unwrap();
        let l = Login {
            id: "me".into(),
            name: "work".into(),
            profile: Some(rush_state::json::RawJson(
                RawValue::from_string(r#"{"accountUuid":"me"}"#.into()).unwrap(),
            )),
            ..Login::default()
        };
        assert!(
            seed_home(&home, &root, &l, None).is_err(),
            "no sign-in to give it"
        );
        let cred = br#"{"claudeAiOauth":{"refreshToken":"mine"}}"#;
        seed_home(&home, &root, &l, Some(cred)).unwrap();
        assert!(has_home(&home));
        assert_eq!(signed_in_as(&home), "me");
        let st: serde_json::Value =
            serde_json::from_slice(&fs::read(home.state_path()).unwrap()).unwrap();
        assert_eq!(
            st["hasCompletedOnboarding"], true,
            "~/.claude's state, for everything else"
        );
        let got: serde_json::Value = serde_json::from_slice(&read_creds(&home).unwrap()).unwrap();
        assert_eq!(got["claudeAiOauth"]["refreshToken"], "mine");
        assert!(
            got["mcpOAuth"].get("m").is_some(),
            "~/.claude's MCP logins carry on"
        );
        // Once it has its own, the vault's isn't put over it.
        seed_home(
            &home,
            &root,
            &l,
            Some(br#"{"claudeAiOauth":{"refreshToken":"stale"}}"#),
        )
        .unwrap();
        let got: serde_json::Value = serde_json::from_slice(&read_creds(&home).unwrap()).unwrap();
        assert_eq!(got["claudeAiOauth"]["refreshToken"], "mine");
        remove_home(&home).unwrap();
        assert!(!home.config_dir.exists());
    }
}
