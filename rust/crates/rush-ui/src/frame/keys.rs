//! Where a key, a click and a paste go: hooks first, then whatever is
//! over the screen (a question, a sheet), the places, then the screen
//! showing. After Go's internal/ui/keys.go (key :22, placeStep :232,
//! submit :767, command :874), model.go (the mouse in Update :943,
//! acceptsText :1939, copyText in editor.go:284) and view.go:604 (render:
//! what's drawn on top of what).

use rush_core::text::Line;
use rush_draw::{Button, MouseKind};

use super::overlayclick::MouseEv;
use super::places::PlaceId;
use super::{Mouse, Out, Press};
use crate::model::Model;

impl Model {
    /// A key, through every binding (Go's key).
    pub fn press_key(&mut self, k: &Press) -> Out {
        // A failure on the line under the box has been seen once a key
        // comes (keyClearsFailure).
        self.key_clears_failure();
        // The keyboard takes over from the mouse.
        self.hover.clear();
        self.top_hover = None;
        for i in 0..self.ext.keys.len() {
            let f = self.ext.keys[i];
            let o = f(self, k);
            if o.used() {
                return o;
            }
        }
        if self.help && k.s != "ctrl+q" {
            self.help = false;
            return Out::Draw;
        }
        if k.s == "ctrl+q" {
            return Out::Quit;
        }
        if self.confirm.is_some() {
            return self.confirm_key(&k.s);
        }
        if self.sheet.is_some() {
            return self.sheet_key(k);
        }
        // , and . (or < and >) with nothing typed go to the previous and
        // next place; ctrl+\ from anywhere but a question being asked.
        let d = self.place_step(k);
        if d != 0 {
            self.step_place(d);
            return Out::Draw;
        }
        if self.on_place() {
            return self.place_key(k);
        }
        if self.session.is_some() {
            // Beside the list, the opened Session takes its keys laid out
            // at the pane's width, as it was drawn.
            let (lw, pw) = crate::screens::list_extras::split::widths(self);
            if lw > 0 && pw > 0 && !self.in_pane {
                return crate::screens::list_extras::split::in_pane(self, pw, |m| {
                    m.session_press(k)
                });
            }
            return self.session_press(k);
        }
        self.list_press(k)
    }

    /// A key for the opened Session: its screens' keys, then its own.
    fn session_press(&mut self, k: &Press) -> Out {
        {
            for i in 0..self.ext.session_keys.len() {
                let f = self.ext.session_keys[i];
                let o = f(self, k);
                if o.used() {
                    return o;
                }
            }
            if k.s == "ctrl+c" {
                return Out::Quit;
            }
            if k.s == "?" || k.s == "f1" {
                self.help = true;
                return Out::Draw;
            }
            self.session_key(&k.s).into()
        }
    }

    /// A key for Agents' list.
    fn list_press(&mut self, k: &Press) -> Out {
        if k.s == "ctrl+c" {
            if let Some((from, to)) = self.input.selection() {
                let t: String = self.input.text[from..to].iter().collect();
                self.input.anchor = None;
                self.copy_text(t);
                return Out::Draw;
            }
            if !self.input.is_empty() {
                self.input.clear();
                return Out::Draw;
            }
            return Out::Quit;
        }
        for i in 0..self.ext.list_keys.len() {
            let f = self.ext.list_keys[i];
            let o = f(self, k);
            if o.used() {
                return o;
            }
        }
        self.list_key(k)
    }

    /// A key for a place other than Agents' list: its own first, then tab
    /// and [ ] turn its pages, esc goes back to Agents.
    fn place_key(&mut self, k: &Press) -> Out {
        let o = self.with_shown(|p, m| p.key(m, k)).unwrap_or(Out::Pass);
        if o == Out::Close {
            self.back_to_agents();
            return Out::Draw;
        }
        if o.used() {
            return o;
        }
        let d = match k.s.as_str() {
            "tab" | "]" => 1,
            "shift+tab" | "[" => -1,
            "esc" => {
                self.back_to_agents();
                return Out::Draw;
            }
            "ctrl+c" => return Out::Quit,
            _ => return Out::Idle,
        };
        if self.step_page(d) {
            Out::Draw
        } else {
            Out::Idle
        }
    }

    /// esc from a place, or a page of Agents: Agents' plain list.
    fn back_to_agents(&mut self) {
        if self.place == PlaceId::Agents {
            self.set_page(0);
            return;
        }
        let from = self.place;
        self.set_place(PlaceId::Agents);
        self.land_on_needs(from);
    }

    /// The Agents box's own keys, once it's on screen (Ext::agents_box):
    /// enter sends what's typed, esc clears it, the rest edit it. Idle
    /// when the key wasn't one.
    pub(crate) fn box_key(&mut self, k: &Press) -> Out {
        if !self.ext.agents_box {
            return Out::Pass;
        }
        match k.s.as_str() {
            "enter" if !self.input.is_empty() => return self.submit(),
            "esc" if !self.input.is_empty() => {
                self.wipe_prompt();
                return Out::Draw;
            }
            _ => {}
        }
        match self.input.key(k) {
            Some(copied) => {
                if !copied.is_empty() {
                    self.copy_text(copied);
                }
                Out::Draw
            }
            None => Out::Pass,
        }
    }

    /// enter in the Agents box: a #command, or what the screens that send
    /// take (Ext::submit); the box empties once one has it (submit).
    pub fn submit(&mut self) -> Out {
        // Each paste's chip its text again; what starts a session or goes
        // to one has each between tags, as Claude Code sends a paste
        // (keys.go:771-772).
        let text = self.input.sent(false);
        let t = text.trim();
        if t.is_empty() {
            return Out::Idle;
        }
        if t.starts_with('#') {
            return self.command(t);
        }
        let tagged = self.input.sent(true);
        let t = tagged.trim();
        for i in 0..self.ext.submit.len() {
            let f = self.ext.submit[i];
            let o = f(self, t);
            if o.used() {
                self.input.clear();
                // For the stash's history's Sent (keys.go:790).
                crate::screens::plugin_ui::hooks::emit_box(self, rush_plugin::EV_INPUT_SENT, "", t);
                return o;
            }
        }
        self.flash_err("rush can't start agents from here yet");
        Out::Draw
    }

    /// Clears the Agents box, keeping what was in it to undo and in the
    /// stash's history (stash.go:187 wipePrompt).
    pub fn wipe_prompt(&mut self) {
        if self.input.is_empty() {
            return;
        }
        let text = self.input.string();
        let e = &mut self.input;
        e.undo.save(&e.text, e.pos, false);
        self.input.clear();
        crate::screens::plugin_ui::hooks::cleared(self, "", &text);
    }

    /// Runs a #command typed: its name lower-cased, #view:list as #view
    /// list (command).
    pub fn command(&mut self, text: &str) -> Out {
        let first = text.split_whitespace().next().unwrap_or("");
        let mut name = first.trim_start_matches(['#', '/']).to_lowercase();
        let mut arg = text[first.len()..].trim().to_string();
        if let Some((n, r)) = name.clone().split_once(':')
            && arg.is_empty()
        {
            name = n.to_string();
            arg = r.to_string();
        }
        if let Some(c) = self.ext.commands.iter().find(|c| c.name == name).copied() {
            self.input.clear();
            let o = (c.run)(self, &arg);
            return if o == Out::Quit { o } else { Out::Draw };
        }
        for i in 0..self.ext.hash.len() {
            let f = self.ext.hash[i];
            let o = f(self, text);
            if o.used() {
                return o;
            }
        }
        self.flash_err(&format!("unknown command #{name} · # lists rush's"));
        Out::Draw
    }

    /// Puts text on the clipboard with the next frame (copyText).
    pub fn copy_text(&mut self, t: String) {
        let n = t.chars().count();
        self.pending_copy = Some(t);
        self.flash(&format!("copied {n} characters"));
    }

    /// A paste: hooks, then what has the keys.
    pub fn paste(&mut self, text: &str) -> Out {
        for i in 0..self.ext.paste.len() {
            let f = self.ext.paste[i];
            let o = f(self, text);
            if o.used() {
                return o;
            }
        }
        if self.confirm.is_some() {
            return Out::Idle;
        }
        if self.sheet.is_some() {
            return self.sheet_paste(text);
        }
        if self.on_place() {
            return self
                .with_shown(|p, m| p.paste(m, text))
                .unwrap_or(Out::Idle);
        }
        if self.session.is_none() && self.ext.agents_box {
            // A long one shows as a chip and goes out whole; a short one
            // is made one line (model.go:1449-1456).
            self.input.paste(&super::editor::clean_paste(text), false);
            return Out::Draw;
        }
        Out::Idle
    }

    /// The mouse: hooks, then a box over the screen, the header's tabs,
    /// then the screen showing (Update's mouse messages).
    pub fn mouse_event(&mut self, e: &Mouse) -> Out {
        use crate::screens::list_extras::split;
        let (x, y) = (e.x as usize, e.y as usize);
        if self.session.is_some()
            && !self.on_place()
            && self.confirm.is_none()
            && self.sheet.is_none()
            && y >= self.top_h()
        {
            let (lw, pw) = split::widths(self);
            if lw > 0 && pw > 0 {
                if x > lw {
                    // The opened Session beside the list: its parts are
                    // where they were drawn, from the pane's left edge
                    // at its width.
                    let at = Mouse {
                        x: (x - lw - 1) as u16,
                        ..*e
                    };
                    return split::in_pane(self, pw, |m| m.mouse_at(&at));
                }
                if x < lw && matches!(e.kind, MouseKind::Down(Button::Left)) {
                    // A click on the list gives it the keys (focusAt).
                    self.close();
                }
            }
        }
        self.mouse_at(e)
    }

    /// The mouse, laid out as the screen showing is (mouse_event).
    fn mouse_at(&mut self, e: &Mouse) -> Out {
        for i in 0..self.ext.mouse.len() {
            let f = self.ext.mouse[i];
            let o = f(self, e);
            if o.used() {
                return o;
            }
        }
        let (x, y) = (e.x as usize, e.y as usize);
        if self.confirm.is_some() || self.sheet.is_some() {
            let ev = match e.kind {
                MouseKind::ScrollUp => MouseEv::WheelUp,
                MouseKind::ScrollDown => MouseEv::WheelDown,
                MouseKind::Down(Button::Left) => MouseEv::Press,
                MouseKind::Drag(Button::Left) => MouseEv::Drag,
                MouseKind::Up(Button::Left) => MouseEv::Release,
                _ => return Out::Idle,
            };
            return match ev {
                MouseEv::Press => self.overlay_click(e),
                MouseEv::WheelUp | MouseEv::WheelDown => {
                    let o = if self.confirm.is_none() {
                        self.sheet_mouse(ev, e)
                    } else {
                        Out::Pass
                    };
                    if o.used() {
                        o
                    } else {
                        self.overlay_wheel(ev == MouseEv::WheelUp)
                    }
                }
                _ if self.confirm.is_none() => self.sheet_mouse(ev, e),
                _ => Out::Idle,
            };
        }
        if y < self.top_h() {
            match e.kind {
                MouseKind::Moved => {
                    let h = self.header_tab_at(x, y);
                    if h == self.top_hover {
                        return Out::Idle;
                    }
                    self.top_hover = h;
                    return Out::Draw;
                }
                MouseKind::Down(Button::Left) if self.click_tab(x, y) => return Out::Draw,
                _ => {}
            }
        } else if self.top_hover.take().is_some() && e.kind == MouseKind::Moved {
            return Out::Draw;
        }
        if self.on_place() {
            let top = self.top_h();
            if y < top || y >= self.h.saturating_sub(2) {
                return Out::Idle;
            }
            return self
                .with_shown(|p, m| p.mouse(m, e, x.saturating_sub(2), y - top))
                .unwrap_or(Out::Idle);
        }
        self.mouse(e.kind, e.x, e.y).into()
    }

    /// Lays what goes over the screen on the frame's rows, in Go's order:
    /// the hooks' floats, the sheet, the keys' help, the question.
    pub(crate) fn overlays(&mut self, out: &mut Vec<Line>) {
        self.over = Default::default();
        for i in 0..self.ext.overlays.len() {
            let f = self.ext.overlays[i];
            f(self, out);
        }
        if self.sheet.is_some() {
            self.sheet_view(out);
        }
        if self.help {
            self.help_over(out);
        }
        if self.confirm.is_some() {
            self.confirm_view(out);
        }
    }
}
