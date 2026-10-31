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
            // A memory file with unsaved edits asks first (keys.go:60-71).
            if self.confirm.is_none()
                && let Some(path) = crate::screens::session_core::memedit::dirty_path(self)
            {
                let q = super::confirm::Confirm::new(
                    format!(
                        "Quit with unsaved changes to {}?",
                        crate::style::tildify(&path)
                    ),
                    |_| Out::Quit,
                )
                .detail("they're lost");
                self.ask(q);
                return Out::Draw;
            }
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
        // A grid tile's box has the keys: its Session takes them where it
        // is (keys.go:137 gridKey).
        if let Some(o) = crate::screens::list_extras::grid::key(self, k) {
            return o;
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
    pub(crate) fn session_press(&mut self, k: &Press) -> Out {
        {
            for i in 0..self.ext.session_keys.len() {
                let f = self.ext.session_keys[i];
                let o = f(self, k);
                if o.used() {
                    return o;
                }
            }
            if k.s == "ctrl+c" {
                return self.quit_key();
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
                // Kept to undo and in the stash's history (keys.go:151
                // clearPrompt).
                self.wipe_prompt();
                return Out::Draw;
            }
            return self.quit_key();
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
            "ctrl+c" => return self.quit_key(),
            _ => return Out::Idle,
        };
        if self.step_page(d) {
            Out::Draw
        } else {
            Out::Idle
        }
    }

    /// ctrl+c with nothing to clear or copy: the first arms quitting, a
    /// second within 2s quits (keys.go:650 quitKey).
    pub(crate) fn quit_key(&mut self) -> Out {
        if rush_core::time::now() - self.quit_armed < 2000 {
            return Out::Quit;
        }
        self.quit_armed = rush_core::time::now();
        self.flash("ctrl+c again to quit");
        Out::Draw
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
                if k.s == "space" && self.input.anchor.is_none() {
                    // A path to an image, typed or dropped in as keys,
                    // becomes an attachment once it's done (keys.go:538).
                    self.inline_typed();
                }
                Out::Draw
            }
            None => Out::Pass,
        }
    }

    /// Image files named in the Agents box's text, typed or dropped in as
    /// keys, as [Image #N] where they were; the cursor at the end
    /// (keys.go:538-544).
    pub fn inline_typed(&mut self) {
        let text = self.input.string();
        if let Some(t) = self.imgs.inline(&text, &super::editor::look_disk) {
            self.input.text = t.chars().collect();
            self.input.pos = self.input.text.len();
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
        // # and a letter: a Markdown heading (# Plan) or an issue (#123)
        // is still a message (keys.go:841 isHashCmd).
        if crate::screens::cmdbar::fleetslash::is_hash_cmd(t) {
            return self.command(t);
        }
        // /stop typed the old way runs #stop (keys.go:847 legacyCommand).
        if t.starts_with('/')
            && let Some(o) = crate::screens::cmdbar::fleetslash::legacy_command(self, t)
        {
            return o;
        }
        let tagged = self.input.sent(true);
        let t = tagged.trim();
        for i in 0..self.ext.submit.len() {
            let f = self.ext.submit[i];
            let o = f(self, t);
            if o.used() {
                self.input.clear_sent();
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
        self.flash(&format!(
            "cleared · {} brings it back",
            crate::screens::social::parts::UNDO_HINT
        ));
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
        // Other names a command answers to: #twotter is #feed, #history
        // #stash (keys.go:884-889).
        let to = crate::screens::cmdbar::fleetslash::alias_of(&name);
        if to != name {
            if matches!(name.as_str(), "agent" | "handoff" | "profile") {
                self.flash(&format!("#{name} is now called #{to}"));
            }
            name = to.to_string();
        }
        // In a Session its box's commands act on its own agent, whatever
        // the list has picked (command's a, keys.go:875-876).
        if let Some(key) = self.session.as_ref().map(|v| v.key.clone())
            && self.sel != key
            && self.agents.iter().any(|a| a.key == key)
        {
            self.select(key);
        }
        if let Some(c) = self.ext.commands.iter().find(|c| c.name == name).copied() {
            // Typed in the Agents box, it's sent; from a Session's box,
            // what's in this one stays, kept for undo if it goes.
            if self.input.string().trim() == text.trim() {
                self.input.clear_sent();
            } else {
                self.input.clear();
            }
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

    /// A paste: hooks, then what has the keys; undo takes it back, and
    /// brings back what it was pasted over.
    pub fn paste(&mut self, text: &str) -> Out {
        self.guarded(|m| m.paste_to(text))
    }

    fn paste_to(&mut self, text: &str) -> Out {
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
            // Image files dropped onto the terminal arrive as a paste of
            // their paths: each becomes [Image #N] where it was dropped
            // (model.go:1424). A long one shows as a chip and goes out
            // whole; a short one is made one line (model.go:1449-1456).
            let mut text = super::editor::clean_paste(text);
            if let Some(t) = self.imgs.inline(&text, &super::editor::look_disk) {
                text = t;
            }
            self.input.paste(&text, false);
            return Out::Draw;
        }
        Out::Idle
    }

    /// The mouse: hooks, then a box over the screen, the header's tabs,
    /// then the screen showing (Update's mouse messages).
    pub fn mouse_event(&mut self, e: &Mouse) -> Out {
        use crate::screens::list_extras::split;
        // The list's edge first: grabbed, it follows the mouse wherever
        // it goes (model.go:1528, :1658).
        let lit = match split::edge_mouse(self, e) {
            Ok(o) => return o,
            Err(changed) => changed,
        };
        let o = self.mouse_routed(e);
        match o {
            Out::Pass | Out::Idle if lit => Out::Draw,
            o => o,
        }
    }

    /// The mouse, the list's edge having passed (mouse_event).
    fn mouse_routed(&mut self, e: &Mouse) -> Out {
        use crate::screens::list_extras::split;
        let (x, y) = (e.x as usize, e.y as usize);
        // The grid's tiles and edge, where they are on screen, before the
        // Session beside the list takes the click at its own columns
        // (model.go:1496, :1589, :1640 gridDrag, gridClick).
        let o = crate::screens::list_extras::grid::mouse(self, e);
        if o.used() {
            return o;
        }
        // The pointer coming onto the Session or the list gives it the
        // keys, once as it crosses (model.go:1556 focusAt).
        let mut crossed = split::pointer_at(self, x, e.kind == MouseKind::Moved);
        if !self.on_place()
            && self.confirm.is_none()
            && self.sheet.is_none()
            && y >= self.top_h()
            && matches!(e.kind, MouseKind::Down(Button::Left))
        {
            // A click on the Session gives it the keys, and is then its
            // own: in its box, the cursor goes there (model.go:1664
            // paneFocus, clickBox). One on the list gives the list the
            // keys (focusAt).
            crossed |= split::focus_at(self, x);
        }
        let o = self.mouse_split(e, x, y);
        match o {
            Out::Pass | Out::Idle if crossed => Out::Draw,
            o => o,
        }
    }

    /// The mouse on Agents and the Session beside it (mouse_routed).
    fn mouse_split(&mut self, e: &Mouse, x: usize, y: usize) -> Out {
        use crate::screens::list_extras::split;
        if !self.on_place() && self.confirm.is_none() && self.sheet.is_none() && y >= self.top_h() {
            let (lw, pw) = split::widths(self);
            if self.session.is_some() && lw > 0 && pw > 0 && x > lw {
                // The opened Session beside the list: its parts are where
                // they were drawn, from the pane's left edge at its width.
                let at = Mouse {
                    x: (x - lw - 1) as u16,
                    ..*e
                };
                return split::in_pane(self, pw, |m| m.mouse_at(&at));
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
            let o = self
                .with_shown(|p, m| p.mouse(m, e, x.saturating_sub(2), y - top))
                .unwrap_or(Out::Idle);
            if o == Out::Close {
                // As a key's Close: back to Agents (the Wall's double
                // click opening a tile, wall.go:1066 wallClick).
                self.back_to_agents();
                return Out::Draw;
            }
            return o;
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
