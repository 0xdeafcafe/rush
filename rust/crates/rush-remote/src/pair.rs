//! Pairing a phone: rush (with the token) asks serve for a one-time code
//! and shows it as a QR code of the app's address with the code after
//! #pair=. The phone opens it, and the app trades the code for the
//! sign-in cookie at /api/login. The code works once, for 10 minutes; the
//! token itself is never shown. A fragment isn't sent in requests, so the
//! code isn't in any log.
//!
//! After Go's internal/remote/pair.go (pairTTL :22, Pairing :25, pairCodes
//! :37, pairAlphabet :44, pairMisses :48, issue :50, take :72, PairCode
//! :93, pair :100, reachedAt :120).

use std::collections::HashMap;
use std::sync::Mutex;

use rush_state::Time;
use serde::{Deserialize, Serialize};

use crate::config::rand_bytes;
use crate::devices::same;

/// How long a code works.
pub const PAIR_TTL_NS: i64 = 10 * 60 * 1_000_000_000;

/// A code to pair a phone with, and where the phone opens it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pairing {
    pub code: String,
    /// The app with the code, or "" when serve can't say where it's reached.
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base: String,
    pub expires: Time,
    /// The command an SSH client connects with, while remote-ssh is on.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ssh: String,
    /// serve's SSH host key's fingerprint, to check it by.
    #[serde(rename = "hostKey", skip_serializing_if = "String::is_empty")]
    pub host_key: String,
}

/// A code's letters: none that read as another.
const PAIR_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// How many wrong codes void every live one: a code is 40 bits, short
/// enough to type on a phone, so guessing is cut off instead.
const PAIR_MISSES: u32 = 10;

/// The live codes, each with when it stops working.
#[derive(Default)]
pub struct PairCodes {
    inner: Mutex<(HashMap<String, Time>, u32)>,
}

impl PairCodes {
    /// A new code, and when it stops working.
    pub fn issue(&self) -> (String, Time) {
        let code: String = rand_bytes(8)
            .into_iter()
            .map(|b| PAIR_ALPHABET[(b % 32) as usize] as char)
            .collect();
        let now = Time::now();
        let until = now.add_nanos(PAIR_TTL_NS);
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.0.retain(|_, t| !now.after(*t));
        g.0.insert(code.clone(), until);
        (code, until)
    }

    /// Whether code is a live one, using it up if so. It's read as typed:
    /// any case, with or without its dash or spaces.
    pub fn take(&self, code: &str) -> bool {
        let code = code.replace(['-', ' '], "").to_uppercase();
        if code.is_empty() {
            return false;
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let hit =
            g.0.keys()
                .find(|c| same(c.as_bytes(), code.as_bytes()))
                .cloned();
        if let Some(c) = hit {
            let t = g.0.remove(&c).unwrap_or_default();
            return Time::now().before(t);
        }
        g.1 += 1;
        if g.1 >= PAIR_MISSES {
            g.0.clear();
            g.1 = 0;
        }
        false
    }
}

/// A code as it's shown, in two halves.
pub fn pair_code(c: &str) -> String {
    if c.len() != 8 || !c.is_ascii() {
        return c.to_string();
    }
    format!("{}-{}", &c[..4], &c[4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_works_once_as_typed() {
        let p = PairCodes::default();
        let (c, _) = p.issue();
        assert_eq!(c.len(), 8);
        assert!(c.bytes().all(|b| PAIR_ALPHABET.contains(&b)));
        let typed = pair_code(&c).to_lowercase().replace('-', " - ");
        assert!(p.take(&typed));
        assert!(!p.take(&c));
        assert!(!p.take(""));
    }

    /// Ten wrong guesses void every live code (sshd_test.go
    /// TestPairCodesStopGuessing).
    #[test]
    fn codes_stop_guessing() {
        let p = PairCodes::default();
        let (c, _) = p.issue();
        for _ in 0..PAIR_MISSES {
            assert!(!p.take("AAAAAAAA"));
        }
        assert!(!p.take(&c));
        assert_eq!(pair_code("ABCDEFGH"), "ABCD-EFGH");
        assert_eq!(pair_code("ABC"), "ABC");
    }
}
