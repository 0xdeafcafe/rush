//! What a Claude Code agent's own screen shows that its transcript
//! doesn't: the working line (elapsed time and tokens), the todo list
//! under it, and the status lines under the prompt. After Go's
//! internal/ui/screeninfo.go (screenInfo :13, readScreen :28).

use std::sync::LazyLock;

use regex::Regex;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenInfo {
    pub working: String,
    pub todos: Vec<String>,
    pub status: Vec<String>,
}

static SPIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[·✢✳✶✻✽*]\s+\S.*…").unwrap());
static TODO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[☐☒◼■□✓✔]").unwrap());
static RULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[─━╭╰]{3,}").unwrap());

/// Picks those out of the screen's lines (plain text). Claude Code draws
/// the prompt between two rules near the bottom; the working line and
/// todos sit above it, the status lines below (readScreen).
pub fn read_screen(lines: &[String]) -> ScreenInfo {
    let plain: Vec<&str> = lines.iter().map(|l| l.trim_end_matches(' ')).collect();
    // The prompt: the last line starting with ❯ or > under a rule.
    let mut prompt = None;
    for i in (1..plain.len()).rev() {
        let t = plain[i].trim_start_matches([' ', '│']);
        if (t.starts_with('❯') || t.starts_with('>')) && RULE.is_match(plain[i - 1]) {
            prompt = Some(i);
            break;
        }
    }
    let Some(prompt) = prompt else {
        return ScreenInfo::default();
    };
    let mut si = ScreenInfo::default();
    let top = prompt - 1; // the rule above it
    let lo = top.saturating_sub(14);
    for i in (lo..top).rev() {
        if SPIN.is_match(plain[i]) {
            si.working = plain[i].trim().to_string();
            for t in &plain[i + 1..top] {
                if TODO.is_match(t) {
                    si.todos
                        .push(t.trim().trim_start_matches(['⎿', ' ']).trim().to_string());
                }
            }
            break;
        }
    }
    // Under the prompt's closing rule.
    let mut below = prompt + 1;
    while below < plain.len() && !RULE.is_match(plain[below]) {
        below += 1;
    }
    for t in &plain[(below + 1).min(plain.len())..] {
        let t = t.trim();
        if !t.is_empty() {
            si.status.push(t.to_string());
        }
    }
    si
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_codes_screen() {
        let screen: Vec<String> = [
            "● Done the first part.",
            "",
            "✻ Thinking… (12s · ↓ 1.2k tokens)",
            "  ⎿  ☒ read the code",
            "     ☐ write the test",
            "",
            "╭──────────────────────────╮",
            "│ > fix it                 │",
            "╰──────────────────────────╯",
            "  ? for shortcuts",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let si = read_screen(&screen);
        assert_eq!(si.working, "✻ Thinking… (12s · ↓ 1.2k tokens)");
        assert_eq!(si.todos, ["☒ read the code", "☐ write the test"]);
        assert_eq!(si.status, ["? for shortcuts"]);
        assert_eq!(
            read_screen(&["just text".to_string()]),
            ScreenInfo::default()
        );
    }
}
