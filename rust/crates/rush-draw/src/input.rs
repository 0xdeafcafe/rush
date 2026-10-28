//! rush's own input events, translated from crossterm's. Key names follow
//! bubbletea v2 (ultraviolet key.go:391 `String`, :412 `Keystroke`, :459
//! the names), so Go rush's key matches port as they are: "ctrl+c",
//! "shift+tab", "pgdown", "space", "?".

use crossterm::event as ct;

/// A key, without its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

/// Modifier keys held, as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeyMods(pub u8);

impl KeyMods {
    pub const NONE: KeyMods = KeyMods(0);
    pub const SHIFT: KeyMods = KeyMods(1);
    pub const CTRL: KeyMods = KeyMods(1 << 1);
    pub const ALT: KeyMods = KeyMods(1 << 2);
    pub const SUPER: KeyMods = KeyMods(1 << 3);
    pub const META: KeyMods = KeyMods(1 << 4);
    pub const HYPER: KeyMods = KeyMods(1 << 5);

    pub const fn contains(self, o: KeyMods) -> bool {
        self.0 & o.0 == o.0
    }
    pub const fn with(self, o: KeyMods) -> KeyMods {
        KeyMods(self.0 | o.0)
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// What the mouse did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseKind {
    Down(Button),
    Up(Button),
    Drag(Button),
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

/// One thing from the terminal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    Key {
        code: Key,
        mods: KeyMods,
    },
    Mouse {
        kind: MouseKind,
        x: u16,
        y: u16,
        mods: KeyMods,
    },
    Resize(u16, u16),
    Paste(String),
    FocusGained,
    FocusLost,
}

impl Event {
    pub const fn key(code: Key) -> Event {
        Event::Key {
            code,
            mods: KeyMods::NONE,
        }
    }
    pub const fn key_with(code: Key, mods: KeyMods) -> Event {
        Event::Key { code, mods }
    }

    /// The key as bubbletea names it (`KeyPressMsg.String()`): typed text
    /// as itself, anything else as its keystroke. None when not a key.
    pub fn key_string(&self) -> Option<String> {
        match self {
            Event::Key { code, mods } => Some(key_string(*code, *mods)),
            _ => None,
        }
    }
}

/// The key as bubbletea's `Key.String()`: a char typed without ctrl or alt
/// is its own text ("A", "?"), except space; anything else is its
/// keystroke ("ctrl+c", "shift+tab", "enter").
pub fn key_string(code: Key, mods: KeyMods) -> String {
    if let Key::Char(c) = code
        && c != ' '
        && !mods.contains(KeyMods::CTRL)
        && !mods.contains(KeyMods::ALT)
        && !mods.contains(KeyMods::SUPER)
    {
        return c.to_string();
    }
    keystroke(code, mods)
}

/// The keystroke as bubbletea's `Key.Keystroke()`: modifiers in the order
/// ctrl, alt, shift, meta, hyper, super, then the key's name.
pub fn keystroke(code: Key, mods: KeyMods) -> String {
    let mut s = String::new();
    for (m, name) in [
        (KeyMods::CTRL, "ctrl+"),
        (KeyMods::ALT, "alt+"),
        (KeyMods::SHIFT, "shift+"),
        (KeyMods::META, "meta+"),
        (KeyMods::HYPER, "hyper+"),
        (KeyMods::SUPER, "super+"),
    ] {
        if mods.contains(m) {
            s.push_str(name);
        }
    }
    match code {
        Key::Char(' ') => s.push_str("space"),
        // A shifted letter is named by its key, as on a PC-101 layout.
        Key::Char(c) if mods.contains(KeyMods::SHIFT) => s.extend(c.to_lowercase()),
        Key::Char(c) => s.push(c),
        Key::F(n) => {
            s.push('f');
            s.push_str(&n.to_string());
        }
        k => s.push_str(match k {
            Key::Enter => "enter",
            Key::Tab => "tab",
            Key::Backspace => "backspace",
            Key::Esc => "esc",
            Key::Up => "up",
            Key::Down => "down",
            Key::Left => "left",
            Key::Right => "right",
            Key::Home => "home",
            Key::End => "end",
            Key::PageUp => "pgup",
            Key::PageDown => "pgdown",
            Key::Insert => "insert",
            Key::Delete => "delete",
            Key::Char(_) | Key::F(_) => unreachable!(),
        }),
    }
    s
}

fn key_mods(m: ct::KeyModifiers) -> KeyMods {
    let mut o = KeyMods::NONE;
    for (from, to) in [
        (ct::KeyModifiers::SHIFT, KeyMods::SHIFT),
        (ct::KeyModifiers::CONTROL, KeyMods::CTRL),
        (ct::KeyModifiers::ALT, KeyMods::ALT),
        (ct::KeyModifiers::SUPER, KeyMods::SUPER),
        (ct::KeyModifiers::META, KeyMods::META),
        (ct::KeyModifiers::HYPER, KeyMods::HYPER),
    ] {
        if m.contains(from) {
            o = o.with(to);
        }
    }
    o
}

fn button(b: ct::MouseButton) -> Button {
    match b {
        ct::MouseButton::Left => Button::Left,
        ct::MouseButton::Right => Button::Right,
        ct::MouseButton::Middle => Button::Middle,
    }
}

/// crossterm's event as rush's; None for ones rush doesn't use (key
/// releases, keys it has no name for).
pub(crate) fn translate(e: ct::Event) -> Option<Event> {
    Some(match e {
        ct::Event::Key(k) => {
            if k.kind == ct::KeyEventKind::Release {
                return None;
            }
            let mut mods = key_mods(k.modifiers);
            let code = match k.code {
                ct::KeyCode::Char(c) => Key::Char(c),
                ct::KeyCode::Enter => Key::Enter,
                ct::KeyCode::Tab => Key::Tab,
                // bubbletea has no back tab: it's shift+tab.
                ct::KeyCode::BackTab => {
                    mods = mods.with(KeyMods::SHIFT);
                    Key::Tab
                }
                ct::KeyCode::Backspace => Key::Backspace,
                ct::KeyCode::Esc => Key::Esc,
                ct::KeyCode::Up => Key::Up,
                ct::KeyCode::Down => Key::Down,
                ct::KeyCode::Left => Key::Left,
                ct::KeyCode::Right => Key::Right,
                ct::KeyCode::Home => Key::Home,
                ct::KeyCode::End => Key::End,
                ct::KeyCode::PageUp => Key::PageUp,
                ct::KeyCode::PageDown => Key::PageDown,
                ct::KeyCode::Insert => Key::Insert,
                ct::KeyCode::Delete => Key::Delete,
                ct::KeyCode::F(n) => Key::F(n),
                _ => return None,
            };
            Event::Key { code, mods }
        }
        ct::Event::Mouse(m) => {
            let kind = match m.kind {
                ct::MouseEventKind::Down(b) => MouseKind::Down(button(b)),
                ct::MouseEventKind::Up(b) => MouseKind::Up(button(b)),
                ct::MouseEventKind::Drag(b) => MouseKind::Drag(button(b)),
                ct::MouseEventKind::Moved => MouseKind::Moved,
                ct::MouseEventKind::ScrollUp => MouseKind::ScrollUp,
                ct::MouseEventKind::ScrollDown => MouseKind::ScrollDown,
                ct::MouseEventKind::ScrollLeft => MouseKind::ScrollLeft,
                ct::MouseEventKind::ScrollRight => MouseKind::ScrollRight,
            };
            Event::Mouse {
                kind,
                x: m.column,
                y: m.row,
                mods: key_mods(m.modifiers),
            }
        }
        ct::Event::Resize(w, h) => Event::Resize(w, h),
        ct::Event::Paste(s) => Event::Paste(s),
        ct::Event::FocusGained => Event::FocusGained,
        ct::Event::FocusLost => Event::FocusLost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keys_like_bubbletea() {
        let (n, s, c, a) = (KeyMods::NONE, KeyMods::SHIFT, KeyMods::CTRL, KeyMods::ALT);
        let cases = [
            (Key::Char('a'), n, "a"),
            (Key::Char('A'), s, "A"),
            (Key::Char('?'), s, "?"),
            (Key::Char(' '), n, "space"),
            (Key::Char('c'), c, "ctrl+c"),
            (Key::Char('x'), a, "alt+x"),
            (Key::Char('A'), c.with(s), "ctrl+shift+a"),
            (Key::Char('é'), n, "é"),
            (Key::Enter, n, "enter"),
            (Key::Enter, s, "shift+enter"),
            (Key::Tab, s, "shift+tab"),
            (Key::Esc, n, "esc"),
            (Key::PageDown, n, "pgdown"),
            (Key::PageUp, c, "ctrl+pgup"),
            (Key::F(5), n, "f5"),
            (Key::Left, a.with(c), "ctrl+alt+left"),
        ];
        for (k, m, want) in cases {
            assert_eq!(key_string(k, m), want, "{k:?} {m:?}");
        }
    }

    #[test]
    fn translates_crossterm() {
        use ct::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let press = |code, m| {
            ct::Event::Key(KeyEvent {
                code,
                modifiers: m,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            })
        };
        assert_eq!(
            translate(press(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Event::key_with(Key::Tab, KeyMods::SHIFT))
        );
        assert_eq!(
            translate(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Event::key_with(Key::Char('c'), KeyMods::CTRL))
        );
        let release = ct::Event::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        assert_eq!(translate(release), None);
        let m = ct::Event::Mouse(ct::MouseEvent {
            kind: ct::MouseEventKind::Down(ct::MouseButton::Left),
            column: 3,
            row: 4,
            modifiers: KeyModifiers::ALT,
        });
        assert_eq!(
            translate(m),
            Some(Event::Mouse {
                kind: MouseKind::Down(Button::Left),
                x: 3,
                y: 4,
                mods: KeyMods::ALT
            })
        );
        assert_eq!(
            translate(ct::Event::Resize(80, 24)),
            Some(Event::Resize(80, 24))
        );
        assert_eq!(translate(ct::Event::FocusLost), Some(Event::FocusLost));
    }
}
