//! The scratch volume (Settings › Disk's "Give scratch its own volume",
//! macOS only, made only on your yes): an APFS volume in the startup
//! disk's container, sharing its space with no quota, that sessions'
//! $TMPDIR goes on. On it macOS keeps no FSEvents log (`.fseventsd/no_log`
//! at its top, read at each mount), Spotlight doesn't index it (`mdutil -i
//! off`, kept in the volume's own Spotlight store) and Time Machine skips
//! it (`tmutil addexclusion -p`), so a build's hundreds of thousands of
//! files an hour no longer set fseventsd, mds and backupd working.
//!
//! Making and removing it need an admin: each is one shell script, run
//! by `osascript`'s `do shell script … with administrator privileges`, so
//! macOS's own dialog asks for the password once. The scripts are built
//! by pure functions ([`create_script`], [`remove_script`],
//! [`applescript`]); what's on the volume is read by statfs and `mdutil
//! -s`, neither of which needs one. Where scratch goes once it's there is
//! rush-state's (`ready_volume_root`). Rust-only: Go had none.
//!
//! With FileVault on (`fdesetup status`), the volume is encrypted as the
//! disk is: rush makes a random passphrase, keeps it in the login
//! keychain (service `rush-scratch-volume`, account `rush-scratch`, put
//! there through `security -i` on stdin so it's never in a process's
//! arguments), and hands it to diskutil on stdin from an owner-only file
//! the admin script removes. macOS doesn't unlock such a volume by itself
//! after a restart (it does only for passwords it saved itself, through
//! its own prompt), so the rush that leads unlocks it ([`unlock`]:
//! `diskutil apfs unlockVolume … -stdinpassphrase`, no admin needed),
//! when it starts and every 10 minutes it's still locked. Until then new
//! sessions' scratch goes in /tmp. Removing the volume removes the
//! keychain item. The passphrase is never logged nor drawn.

use std::path::Path;

use rush_state::{MountFacts, VOLUME_MOUNT, VOLUME_NAME};

/// The volume whose APFS container the scratch volume shares.
pub const DATA_VOLUME: &str = "/System/Volumes/Data";

/// What's on the volume, as read without an admin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// An APFS volume is mounted at /Volumes/rush-scratch.
    pub mounted: bool,
    /// New sessions' scratch goes on it (rush_state::volume_ready).
    pub ready: bool,
    /// Its device: disk3s7.
    pub device: String,
    /// `.fseventsd/no_log` is at its top: FSEvents keeps no log of it.
    pub no_log: bool,
    /// Whether Spotlight indexes it, as `mdutil -s` says; None unknown.
    pub indexing: Option<bool>,
    /// Owners and modes count on it.
    pub owners: bool,
    /// Why it's mounted but not used, when it is.
    pub why: String,
    /// A volume of its name is in the container, mounted or not.
    pub made: bool,
    /// It's encrypted (FileVault, its own passphrase).
    pub encrypted: bool,
    /// It's encrypted and locked: not mounted until rush unlocks it.
    pub locked: bool,
    /// Whether FileVault is on for the startup disk; None unknown.
    pub filevault: Option<bool>,
}

/// The keychain item's service: the volume's passphrase.
pub const KEYCHAIN_SERVICE: &str = "rush-scratch-volume";

/// The keychain item's account.
pub const KEYCHAIN_ACCOUNT: &str = VOLUME_NAME;

/// The scratch volume as `diskutil apfs list` lists it, mounted or not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listed {
    /// disk3s7.
    pub device: String,
    /// "FileVault: Yes …".
    pub encrypted: bool,
    /// "FileVault: Yes (Locked)".
    pub locked: bool,
}

/// The volume named rush-scratch in `diskutil apfs list`'s answer: each
/// volume starts at a "+-> Volume disk3s7 <uuid>" line, then has its
/// "Name:" and "FileVault:" lines. Pure.
pub fn find_volume(list: &str) -> Option<Listed> {
    let mut cur: Option<Listed> = None;
    let mut named = false;
    let mut found: Option<Listed> = None;
    let mut done = |cur: &mut Option<Listed>, named: &mut bool| {
        if *named && found.is_none() {
            found = cur.take();
        }
        *cur = None;
        *named = false;
    };
    for line in list.lines() {
        let t = line.trim_start_matches([' ', '|', '+', '-', '>']).trim();
        if let Some(rest) = t.strip_prefix("Volume ")
            && let Some(dev) = rest.split_whitespace().next()
            && is_volume_device(dev)
        {
            done(&mut cur, &mut named);
            cur = Some(Listed {
                device: dev.to_string(),
                ..Listed::default()
            });
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        if let Some(n) = t.strip_prefix("Name:") {
            let n = n.trim();
            named = n == VOLUME_NAME || n.starts_with(&format!("{VOLUME_NAME} ("));
        } else if let Some(f) = t.strip_prefix("FileVault:") {
            let f = f.trim();
            c.encrypted = f.starts_with("Yes");
            c.locked = c.encrypted && f.contains("(Locked)");
        }
    }
    done(&mut cur, &mut named);
    found
}

/// Whether `fdesetup status` says FileVault is on ("FileVault is On.";
/// "Encryption in progress" counts too). Pure.
pub fn filevault_on(out: &str) -> bool {
    out.contains("FileVault is On") || out.contains("Encryption in progress")
}

/// s as one word for sh, in single quotes.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Whether s names a whole disk or container: disk3.
fn is_disk(s: &str) -> bool {
    s.strip_prefix("disk")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether s names a volume of a container: disk3s7.
pub fn is_volume_device(s: &str) -> bool {
    s.rsplit_once('s')
        .is_some_and(|(d, v)| is_disk(d) && !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

/// The APFS container in `diskutil info -plist`'s answer: its
/// APFSContainerReference (disk3). None when it isn't there or isn't a
/// plain disk name, so nothing else ever reaches the admin script.
pub fn container_of(plist: &str) -> Option<String> {
    let rest = plist.split("<key>APFSContainerReference</key>").nth(1)?;
    let rest = rest.trim_start().strip_prefix("<string>")?;
    let name = rest.split("</string>").next()?.trim();
    is_disk(name).then(|| name.to_string())
}

/// What `mdutil -s` says of indexing: "Indexing disabled." or "Indexing
/// enabled."; None when it says neither.
pub fn indexing_of(out: &str) -> Option<bool> {
    if out.contains("Indexing disabled") {
        Some(false)
    } else if out.contains("Indexing enabled") {
        Some(true)
    } else {
        None
    }
}

/// The volume's state from what was read: statfs at the mount point,
/// whether its user folder is ready, the no_log file, and `mdutil -s`.
/// Pure.
pub fn status_of(
    facts: Option<&MountFacts>,
    listed: Option<&Listed>,
    filevault: Option<bool>,
    ready: bool,
    no_log: bool,
    indexing: Option<bool>,
) -> Status {
    let mounted = facts.is_some_and(|f| f.on == Path::new(VOLUME_MOUNT) && f.fs == "apfs");
    if !mounted {
        let Some(l) = listed else {
            return Status {
                filevault,
                ..Status::default()
            };
        };
        return Status {
            made: true,
            device: l.device.clone(),
            encrypted: l.encrypted,
            locked: l.locked,
            filevault,
            why: if l.locked {
                "locked: rush unlocks it when it starts".into()
            } else {
                "not mounted".into()
            },
            ..Status::default()
        };
    }
    let f = facts.cloned().unwrap_or_default();
    let device = f.from.strip_prefix("/dev/").unwrap_or(&f.from).to_string();
    let why = if ready {
        String::new()
    } else if !f.owners {
        "owners are ignored on it".into()
    } else {
        "its rush folder isn't yours alone".into()
    };
    Status {
        mounted,
        ready,
        device,
        no_log,
        indexing,
        owners: f.owners,
        why,
        made: true,
        encrypted: listed.is_some_and(|l| l.encrypted),
        locked: false,
        filevault,
    }
}

/// What's on the volume now. Off the UI: it runs `mdutil -s` when the
/// volume's there. Nothing in a test, which has no real volume.
pub fn status() -> Status {
    if rush_core::sandbox::active() {
        return Status::default();
    }
    let mount = Path::new(VOLUME_MOUNT);
    let facts = rush_state::mount_facts(mount);
    let ready = rush_state::ready_volume_root().is_some();
    let no_log = mount.join(".fseventsd").join("no_log").exists();
    let mounted = facts.as_ref().is_some_and(|f| f.on == mount);
    let indexing = if mounted {
        std::process::Command::new("/usr/bin/mdutil")
            .arg("-s")
            .arg(mount)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|o| indexing_of(&String::from_utf8_lossy(&o.stdout)))
    } else {
        None
    };
    let listed = apfs_list().as_deref().and_then(find_volume);
    status_of(
        facts.as_ref(),
        listed.as_ref(),
        filevault(),
        ready,
        no_log,
        indexing,
    )
}

/// `diskutil apfs list`'s answer; None when it can't be run.
fn apfs_list() -> Option<String> {
    let o = std::process::Command::new("/usr/sbin/diskutil")
        .args(["apfs", "list"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Whether FileVault is on, asked of `fdesetup status` once a process.
fn filevault() -> Option<bool> {
    static FV: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *FV.get_or_init(|| {
        let o = std::process::Command::new("/usr/bin/fdesetup")
            .arg("status")
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        Some(filevault_on(&String::from_utf8_lossy(&o.stdout)))
    })
}

/// The script that makes the volume, run as root: refuses when one of
/// its name is there already; adds it to container, sharing its space
/// (no quota, no reserve); turns owners on; turns Spotlight off before
/// it starts indexing; writes `.fseventsd/no_log` and remounts so
/// FSEvents reads it; makes its top root's, 0755, and `rush/` in it
/// uid's alone (0700); excludes it from Time Machine by path (`-p`, which
/// needs the root this runs as), else by the sticky xattr. Err when
/// container isn't a plain disk name.
///
/// With pass (FileVault on) it's encrypted: diskutil reads the
/// passphrase on stdin from that file, which is removed however the
/// script ends; one that comes back locked from the remount is unlocked
/// from it.
pub fn create_script(
    container: &str,
    uid: u32,
    gid: u32,
    pass: Option<&Path>,
) -> Result<String, String> {
    if !is_disk(container) {
        return Err(format!("{container:?} isn't an APFS container"));
    }
    let (m, n) = (VOLUME_MOUNT, VOLUME_NAME);
    let mut lines = vec!["set -e".to_string(), format!("m='{m}'")];
    if let Some(p) = pass {
        lines.push(format!("p={}", sh_quote(&p.to_string_lossy())));
        lines.push("trap '/bin/rm -f \"$p\"' EXIT".into());
    }
    lines.push(format!(
        "if [ -e \"$m\" ] || /usr/sbin/diskutil apfs list {container} | /usr/bin/grep -q 'Name: *{n} ('; then echo 'a volume named {n} is there already' >&2; exit 1; fi"
    ));
    lines.push(match pass {
        Some(_) => format!(
            "/usr/sbin/diskutil apfs addVolume {container} APFS {n} -stdinpassphrase < \"$p\""
        ),
        None => format!("/usr/sbin/diskutil apfs addVolume {container} APFS {n}"),
    });
    let mount = match pass {
        Some(_) => {
            "/usr/sbin/diskutil mount \"$d\" || /usr/sbin/diskutil apfs unlockVolume \"$d\" -stdinpassphrase < \"$p\""
        }
        None => "/usr/sbin/diskutil mount \"$d\"",
    };
    lines.extend([
        "/usr/sbin/diskutil enableOwnership \"$m\"".into(),
        "/usr/bin/mdutil -i off \"$m\"".into(),
        "/bin/mkdir -p \"$m/.fseventsd\"".into(),
        "/usr/bin/touch \"$m/.fseventsd/no_log\" \"$m/.metadata_never_index\"".into(),
        "d=$(/usr/sbin/diskutil info \"$m\" | /usr/bin/awk '/Device Identifier:/ {print $3}')"
            .into(),
        "/usr/sbin/diskutil unmount \"$d\" || /usr/sbin/diskutil unmount force \"$d\"".into(),
        mount.into(),
        "/usr/sbin/chown root:wheel \"$m\"".into(),
        "/bin/chmod 755 \"$m\"".into(),
        "/bin/mkdir -m 700 \"$m/rush\"".into(),
        format!("/usr/sbin/chown {uid}:{gid} \"$m/rush\""),
        "/usr/bin/tmutil addexclusion -p \"$m\" || /usr/bin/tmutil addexclusion \"$m\"".into(),
    ]);
    Ok(lines.join("\n"))
}

/// The script that removes the volume at device, run as root: only when
/// device is still the volume of that name, mounted at
/// /Volumes/rush-scratch or not at all (a locked one), (what's there is
/// checked again, not taken from when rush looked), its
/// Time Machine exclusion taken back first. Everything on it goes. Err
/// when device isn't a plain volume name.
pub fn remove_script(device: &str) -> Result<String, String> {
    if !is_volume_device(device) {
        return Err(format!("{device:?} isn't an APFS volume"));
    }
    let (m, n) = (VOLUME_MOUNT, VOLUME_NAME);
    Ok([
        "set -e".to_string(),
        format!("m='{m}'"),
        format!(
            "if ! /usr/sbin/diskutil info {device} | /usr/bin/grep -q 'Volume Name: *{n}$' || ! /usr/sbin/diskutil info {device} | /usr/bin/grep -Eq \"Mount Point: *($m|Not Mounted)$\"; then echo '{device} is not the {n} volume' >&2; exit 1; fi"
        ),
        "/usr/bin/tmutil removeexclusion -p \"$m\" 2>/dev/null || true".into(),
        format!("/usr/sbin/diskutil apfs deleteVolume {device}"),
    ]
    .join("\n"))
}

/// An AppleScript that runs sh as root, macOS's own dialog asking for
/// the password with prompt: `do shell script "…" with prompt "…" with
/// administrator privileges`, each string escaped as AppleScript wants
/// (backslash, double quote; a new line as \n).
pub fn applescript(sh: &str, prompt: &str) -> String {
    let q = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    };
    format!(
        "do shell script \"{}\" with prompt \"{}\" with administrator privileges",
        q(sh),
        q(prompt)
    )
}

/// The ids of the sessions whose scratch, of those given (id, its
/// TMPDIR), is on the volume whose user folder is root. Pure.
pub fn using(root: &Path, temps: &[(String, String)]) -> Vec<String> {
    temps
        .iter()
        .filter(|(_, t)| !t.is_empty() && Path::new(t.trim_end_matches('/')).starts_with(root))
        .map(|(id, _)| id.clone())
        .collect()
}

/// The prompt macOS's dialog shows when making it.
pub const CREATE_PROMPT: &str = "rush wants to make a volume for agents' scratch files, with no FSEvents log, Spotlight or Time Machine.";

/// The prompt macOS's dialog shows when removing it.
pub const REMOVE_PROMPT: &str =
    "rush wants to remove its scratch volume and the scratch files on it.";

/// Runs sh as root through osascript, macOS asking for the password.
/// Never in a test.
fn run_admin(sh: &str, prompt: &str) -> Result<(), String> {
    if rush_core::sandbox::active() {
        return Err("not in a test".into());
    }
    if !cfg!(target_os = "macos") {
        return Err("only on macOS".into());
    }
    let out = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(applescript(sh, prompt))
        .output()
        .map_err(|e| format!("couldn't run osascript: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if err.contains("-128") {
        return Err("cancelled".into());
    }
    Err(err)
}

/// Makes the volume: finds the startup disk's container, then runs
/// [`create_script`] as root. Off the UI; macOS asks for the password.
pub fn create() -> Result<(), String> {
    if rush_core::sandbox::active() {
        return Err("not in a test".into());
    }
    let out = std::process::Command::new("/usr/sbin/diskutil")
        .args(["info", "-plist", DATA_VOLUME])
        .output()
        .map_err(|e| format!("couldn't run diskutil: {e}"))?;
    let container = container_of(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| format!("couldn't find {DATA_VOLUME}'s APFS container"))?;
    // SAFETY: geteuid and getegid can't fail.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    let r = if filevault() == Some(true) {
        create_encrypted(&container, uid, gid)
    } else {
        create_script(&container, uid, gid, None).and_then(|sh| run_admin(&sh, CREATE_PROMPT))
    };
    log("made", &container, &r);
    r
}

/// Makes it encrypted: a new passphrase in the keychain first, then in
/// an owner-only file in rush's folder for the script to read and remove
/// (removed here too, whatever happened). When making it failed and no
/// volume of its name is there, the keychain item goes again.
fn create_encrypted(container: &str, uid: u32, gid: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let pass = new_passphrase().map_err(|e| format!("couldn't make a passphrase: {e}"))?;
    keychain_put(&pass)?;
    let file = rush_state::dir().join("scratch-volume.pass");
    let _ = std::fs::remove_file(&file);
    let wrote = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .and_then(|mut f| f.write_all(pass.as_bytes()));
    let r = match wrote {
        Err(e) => Err(format!("couldn't hand the passphrase over: {e}")),
        Ok(()) => create_script(container, uid, gid, Some(&file))
            .and_then(|sh| run_admin(&sh, CREATE_PROMPT)),
    };
    let _ = std::fs::remove_file(&file);
    if r.is_err() && apfs_list().as_deref().and_then(find_volume).is_none() {
        let _ = keychain_delete();
    }
    r
}

/// A new passphrase: 32 random bytes from the system, as hex.
fn new_passphrase() -> std::io::Result<String> {
    use std::io::Read;
    let mut b = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// What `security -i` is told to keep the passphrase: one line on its
/// stdin, so the passphrase is in no process's arguments. -U replaces
/// one there already. Pure.
pub fn keychain_put_command(pass: &str) -> String {
    format!(
        "add-generic-password -U -s {KEYCHAIN_SERVICE} -a {KEYCHAIN_ACCOUNT} -l {KEYCHAIN_SERVICE} -w {pass}\n"
    )
}

/// Keeps the passphrase in the login keychain. Never in a test.
fn keychain_put(pass: &str) -> Result<(), String> {
    if rush_core::sandbox::active() {
        return Err("not in a test".into());
    }
    let out = with_stdin(
        std::process::Command::new("/usr/bin/security").arg("-i"),
        &keychain_put_command(pass),
    )
    .map_err(|e| format!("couldn't run security: {e}"))?;
    let err = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || err.contains("error") {
        return Err(format!(
            "couldn't keep the passphrase in your keychain: {}",
            err.trim()
        ));
    }
    Ok(())
}

/// The passphrase from the login keychain. Never in a test.
fn keychain_get() -> Result<String, String> {
    if rush_core::sandbox::active() {
        return Err("not in a test".into());
    }
    let out = std::process::Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            KEYCHAIN_ACCOUNT,
            "-w",
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run security: {e}"))?;
    let pass = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || pass.is_empty() {
        return Err("its passphrase isn't in your keychain".into());
    }
    Ok(pass)
}

/// Removes the passphrase from the login keychain; one not there is
/// fine. Never in a test.
fn keychain_delete() -> Result<(), String> {
    if rush_core::sandbox::active() {
        return Err("not in a test".into());
    }
    let out = std::process::Command::new("/usr/bin/security")
        .args([
            "delete-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            KEYCHAIN_ACCOUNT,
        ])
        .output()
        .map_err(|e| format!("couldn't run security: {e}"))?;
    // 44: errSecItemNotFound, as security exits.
    if out.status.success() || out.status.code() == Some(44) {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// Runs c with input on its stdin, and its output.
fn with_stdin(c: &mut std::process::Command, input: &str) -> std::io::Result<std::process::Output> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut i) = child.stdin.take() {
        i.write_all(input.as_bytes())?;
    }
    child.wait_with_output()
}

/// What unlocking did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unlocked {
    /// There's nothing to unlock: no volume, a plain one, or one mounted.
    Nothing,
    /// It was locked and is mounted now.
    Unlocked,
}

/// Unlocks the volume when it's there, encrypted and locked, with the
/// passphrase from the keychain on diskutil's stdin. Cheap when it's
/// mounted (statfs only). Off the UI; never in a test.
pub fn unlock() -> Result<Unlocked, String> {
    if rush_core::sandbox::active() || !cfg!(target_os = "macos") {
        return Ok(Unlocked::Nothing);
    }
    let mount = Path::new(VOLUME_MOUNT);
    if rush_state::mount_facts(mount).is_some_and(|f| f.on == mount) {
        return Ok(Unlocked::Nothing);
    }
    let Some(l) = apfs_list().as_deref().and_then(find_volume) else {
        return Ok(Unlocked::Nothing);
    };
    if !l.locked {
        return Ok(Unlocked::Nothing);
    }
    let pass = keychain_get()?;
    let out = with_stdin(
        std::process::Command::new("/usr/sbin/diskutil").args([
            "apfs",
            "unlockVolume",
            &l.device,
            "-stdinpassphrase",
        ]),
        &pass,
    )
    .map_err(|e| format!("couldn't run diskutil: {e}"))?;
    let r = if out.status.success() {
        Ok(Unlocked::Unlocked)
    } else {
        Err(format!(
            "couldn't unlock it: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    };
    let said: Result<(), String> = match &r {
        Ok(_) => Ok(()),
        Err(e) => Err(e.clone()),
    };
    log("unlocked", &l.device, &said);
    r
}

/// Removes the volume, unless a session's scratch (in_use: their ids)
/// is on it. Off the UI; macOS asks for the password.
pub fn remove(device: &str, in_use: &[String]) -> Result<(), String> {
    if !in_use.is_empty() {
        return Err(format!(
            "{} running session{} {} scratch on it: {}",
            in_use.len(),
            if in_use.len() == 1 { "" } else { "s" },
            if in_use.len() == 1 {
                "has its"
            } else {
                "have their"
            },
            in_use.join(", ")
        ));
    }
    let mut r = remove_script(device).and_then(|sh| run_admin(&sh, REMOVE_PROMPT));
    if r.is_ok()
        && let Err(e) = keychain_delete()
    {
        r = Err(format!(
            "removed it, but couldn't remove its passphrase from your keychain ({KEYCHAIN_SERVICE}): {e}"
        ));
    }
    log("removed", device, &r);
    r
}

/// Logs a scratch-volume event: what was done, to what, or why not.
fn log(did: &str, on: &str, r: &Result<(), String>) {
    let mut f = serde_json::json!({ "did": did, "on": on });
    if let Err(e) = r {
        f["err"] = e.clone().into();
    }
    crate::temp::agent_event(None, "scratch-volume", f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_reach_the_scripts_only_when_plain() {
        for (s, disk, vol) in [
            ("disk3", true, false),
            ("disk3s7", false, true),
            ("disk12s104", false, true),
            ("disk", false, false),
            ("disks1", false, false),
            ("disk3s", false, false),
            ("disk3; rm -rf /", false, false),
            ("disk3s7'", false, false),
            ("", false, false),
        ] {
            assert_eq!(is_disk(s), disk, "{s:?}");
            assert_eq!(is_volume_device(s), vol, "{s:?}");
        }
        assert!(create_script("disk3; reboot", 501, 20, None).is_err());
        assert!(remove_script("disk3s7 && reboot").is_err());
    }

    #[test]
    fn the_container_from_diskutil() {
        let plist = "<dict>\n\t<key>APFSContainerReference</key>\n\t<string>disk3</string>\n\t<key>APFSPhysicalStores</key>";
        for (text, want) in [
            (plist, Some("disk3")),
            (
                "<key>APFSContainerReference</key><string>disk3s1</string>",
                None,
            ),
            (
                "<key>APFSContainerReference</key><string>$(x)</string>",
                None,
            ),
            ("<key>DeviceIdentifier</key><string>disk3s5</string>", None),
            ("", None),
        ] {
            assert_eq!(container_of(text).as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn mdutil_says_indexing() {
        for (out, want) in [
            (
                "/Volumes/rush-scratch:\n\tIndexing disabled.\n",
                Some(false),
            ),
            ("/:\n\tIndexing enabled. \n", Some(true)),
            ("Error: unknown indexing state.", None),
        ] {
            assert_eq!(indexing_of(out), want, "{out:?}");
        }
    }

    // The script makes it in the container named, sharing its space,
    // owners on, no FSEvents log before it's remounted, the user's
    // folder theirs alone, out of Spotlight and Time Machine; in that
    // order, stopping at the first failure.
    #[test]
    fn the_create_script_word_for_word() {
        let sh = create_script("disk3", 501, 20, None).unwrap();
        let want = "set -e
m='/Volumes/rush-scratch'
if [ -e \"$m\" ] || /usr/sbin/diskutil apfs list disk3 | /usr/bin/grep -q 'Name: *rush-scratch ('; then echo 'a volume named rush-scratch is there already' >&2; exit 1; fi
/usr/sbin/diskutil apfs addVolume disk3 APFS rush-scratch
/usr/sbin/diskutil enableOwnership \"$m\"
/usr/bin/mdutil -i off \"$m\"
/bin/mkdir -p \"$m/.fseventsd\"
/usr/bin/touch \"$m/.fseventsd/no_log\" \"$m/.metadata_never_index\"
d=$(/usr/sbin/diskutil info \"$m\" | /usr/bin/awk '/Device Identifier:/ {print $3}')
/usr/sbin/diskutil unmount \"$d\" || /usr/sbin/diskutil unmount force \"$d\"
/usr/sbin/diskutil mount \"$d\"
/usr/sbin/chown root:wheel \"$m\"
/bin/chmod 755 \"$m\"
/bin/mkdir -m 700 \"$m/rush\"
/usr/sbin/chown 501:20 \"$m/rush\"
/usr/bin/tmutil addexclusion -p \"$m\" || /usr/bin/tmutil addexclusion \"$m\"";
        assert_eq!(sh, want);
        assert!(
            !sh.contains("-quota") && !sh.contains("-reserve"),
            "shares the space"
        );
        let at = |s: &str| sh.find(s).unwrap_or_else(|| panic!("no {s:?}"));
        assert!(at("addVolume") < at("no_log") && at("no_log") < at("unmount"));
        assert!(at("enableOwnership") < at("mkdir -m 700"));
        assert!(
            at("mdutil") < at("unmount"),
            "Spotlight off before it holds the volume"
        );
    }

    // With FileVault on: the passphrase only ever on diskutil's stdin,
    // from a file removed however the script ends; the rest as plain.
    #[test]
    fn the_encrypted_create_script() {
        let plain = create_script("disk3", 501, 20, None).unwrap();
        let sh = create_script(
            "disk3",
            501,
            20,
            Some(Path::new("/Users/a b/.rush/scratch-volume.pass")),
        )
        .unwrap();
        let lines: Vec<&str> = sh.lines().collect();
        assert_eq!(lines[2], "p='/Users/a b/.rush/scratch-volume.pass'");
        assert_eq!(lines[3], "trap '/bin/rm -f \"$p\"' EXIT");
        assert!(lines.contains(
            &"/usr/sbin/diskutil apfs addVolume disk3 APFS rush-scratch -stdinpassphrase < \"$p\""
        ));
        assert!(lines.contains(
            &"/usr/sbin/diskutil mount \"$d\" || /usr/sbin/diskutil apfs unlockVolume \"$d\" -stdinpassphrase < \"$p\""
        ));
        assert!(!sh.contains("-passphrase "), "a passphrase in arguments");
        let rest: Vec<&str> = lines
            .iter()
            .filter(|l| !l.contains("$p") && !l.starts_with("p="))
            .copied()
            .collect();
        let plain_rest: Vec<&str> = plain
            .lines()
            .filter(|l| !l.contains("addVolume") && !l.starts_with("/usr/sbin/diskutil mount"))
            .collect();
        assert_eq!(rest, plain_rest);
    }

    #[test]
    fn sh_quote_is_one_word() {
        for (s, want) in [
            ("/a b/c", "'/a b/c'"),
            ("it's", "'it'\\''s'"),
            ("$(x)", "'$(x)'"),
        ] {
            assert_eq!(sh_quote(s), want, "{s:?}");
        }
    }

    #[test]
    fn the_keychain_line() {
        assert_eq!(
            keychain_put_command("abc123"),
            "add-generic-password -U -s rush-scratch-volume -a rush-scratch -l rush-scratch-volume -w abc123\n"
        );
    }

    #[test]
    fn filevault_from_fdesetup() {
        for (out, want) in [
            ("FileVault is On.\n", true),
            ("FileVault is Off.\n", false),
            ("Encryption in progress: Percent completed = 12.3\n", true),
            ("", false),
        ] {
            assert_eq!(filevault_on(out), want, "{out:?}");
        }
    }

    // diskutil apfs list, trimmed: the volume found by its name with its
    // device, encrypted and locked or not; another's never.
    #[test]
    fn the_volume_in_diskutils_list() {
        let list = |name: &str, fv: &str| {
            format!(
                "APFS Container (1 found)
|
+-- Container disk3 11111111-2222-3333-4444-555555555555
    ====================================================
    APFS Container Reference:     disk3
    |
    +-> Volume disk3s5 AAAAAAAA-2222-3333-4444-555555555555
    |   ---------------------------------------------------
    |   APFS Volume Disk (Role):   disk3s5 (Data)
    |   Name:                      Macintosh HD - Data (Case-insensitive)
    |   Mount Point:               /System/Volumes/Data
    |   FileVault:                 Yes (Unlocked)
    |
    +-> Volume disk3s7 BBBBBBBB-2222-3333-4444-555555555555
    |   ---------------------------------------------------
    |   APFS Volume Disk (Role):   disk3s7 (No specific role)
    |   Name:                      {name} (Case-insensitive)
    |   Mount Point:               Not Mounted
    |   FileVault:                 {fv}
"
            )
        };
        let l = |dev: &str, encrypted, locked| {
            Some(Listed {
                device: dev.into(),
                encrypted,
                locked,
            })
        };
        for (text, want) in [
            (
                list("rush-scratch", "Yes (Locked)"),
                l("disk3s7", true, true),
            ),
            (
                list("rush-scratch", "Yes (Unlocked)"),
                l("disk3s7", true, false),
            ),
            (list("rush-scratch", "No"), l("disk3s7", false, false)),
            (list("rush-scratch-2", "No"), None),
            (list("other", "Yes (Locked)"), None),
            (String::new(), None),
        ] {
            assert_eq!(find_volume(&text), want, "{text}");
        }
    }

    #[test]
    fn the_remove_script_checks_what_it_removes() {
        let sh = remove_script("disk3s7").unwrap();
        let want = "set -e
m='/Volumes/rush-scratch'
if ! /usr/sbin/diskutil info disk3s7 | /usr/bin/grep -q 'Volume Name: *rush-scratch$' || ! /usr/sbin/diskutil info disk3s7 | /usr/bin/grep -Eq \"Mount Point: *($m|Not Mounted)$\"; then echo 'disk3s7 is not the rush-scratch volume' >&2; exit 1; fi
/usr/bin/tmutil removeexclusion -p \"$m\" 2>/dev/null || true
/usr/sbin/diskutil apfs deleteVolume disk3s7";
        assert_eq!(sh, want);
    }

    #[test]
    fn applescript_escapes_the_script() {
        for (sh, prompt, want) in [
            (
                "echo hi",
                "why",
                "do shell script \"echo hi\" with prompt \"why\" with administrator privileges",
            ),
            (
                "a \"b\"\nc \\d",
                "it's \"so\"",
                "do shell script \"a \\\"b\\\"\\nc \\\\d\" with prompt \"it's \\\"so\\\"\" with administrator privileges",
            ),
        ] {
            assert_eq!(applescript(sh, prompt), want, "{sh:?}");
        }
    }

    #[test]
    fn status_from_what_was_read() {
        let on = MountFacts {
            on: VOLUME_MOUNT.into(),
            from: "/dev/disk3s7".into(),
            fs: "apfs".into(),
            owners: true,
        };
        let elsewhere = MountFacts {
            on: "/System/Volumes/Data".into(),
            ..on.clone()
        };
        let no_owners = MountFacts {
            owners: false,
            ..on.clone()
        };
        assert_eq!(
            status_of(None, None, None, false, false, None),
            Status::default()
        );
        assert_eq!(
            status_of(Some(&elsewhere), None, None, false, false, None),
            Status::default(),
            "a folder left in /Volumes"
        );
        let s = status_of(Some(&on), None, None, true, true, Some(false));
        assert!(s.mounted && s.made && s.ready && s.no_log && s.owners && s.why.is_empty());
        assert!(!s.encrypted);
        assert_eq!((s.device.as_str(), s.indexing), ("disk3s7", Some(false)));
        let s = status_of(Some(&no_owners), None, None, false, true, None);
        assert_eq!(
            (s.ready, s.why.as_str()),
            (false, "owners are ignored on it")
        );
        let s = status_of(Some(&on), None, None, false, true, None);
        assert_eq!(s.why, "its rush folder isn't yours alone");
        // Encrypted: mounted, or locked and not mounted till rush unlocks it.
        let enc = Listed {
            device: "disk3s7".into(),
            encrypted: true,
            locked: false,
        };
        let s = status_of(Some(&on), Some(&enc), Some(true), true, true, None);
        assert!(s.mounted && s.encrypted && !s.locked && s.filevault == Some(true));
        let locked = Listed {
            locked: true,
            ..enc.clone()
        };
        let s = status_of(None, Some(&locked), Some(true), false, false, None);
        assert!(s.made && !s.mounted && s.encrypted && s.locked && !s.ready);
        assert_eq!(
            (s.device.as_str(), s.why.as_str()),
            ("disk3s7", "locked: rush unlocks it when it starts")
        );
        let s = status_of(None, None, Some(false), false, false, None);
        assert!(!s.made && s.filevault == Some(false));
    }

    #[test]
    fn sessions_using_it() {
        let root = Path::new("/Volumes/rush-scratch/rush");
        let temps = [
            (
                "a".to_string(),
                "/Volumes/rush-scratch/rush/app-1/a/".to_string(),
            ),
            ("b".to_string(), "/private/tmp/rush/app-1/b".to_string()),
            ("c".to_string(), "/Volumes/rush-scratch/rushx/c".to_string()),
            ("d".to_string(), String::new()),
        ];
        assert_eq!(using(root, &temps), ["a"]);
    }

    #[test]
    fn nothing_runs_in_a_test() {
        assert_eq!(run_admin("true", "x"), Err("not in a test".into()));
        assert_eq!(status(), Status::default());
        assert_eq!(create(), Err("not in a test".into()));
        assert_eq!(unlock(), Ok(Unlocked::Nothing));
        for r in [
            keychain_put("x"),
            keychain_delete(),
            keychain_get().map(|_| ()),
        ] {
            assert_eq!(r, Err("not in a test".into()));
        }
        assert!(
            remove("disk3s7", &["s1".into()])
                .unwrap_err()
                .contains("s1")
        );
    }
}
