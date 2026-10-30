//! A key from its name, and a Mac's Option letters read as alt. After Go's
//! internal/ui/keyname.go (keyOf :30) and macoption.go (macOption :22).

use rush_draw::{Key, KeyMods, key_string};

/// The named keys, as bubbletea names them.
const NAMED: &[(&str, Key)] = &[
    ("enter", Key::Enter),
    ("tab", Key::Tab),
    ("backspace", Key::Backspace),
    ("esc", Key::Esc),
    ("space", Key::Char(' ')),
    ("up", Key::Up),
    ("down", Key::Down),
    ("left", Key::Left),
    ("right", Key::Right),
    ("home", Key::Home),
    ("end", Key::End),
    ("pgup", Key::PageUp),
    ("pgdown", Key::PageDown),
    ("insert", Key::Insert),
    ("delete", Key::Delete),
];

/// The key whose name is s, as the keymap names it: how a key you bound
/// stands in for the one rush's handling expects. None when no key prints
/// as s.
pub fn key_of(s: &str) -> Option<(Key, KeyMods)> {
    let mut mods = KeyMods::NONE;
    let mut rest = s;
    loop {
        let (p, m) = if rest.len() > 5 && rest.starts_with("ctrl+") {
            ("ctrl+", KeyMods::CTRL)
        } else if rest.len() > 4 && rest.starts_with("alt+") {
            ("alt+", KeyMods::ALT)
        } else if rest.len() > 6 && rest.starts_with("shift+") {
            ("shift+", KeyMods::SHIFT)
        } else if rest.len() > 6 && rest.starts_with("super+") {
            ("super+", KeyMods::SUPER)
        } else {
            break;
        };
        mods = mods.with(m);
        rest = &rest[p.len()..];
    }
    let code = if let Some(&(_, k)) = NAMED.iter().find(|(n, _)| *n == rest) {
        k
    } else if let Some(n) = rest.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        Key::F(n)
    } else {
        let mut cs = rest.chars();
        match (cs.next(), cs.next()) {
            (Some(c), None) => Key::Char(c),
            _ => return None,
        }
    };
    (key_string(code, mods) == s).then_some((code, mods))
}

/// The letters a US Mac keyboard's Option key types in place of the
/// letter, when the terminal doesn't send Option as Meta.
const OPTION_KEYS: &[(char, char)] = &[
    ('å', 'a'),
    ('∫', 'b'),
    ('ç', 'c'),
    ('∂', 'd'),
    ('ƒ', 'f'),
    ('©', 'g'),
    ('˙', 'h'),
    ('∆', 'j'),
    ('˚', 'k'),
    ('¬', 'l'),
    ('µ', 'm'),
    ('ø', 'o'),
    ('π', 'p'),
    ('œ', 'q'),
    ('®', 'r'),
    ('ß', 's'),
    ('†', 't'),
    ('√', 'v'),
    ('∑', 'w'),
    ('≈', 'x'),
    ('¥', 'y'),
    ('Ω', 'z'),
];

/// The key as the ⌥ key it was on a Mac, so ⌥m picks what starts whether
/// or not the terminal sends Option as Meta. US layout only, and those
/// characters can't be typed into rush on a Mac, as in Go.
pub fn mac_option(code: Key, mods: KeyMods) -> (Key, KeyMods) {
    mac_option_on(cfg!(target_os = "macos"), code, mods)
}

pub(crate) fn mac_option_on(mac: bool, code: Key, mods: KeyMods) -> (Key, KeyMods) {
    if !mac || !mods.is_empty() {
        return (code, mods);
    }
    if let Key::Char(c) = code
        && let Some(&(_, l)) = OPTION_KEYS.iter().find(|(o, _)| *o == c)
    {
        return (Key::Char(l), KeyMods::ALT);
    }
    (code, mods)
}
