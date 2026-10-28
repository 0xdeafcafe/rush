//! How full a session's context is, against the window it compacts
//! within. After Go's internal/agent/window.go.

/// The window assumed for a model no agent says the size of.
pub const DEFAULT_WINDOW: i64 = 200_000;

/// Where an agent compacts a session's context on its own, when its
/// settings or environment set a window smaller than the model's. The
/// default Compaction is none: the model's window is the one that counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compaction {
    /// The window it compacts within; the model's own caps it.
    pub window: i64,
    /// How far short of window compaction starts.
    pub headroom: i64,
}

/// How full a session's context is: used tokens of window, the window it
/// compacts within, which is model's unless a Compaction makes it smaller;
/// at is where it compacts, zero when only the model's window stops it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fill {
    pub used: i64,
    pub window: i64,
    pub model: i64,
    pub at: i64,
}

/// used tokens against a model's window of model tokens, and c.
pub fn fill_of(used: i64, model: i64, c: Compaction) -> Fill {
    let mut f = Fill {
        used,
        window: model,
        model,
        at: 0,
    };
    if c.window > 0 && c.window < model {
        f.window = c.window;
        f.at = 0.max(c.window - c.headroom);
    }
    f
}

impl Fill {
    /// used as a percentage of window.
    pub fn pct(&self) -> f64 {
        if self.window <= 0 {
            return 0.0;
        }
        self.used as f64 / self.window as f64 * 100.0
    }

    /// Whether a window of its own, smaller than the model's, counts.
    pub fn compacts(&self) -> bool {
        self.window < self.model
    }

    /// What's filled, for after its percentage, tok writing a count of
    /// tokens: "520k of 1M"; with a window of its own, "520k of 400k ·
    /// auto-compacts at 367k · model 1M".
    pub fn of(&self, tok: impl Fn(i64) -> String) -> String {
        let mut s = format!("{} of {}", tok(self.used), tok(self.window));
        if !self.compacts() {
            return s;
        }
        if self.at > 0 {
            s += &format!(" · auto-compacts at {}", tok(self.at));
        }
        s + &format!(" · model {}", tok(self.model))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill() {
        let tok = |n: i64| format!("{}k", n / 1000);
        let f = fill_of(520_000, 1_000_000, Compaction::default());
        assert!(
            !f.compacts() && f.pct() == 52.0 && f.of(tok) == "520k of 1000k",
            "{f:?}"
        );
        let f = fill_of(
            520_000,
            1_000_000,
            Compaction {
                window: 400_000,
                headroom: 33_000,
            },
        );
        assert!(f.compacts() && f.pct() == 130.0 && f.at == 367_000, "{f:?}");
        assert_eq!(
            f.of(tok),
            "520k of 400k · auto-compacts at 367k · model 1000k"
        );
        // The model's own window caps it.
        let f = fill_of(
            100_000,
            200_000,
            Compaction {
                window: 400_000,
                headroom: 33_000,
            },
        );
        assert!(!f.compacts() && f.window == 200_000, "{f:?}");
    }
}
