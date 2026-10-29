//! Typing into the live pane: keys go to the session shown on the right
//! while the list stays on screen; ctrl+] hands the keyboard back. What a
//! key press was, as the bytes a terminal sends. After Go's
//! internal/ui/embed.go (namedKeys :40, keyBytes :53).

use crate::frame::Press;

/// What a terminal sends for the keys without text of their own
/// (namedKeys).
pub fn named(s: &str) -> Option<&'static str> {
    Some(match s {
        "enter" => "\r",
        "tab" => "\t",
        "shift+tab" => "\x1b[Z",
        "backspace" => "\x7f",
        "esc" => "\x1b",
        "space" => " ",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "pgup" => "\x1b[5~",
        "pgdown" => "\x1b[6~",
        "delete" => "\x1b[3~",
        "shift+enter" | "alt+enter" => "\x1b\r",
        "shift+backspace" => "\x7f",
        "shift+space" => " ",
        // Editing keys a terminal leaves to the shell's line editor, sent
        // as the readline keys Claude Code's prompt understands.
        "alt+backspace" => "\x1b\x7f",
        "ctrl+backspace" => "\x17",
        "super+backspace" => "\x15",
        "super+delete" => "\x0b",
        "alt+left" | "ctrl+left" => "\x1bb",
        "alt+right" | "ctrl+right" => "\x1bf",
        "super+left" => "\x01",
        "super+right" => "\x05",
        _ => return None,
    })
}

/// A key press back as what a terminal would have sent (keyBytes).
pub fn key_bytes(k: &Press) -> Vec<u8> {
    let s = k.s.replace("meta+", "super+");
    if let Some(b) = named(&s) {
        return b.as_bytes().to_vec();
    }
    if let Some(rest) = s.strip_prefix("ctrl+")
        && rest.len() == 1
        && rest.as_bytes()[0].is_ascii_lowercase()
    {
        return vec![rest.as_bytes()[0] - b'a' + 1];
    }
    if let Some(rest) = s.strip_prefix("alt+")
        && rest.chars().count() == 1
    {
        return format!("\x1b{rest}").into_bytes();
    }
    if let Some(c) = k.text() {
        return c.to_string().into_bytes();
    }
    Vec::new()
}

/// A paste as a terminal with bracketed paste sends it (embedPaste).
pub fn paste_bytes(text: &str) -> Vec<u8> {
    format!("\x1b[200~{text}\x1b[201~").into_bytes()
}
