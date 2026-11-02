//! Reading a one-shot run a shell asked of agy (`agy -p "<task>"`), so
//! rush can host it. Port of internal/adapters/acp/agent.go:215-276
//! (Agent.ReadOnce), which Antigravity's ReadOnce runs with its own flags.

use std::collections::HashMap;

use rush_core::adapter::Once;

/// A program's one-shot command and what each flag means: "prompt",
/// "model", "cwd", "mode", "mode=<value>" (a flag that sets the mode), or
/// "=<value>" (a flag whose value must be that).
pub(crate) struct OnceSpec<'a> {
    pub once: &'a str,
    pub flags: &'a HashMap<&'a str, &'a str>,
}

impl OnceSpec<'_> {
    /// What args ask for; None when rush can't host the run.
    pub fn read(&self, args: &[String]) -> Option<Once> {
        let mut o = Once::default();
        let w: Vec<&str> = self.once.split_whitespace().collect();
        let sub = if w.len() > 1 && !w[1].starts_with('-') {
            w[1]
        } else {
            ""
        };
        let mut words: Vec<String> = Vec::new();
        let mut started = false;
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if !arg.starts_with('-') {
                if !sub.is_empty() && !started && arg == sub {
                    started = true;
                } else if !sub.is_empty() && started {
                    words.push(arg.clone());
                } else {
                    return None; // a prompt for its own screen
                }
                i += 1;
                continue;
            }
            let (name, mut val, eq) = match arg.split_once('=') {
                Some((n, v)) => (n, v.to_string(), true),
                None => (arg.as_str(), String::new(), false),
            };
            let role = self.flags.get(name).copied();
            let bare = role.is_none_or(|r| r.is_empty() || r.starts_with("mode="));
            match role {
                None => return None,
                Some(_) if bare && eq => return None,
                Some(_) if !bare && !eq => {
                    if i + 1 >= args.len() || args[i + 1].starts_with('-') {
                        return None;
                    }
                    i += 1;
                    val = args[i].clone();
                }
                _ => {}
            }
            let role = role.unwrap_or_default();
            let set: Option<&mut String> = if role == "prompt" {
                started = true;
                words.push(val.clone());
                None
            } else if role == "model" {
                Some(&mut o.model)
            } else if role == "cwd" {
                Some(&mut o.cwd)
            } else if role == "mode" {
                Some(&mut o.mode)
            } else if let Some(m) = role.strip_prefix("mode=") {
                val = m.to_string();
                Some(&mut o.mode)
            } else if let Some(want) = role.strip_prefix('=')
                && val != want
            {
                return None;
            } else {
                None
            };
            if let Some(s) = set {
                if !s.is_empty() && *s != val {
                    return None;
                }
                *s = val;
            }
            i += 1;
        }
        o.prompt = words.join(" ");
        (started && !o.prompt.is_empty()).then_some(o)
    }
}
