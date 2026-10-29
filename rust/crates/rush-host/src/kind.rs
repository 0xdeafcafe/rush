//! Which agent a session runs, and the account it runs on.
//! Ported from internal/host/kind.go.

use std::path::PathBuf;

use rush_core::adapter::{Account, AdapterRef, Kind, Profile, Registry};

use crate::wire::Config;

impl Config {
    /// Sets the config to run kind, in the agent's first config folder,
    /// and says why not for an agent rush doesn't know, can't run or can't
    /// find installed. An account already set is kept.
    pub fn use_agent(&mut self, reg: &Registry, kind: &str) -> Result<(), String> {
        let Some(a) = reg.get(kind) else {
            let kinds: Vec<String> = reg.all().iter().map(|a| a.kind().0).collect();
            return Err(format!(
                "rush doesn't know the agent {}: it knows {}",
                crate::reload::go_quote(kind),
                kinds.join(", ")
            ));
        };
        if a.driver().is_none() {
            return Err(format!("rush can't run {} sessions", a.name()));
        }
        if !self.kind.is_empty() && self.kind != kind {
            return Err(format!(
                "session {} is a {} session, not {kind}",
                self.id, self.kind
            ));
        }
        let k = a.kind();
        if !reg.installed(k.as_str()) {
            reg.recheck(); // it may have been installed since the last look
            if !reg.installed(k.as_str()) {
                return Err(format!(
                    "{} isn't installed: rush can't find its program",
                    a.name()
                ));
            }
        }
        if !reg.runs(k.as_str()) {
            reg.recheck();
            if !reg.runs(k.as_str()) {
                return Err(format!(
                    "{} can't run sessions here yet: {}",
                    a.name(),
                    reg.hint(k.as_str())
                ));
            }
        }
        self.kind = kind.to_string();
        if self.account.dir.is_empty() {
            let Some(p) = reg.profiles_of(&a).into_iter().next() else {
                return Err(format!(
                    "{} isn't installed: rush can't find its program",
                    a.name()
                ));
            };
            self.account = p;
        }
        Ok(())
    }
}

/// The agents rush can run here: their program is on the machine.
pub fn installed(reg: &Registry) -> Vec<AdapterRef> {
    reg.installed_all()
        .into_iter()
        .filter(|a| {
            let k = a.kind();
            reg.current_kind(k.as_str()) == k
                && a.driver().is_some()
                && reg.runs(k.as_str())
                && !reg.profiles_of(a).is_empty()
        })
        .collect()
}

/// Where every rush and session keeps other agents' plan limits as last
/// read.
pub fn quotas_path() -> PathBuf {
    rush_state::dir().join("quotas.json")
}

/// Where the limits of the account a profile is signed in to are kept.
pub fn quota_key(p: &Profile) -> String {
    format!("{}:{}", p.kind, p.dir)
}

/// Signs profile p of agent kind in as the first account rush keeps for
/// it, when it's signed in as no one: a run started from a shell then
/// works as the app's own do. It says why not when it can't, and reads and
/// writes the disk.
pub fn sign_in_if_out(reg: &Registry, kind: &str, p: &Profile) -> Result<(), String> {
    let Some(a) = reg.get(kind) else {
        return Ok(());
    };
    if kind == rush_state::LOGINS_KIND {
        return Ok(());
    }
    let Some(acc) = a.accounts() else {
        return Ok(());
    };
    if acc.current(p).is_ok_and(|c| !c.id.is_empty()) {
        return Ok(());
    }
    let kept = rush_state::Store::load().config.sign_ins_of(kind);
    if kept.is_empty() {
        return Ok(()); // it may sign in where rush can't see (a keyring)
    }
    let mut last = String::new();
    for s in kept {
        let acct = Account {
            kind: Kind::from(kind),
            id: s.id.clone(),
            name: s.name.clone(),
            email: s.email.clone(),
            plan: s.plan.clone(),
            ..Account::default()
        };
        match acc.switch(p, &acct) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    let name = a.name();
    Err(format!(
        "{name} isn't signed in, and rush couldn't sign it in ({last}): in rush, Settings, {name}, press l to sign in again"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_quota_keys() {
        let p = Profile {
            kind: Kind::from("codex"),
            name: "n".into(),
            dir: "/d".into(),
        };
        assert_eq!(quota_key(&p), "codex:/d");
        let reg = Registry::new();
        let mut c = Config::default();
        let e = c.use_agent(&reg, "nova").unwrap_err();
        assert!(
            e.starts_with("rush doesn't know the agent \"nova\": it knows"),
            "{e}"
        );
    }
}
