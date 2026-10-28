//! Drawing one turn: its heading (what you said, who answered), its items
//! as an open turn has them or a finished one's story, steps and runs of
//! them, and the live line. Port of internal/convo/render.go (drawer, add,
//! squeezed, head, open, drawItems, item, notice, interject, compacted,
//! liveLine, verb, groupWords, run, runGroups, gap, prose, answer, step,
//! cells, label, command, summary, body, pictures, shellBody, cmdRule,
//! errorLine, output, diff, diffLine, widen, addRows, hangRows),
//! history_settled.go (settled, story, speaker, batched, doc, docPreview,
//! settledRun), history_preview.go (folded), rails.go (railed, own,
//! putRail), userbox.go (userBox, askRows, pasteRows, otherAsk, chipRows),
//! denied.go, errcard.go, cards.go:1572-1639 (cardText, box),
//! messages.go:21 (messageTool), :202 (stopTool), :216 (toolLabel), :281
//! (toolSummary), views.go:60-140 (views, viewHint), hooks.go:58 (hooks)
//! and pastechip.go.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;
use rush_core::text::{Color, Line, Style};
use rush_core::time::Ms;
use rush_hl::Lang;

use super::code::{Emph, HlState, highlight, lang_for, pretty_json, pretty_text};
use super::shell::{
    CwdAt, OutSpan, Places, base_name, chain_glyph, chain_label, chain_spans, compound, drop_cd,
    echo_marks, mark_heading, phrases, program, quiet_tint, quiet_tokens, script_lines, segments,
    shell_lines, shell_shape, short_paths, subshell_lines, token_rows, truncate_cells,
};
use super::style::one_line;
use super::style::*;
use super::txt::{Txt, blanks, code_rows, row, width, wrap};
use super::udiff::{UDiff, clean_output, collapse_cr, expand_tabs, has_diff, parse_diff, unscreen};
use super::{CAP_ROW, Depth, GUTTER, Options, RenderedLine};
use crate::model::{Item, Session, Status, Step, StepId, Turn, agent_name};
use crate::text::{first_line, first_non_empty};
use crate::tool::{Call, Kind, Output};

fn re(s: &str) -> Regex {
    Regex::new(s).expect("a valid pattern")
}

/// A step read once a render: its call and how it came out.
pub(crate) struct Info {
    pub call: Call,
    pub out: Output,
}

/// Draws one turn.
pub(crate) struct Drawer<'a> {
    pub s: &'a Session,
    pub t: &'a Turn,
    pub o: &'a Options,
    pub p: &'a Places,
    pub r: String,
    pub cw: usize,
    pub lines: Vec<RenderedLine>,
    /// The session's newest step when it's in this turn.
    pub latest: Option<StepId>,
    /// Whether it's the session's first turn, which draws the hooks.
    pub first: bool,
    /// A timer under FAST_UNDER was drawn.
    pub fast: bool,
    named: bool,
    rule: bool,
    peek: bool,
    pub(crate) tool: i32,
    helper: i32,
    while_: bool,
    worked: bool,
    marks: HashSet<String>,
    lg: Option<&'static Lang>,
    by_path: bool,
    hs: HlState,
    hs_path: String,
    hs_n: usize,
    spans: Vec<OutSpan>,
    view: &'static str,
    nowrap: bool,
    infos: std::cell::RefCell<HashMap<StepId, Rc<Info>>>,
}

impl<'a> Drawer<'a> {
    pub fn new(s: &'a Session, t: &'a Turn, o: &'a Options, p: &'a Places) -> Drawer<'a> {
        Drawer {
            s,
            t,
            o,
            p,
            r: format!("t{}", t.n),
            cw: o.width.min(o.row_cap()),
            lines: Vec::new(),
            latest: None,
            first: false,
            fast: false,
            named: false,
            rule: false,
            peek: false,
            tool: 0,
            helper: 0,
            while_: false,
            worked: false,
            marks: HashSet::new(),
            lg: None,
            by_path: false,
            hs: HlState::default(),
            hs_path: String::new(),
            hs_n: 0,
            spans: Vec::new(),
            view: "",
            nowrap: false,
            infos: Default::default(),
        }
    }

    // ---- rows ----

    /// Column 0: the turn's quiet rule, by whose rows these are.
    fn spine(&self) -> Txt {
        if !self.rule || self.o.no_rule {
            Txt::raw(" ")
        } else if self.helper > 0 {
            paint(C_HELPER, "▍")
        } else if self.tool > 0 {
            paint(SPINE_TOOL, "│")
        } else {
            paint(SPINE_BAR, "▍")
        }
    }

    fn spine_text(&self) -> &'static str {
        if !self.rule || self.o.no_rule {
            ""
        } else if self.tool > 0 && self.helper == 0 {
            "│"
        } else {
            "▍"
        }
    }

    pub(crate) fn sp(&self, n: usize) -> Txt {
        self.spine().and(blanks(n))
    }

    /// Appends a row; the selected one gets the cursor in the gutter.
    pub(crate) fn add(&mut self, r: &str, bg: Color, left: Txt, right: Txt) {
        let mut left = left;
        if !r.is_empty() && r == self.o.selected {
            let sp = self.spine();
            if let (Some(a), Some(b)) = (left.0.first(), sp.0.first())
                && a.text == b.text
                && a.style == b.style
            {
                left.0.remove(0);
            }
            left = cursor().and(left);
        }
        let line = row(bg, left, right, self.o.width, self.cw);
        self.lines.push(RenderedLine {
            line,
            r#ref: r.to_string(),
            wrap: false,
            owner: String::new(),
        });
    }

    /// A row that may use the pane's whole width.
    fn add_wide(&mut self, r: &str, left: Txt) {
        let mut left = left;
        if !r.is_empty() && r == self.o.selected {
            let sp = self.spine();
            if let (Some(a), Some(b)) = (left.0.first(), sp.0.first())
                && a.text == b.text
            {
                left.0.remove(0);
            }
            left = cursor().and(left);
        }
        let line = row(Color::Default, left, Txt::new(), self.o.width, self.o.width);
        self.lines.push(RenderedLine {
            line,
            r#ref: r.to_string(),
            wrap: false,
            owner: String::new(),
        });
    }

    /// What Claude showed with rush's show tool: the drawing in a frame of
    /// its own, never wrapped (render.go:4165).
    fn figure(&mut self, id: StepId, r: &str, indent: usize) -> bool {
        let st = self.s.step(id);
        let Some(rows) = drawing(st) else {
            return false;
        };
        let v = input_value(st);
        let t = one_line(str_of(&v, "title"));
        let title = if t.is_empty() {
            "drawing".to_string()
        } else {
            t
        };
        let pad = self.sp(indent - 1);
        let wide = rows.iter().map(|r| width(r)).max().unwrap_or(0);
        let room = 8.max(self.o.width as isize - pad.width() as isize - 4) as usize;
        if let Some(l) = self.lines.last()
            && !trimmed_is(&l.line, self.spine_text())
        {
            self.blank();
        }
        let inner = wide.max(width(&title) + 6).min(room);
        let edge = |l: &str, rt: &str, label: Txt| -> Txt {
            let mut head = faint(&format!("{l}─"));
            if !label.is_empty() {
                head = head.and_raw(" ").and(label).and_raw(" ");
            }
            let mut fill = (inner + 3) as isize - head.width() as isize;
            if fill < 1 {
                head = head.truncate(inner + 1, &Txt::raw("…"));
                fill = (inner + 3) as isize - head.width() as isize;
            }
            pad.clone()
                .and(head)
                .and(faint(&format!("{}{rt}", "─".repeat(fill.max(0) as usize))))
        };
        let top = edge("╭", "╮", glyph_color("◇").and_raw(" ").and(text(&title)));
        self.add_wide(r, top);
        for row in &rows {
            let mut cell = paint(C_WHITE, row);
            if width(row) > inner {
                cell = cell.take(inner - 1).and(faint("›"));
            }
            let cw = cell.width();
            let l = pad
                .clone()
                .and(faint("│"))
                .and_raw(" ")
                .and(cell)
                .and(blanks(inner.saturating_sub(cw)))
                .and_raw(" ")
                .and(faint("│"));
            self.add_wide("", l);
        }
        let mut foot = Txt::new();
        if wide > inner {
            foot = dim(&key_word(&format!(
                "{} more columns · pick it, alt+c copies it whole",
                wide - inner
            )));
        }
        let bottom = edge("╰", "╯", foot);
        self.add_wide("", bottom);
        self.blank();
        true
    }

    /// A message step as its card: who it went to on its top edge, what it
    /// said inside, how it went at its foot (messages.go:144).
    fn message(&mut self, id: StepId, r: &str, indent: usize) {
        let st = self.s.step(id);
        let v = input_value(st);
        let body = first_non_empty(&[
            str_of(&v, "message"),
            str_of(&v, "text"),
            str_of(&v, "content"),
        ])
        .to_string();
        let gist = one_line(str_of(&v, "summary"));
        let room = self.cw.min(CAP_ROW) as isize - indent as isize - 4;
        if room < 24 {
            let l = self
                .sp(indent - 1)
                .and(self.status_mark(st))
                .and_raw(" ")
                .and(self.label(id));
            self.add(r, Color::Default, l, Txt::new());
            return;
        }
        let room = (room as usize).min(72);
        let (frame, mark) = match st.status {
            Status::Failed => (C_LOST_Q, C_RED),
            Status::Waiting => (C_WARN_Q, C_YELLOW),
            Status::Denied => (C_FAINT, C_DIM),
            _ => (C_FAINT, C_BLUE),
        };
        let mut head = if st.status != Status::Ok {
            self.status_mark(st).and_raw(" ")
        } else {
            Txt::new()
        };
        match &*st.tool {
            "SendMessage" => {
                let to = first_non_empty(&[str_of(&v, "to"), str_of(&v, "recipient")]);
                let to = recipient(to);
                head = head
                    .and(paint(mark, "→"))
                    .and_raw(" ")
                    .and(dim("to "))
                    .and(text(&to));
            }
            "SubagentHandback" => {
                head = head
                    .and(paint(mark, "↩"))
                    .and_raw(" ")
                    .and(dim("reported back"))
            }
            _ => {
                head = head
                    .and(paint(mark, "◉"))
                    .and_raw(" ")
                    .and(dim("notified you"))
            }
        }
        let open = self.o.verbose || self.o.open.get(r).copied().unwrap_or(false);
        let rows = if open {
            let mut rows = Vec::new();
            if !gist.is_empty() {
                rows = wrap(&paint(C_WHITE, &gist), room);
            }
            rows.extend(self.message_markdown(&body, room));
            rows
        } else if !gist.is_empty() {
            card_text(&gist, &body.split('\n').collect::<Vec<_>>(), room)
        } else {
            let b = body.trim();
            let (first, rest) = b.split_once('\n').unwrap_or((b, ""));
            card_text(first, &rest.split('\n').collect::<Vec<_>>(), room)
        };
        let foot = delivery(st);
        self.card(r, indent, head, Txt::new(), rows, foot, room, frame);
    }

    /// A message's words as markdown, laid out at the card's width.
    fn message_markdown(&self, body: &str, room: usize) -> Vec<Txt> {
        let o = Options {
            width: room + 2,
            verbose: self.o.verbose,
            ..Options::default()
        };
        let mut child = Drawer::new(self.s, self.t, &o, self.p);
        child.cw = room + 2;
        child.markdown(body.trim(), 1, C_SUB, true);
        child
            .lines
            .into_iter()
            .map(|l| {
                let mut t = Txt(l.line.spans).skip(1);
                while let Some(last) = t.0.last_mut() {
                    let n = last.text.trim_end_matches(' ').len();
                    if n == 0 {
                        t.0.pop();
                        continue;
                    }
                    last.text.truncate(n);
                    break;
                }
                t
            })
            .collect()
    }

    /// The row just added carries on the one before it.
    pub(crate) fn wrapped(&mut self) {
        if let Some(l) = self.lines.last_mut() {
            l.wrap = true;
        }
    }

    pub(crate) fn blank(&mut self) {
        let sp = self.spine();
        self.add("", Color::Default, sp, Txt::new());
    }

    /// A row with nothing on it, outside the turn's rule.
    fn bare(&mut self) {
        self.add("", Color::Default, Txt::new(), Txt::new());
    }

    fn is_blank(&self, i: usize) -> bool {
        let l = &self.lines[i];
        l.r#ref.is_empty() && trimmed_is(&l.line, self.spine_text())
    }

    /// A blank row, unless the last row already is one or there's none.
    pub(crate) fn gap(&mut self) {
        let n = self.lines.len();
        if n > 0 && !self.is_blank(n - 1) {
            self.blank();
        }
    }

    /// A row of space above what follows, unless the last row is one.
    fn air(&mut self) {
        if let Some(l) = self.lines.last()
            && trimmed_is(&l.line, self.spine_text())
        {
            return;
        }
        let sp = self.spine();
        self.add("", Color::Default, sp, Txt::new());
    }

    fn work(&mut self, f: impl FnOnce(&mut Self)) {
        self.tool += 1;
        f(self);
        self.tool -= 1;
    }

    /// The rows drawn since from are ref's (rails.go:44).
    pub(crate) fn own(&mut self, r: &str, from: usize) {
        let mut end = self.lines.len();
        while end > from && self.is_blank(end - 1) {
            end -= 1;
        }
        for l in &mut self.lines[from..end] {
            if l.owner.is_empty() {
                l.owner = r.to_string();
            } else if l.owner != r && !l.owner.starts_with(&format!("{r}{OWNER_SEP}")) {
                l.owner = format!("{r}{OWNER_SEP}{}", l.owner);
            }
        }
    }

    /// Runs a rail down column col beside the rows draw makes: ╭ on its
    /// header, │ down the rest, ╰ on the last (rails.go:16).
    fn railed(&mut self, col: usize, header: bool, draw: impl FnOnce(&mut Self)) {
        let from = self.lines.len();
        draw(self);
        let mut end = self.lines.len();
        while end > from && self.is_blank(end - 1) {
            end -= 1;
        }
        for i in from..end {
            let g = if i == from && header {
                "╭"
            } else if i == end - 1 {
                "╰"
            } else {
                "│"
            };
            put_rail(&mut self.lines[i].line, col, g);
        }
        if header && from < end && !self.lines[from].r#ref.is_empty() {
            let r = self.lines[from].r#ref.clone();
            self.own(&r, from);
        }
    }

    fn since(&mut self, t: Ms) -> String {
        let x = self.o.now - t;
        if x < FAST_UNDER {
            self.fast = true;
        }
        dur(x)
    }

    pub(crate) fn info(&self, id: StepId) -> Rc<Info> {
        if let Some(i) = self.infos.borrow().get(&id) {
            return i.clone();
        }
        let st = self.s.step(id);
        let i = Rc::new(Info {
            call: st.call(),
            out: st.out(),
        });
        self.infos.borrow_mut().insert(id, i.clone());
        i
    }

    /// Clean, routine steps a turn can fold into one row (render.go:2161).
    fn foldable(&self, id: StepId) -> bool {
        let st = self.s.step(id);
        if st.status != Status::Ok || hidden(st) || !st.images.is_empty() {
            return false;
        }
        if st.kind == Kind::Read && thumbable(&self.p.abs(&self.info(id).call.input.path)) {
            return false;
        }
        if matches!(glyph_for(st), "✎" | "⇉" | "◆" | "◇") {
            return false;
        }
        !message_tool(&st.tool)
    }

    fn step_ref(&self, st: &Step) -> String {
        format!("{}:s:{}", self.r, st.id)
    }

    // ---- the turn ----

    /// How the turn ended, when that's worth a glyph: a stop or a failure.
    fn mark(&self) -> Txt {
        if self.t.stopped {
            dim("⏹ ")
        } else if !self.t.err.is_empty() {
            paint(C_RED, "✗ ")
        } else {
            Txt::new()
        }
    }

    fn meta(&self) -> String {
        let t = self.t;
        let end = if t.live { self.o.now } else { t.end };
        let mut parts = Vec::new();
        let n = t.step_count();
        if n > 0 {
            parts.push(plural(n, "step"));
        }
        if t.start != 0 && end - t.start >= 100 {
            parts.push(dur(end - t.start));
        }
        let m = money(t.cost);
        if !m.is_empty() {
            parts.push(m);
        }
        parts.join("   ")
    }

    /// The start of a turn, open or folded: a gap, then what you said in its
    /// box, or the line for what woke it (render.go:686).
    fn head(&mut self) {
        let t = self.t;
        self.hooks();
        self.rule = true;
        self.bare();
        let mut imgs = Vec::new();
        if !t.images.is_empty() && !t.prompt.contains("[Image #") {
            imgs = image_names(&t.images);
        }
        let mut ask = fold_pastes(&t.prompt);
        if ask.is_empty() {
            ask = unasked(t).to_string();
        }
        let multi = t.command.contains('\n');
        if multi {
            ask.clear();
        }
        let (noun, how, wake) = woke(t);
        let yours = t.from.is_empty()
            && !wake
            && rush_ask(t).is_empty()
            && (!t.prompt.trim().is_empty() || !imgs.is_empty() || multi);
        if yours {
            let ask = if multi { "" } else { t.prompt.trim() };
            let r = self.r.clone();
            self.user_box(&r, ask, &imgs, multi);
        } else {
            self.other_ask(&ask, noun, how, wake);
        }
        if multi {
            let cmd = t.command.clone();
            self.shell_body(None, &cmd, GUTTER + 1);
        }
    }

    /// What start hooks said, at the top of the first turn (hooks.go:58).
    fn hooks(&mut self) {
        if !self.first {
            return;
        }
        for (i, h) in self.s.hooks.iter().enumerate() {
            let r = format!("{}:hook:{i}", self.r);
            let head = dim(&format!("◇ {} · {}", h.event, first_line(&h.text)));
            let open = self.o.open.get(&r).copied().unwrap_or(false);
            let lead = Txt::raw(" ").and(blanks(GUTTER - 3));
            if !open {
                self.add(
                    &r,
                    Color::Default,
                    lead.and(faint("▸ ")).and(head),
                    Txt::new(),
                );
                continue;
            }
            self.add(
                &r,
                Color::Default,
                lead.and(faint("▾ ")).and(head),
                Txt::new(),
            );
            let w = 10.max((self.cw.saturating_sub(GUTTER + 3)).min(self.o.prose_cap()));
            for l in h.text.split('\n') {
                for row in wrap(&dim(l.trim_end_matches(' ')), w) {
                    let left = Txt::raw(" ").and(blanks(GUTTER + 1)).and(row);
                    self.add(&r, Color::Default, left, Txt::new());
                }
            }
        }
    }

    /// Names the agent answering under what you said, and when it began.
    fn speaker(&mut self) {
        if self.t.items.is_empty() {
            return;
        }
        self.bare();
        let who = if self.o.speaker.is_empty() {
            "Agent"
        } else {
            &self.o.speaker
        };
        let mut l = self.spine().and_raw(" ").and(paint(C_ORANGE.bold(), who));
        if self.t.start != 0 {
            l.push(dim(&format!("  {}", clock(self.t.start, self.o.zone))));
        }
        if !self.o.agent.is_empty() {
            l.push(faint(" · "));
            l.push(paint(C_SUB, &self.o.agent));
        }
        self.add("", Color::Default, l, Txt::new());
    }

    /// An open turn: everything in it (render.go:735).
    pub fn open(&mut self) {
        self.head();
        self.speaker();
        let t = self.t;
        let items = &t.items;
        // A running turn keeps its last few steps in view.
        let mut keep = items.len();
        if t.live {
            let mut seen = 0;
            for i in (0..items.len()).rev() {
                if matches!(items[i], Item::Step(_)) {
                    seen += 1;
                    if seen == 3 {
                        keep = i;
                        break;
                    }
                }
            }
            if seen < 3 {
                keep = 0;
            }
        }
        if self.storied() {
            self.story();
        } else {
            self.draw_items(0, items.len(), keep);
        }
        if t.live && !self.o.hide_activity {
            self.live_line();
        }
        if t.stopped {
            let l = self.spine().and_raw("   ").and(dim("⏹ stopped"));
            self.add("", Color::Default, l, Txt::new());
        } else if !t.err.is_empty() && !t.live {
            let e = t.err.clone();
            self.err_card(&e);
        }
        // The rail stops at the last thing drawn.
        while self.lines.len() > 1 {
            let l = self.lines.last().unwrap();
            if l.r#ref.is_empty() && trimmed_is(&l.line, self.spine_text()) {
                self.lines.pop();
            } else {
                break;
            }
        }
        let m = self.meta();
        if !m.is_empty() && !t.live && t.step_count() > 0 {
            let sp = self.spine();
            self.add("", Color::Default, sp, dim(&m).and_raw("  "));
        }
    }

    /// A turn gone past, as its story (history_settled.go:17).
    pub fn settled(&mut self) {
        self.head();
        self.speaker();
        self.story();
        if self.t.stopped {
            let l = self.spine().and_raw("   ").and(dim("⏹ stopped"));
            self.add("", Color::Default, l, Txt::new());
        } else if !self.t.err.is_empty() {
            let l = self
                .sp(GUTTER - 1)
                .and(paint(C_RED, &format!("✗ {}", self.t.err)));
            self.add("", Color::Default, l, Txt::new());
        }
        let m = self.meta();
        if !m.is_empty() && self.t.step_count() > 0 {
            let sp = self.spine();
            self.add("", Color::Default, sp, dim(&m).and_raw("  "));
        }
    }

    /// A turn folded by hand: what you said, and one dim line.
    pub fn folded(&mut self) {
        self.head();
        let r = self.r.clone();
        if let Some(l) = self.lines.last_mut() {
            l.r#ref = r.clone();
        }
        let mut meta = self.meta().replace("   ", " · ");
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        let l = self
            .sp(GUTTER - 1)
            .and(faint("▸ "))
            .and(self.mark())
            .and(dim(&format!("{meta}show")));
        self.add(&r, Color::Default, l, Txt::new());
    }

    fn storied(&self) -> bool {
        !self.t.live
            && !waiting(self.s, self.t)
            && !self.o.verbose
            && self.o.depth == Depth::Default
    }

    /// A finished turn's body: its words, its batches, the documents it
    /// wrote (history_settled.go:40).
    fn story(&mut self) {
        let items = &self.t.items;
        let mut run: Vec<usize> = Vec::new();
        let mut at = 0;
        for (i, it) in items.iter().enumerate() {
            match it {
                Item::Step(id) if self.batched(*id) => {
                    if run.is_empty() {
                        at = i;
                    }
                    run.push(i);
                    continue;
                }
                Item::Thinking(_) => continue,
                _ => {}
            }
            self.flush_story(&mut run, at);
            self.item(i);
            if let Item::Step(id) = it
                && self.doc(*id)
            {
                self.doc_preview(*id);
            }
        }
        self.flush_story(&mut run, at);
    }

    fn flush_story(&mut self, run: &mut Vec<usize>, at: usize) {
        if run.len() == 1 {
            self.item(run[0]);
            run.clear();
        }
        if !run.is_empty() {
            let r = format!("{}:run:{at}", self.r);
            let ids: Vec<StepId> = run
                .iter()
                .filter_map(|&i| match self.t.items[i] {
                    Item::Step(id) => Some(id),
                    _ => None,
                })
                .collect();
            self.settled_run(&r, &ids);
            run.clear();
        }
    }

    /// Whether a finished turn's step folds into its batch.
    fn batched(&self, id: StepId) -> bool {
        let st = self.s.step(id);
        if matches!(glyph_for(st), "⇉" | "◆" | "◇") {
            return false;
        }
        if matches!(
            &*st.tool,
            "ScheduleWakeup"
                | "Monitor"
                | "PushNotification"
                | "SubagentHandback"
                | "EnterPlanMode"
                | "ExitPlanMode"
        ) {
            return false;
        }
        !hidden(st) && !message_tool(&st.tool) && !self.artifact(id) && !self.doc(id)
    }

    /// The file a step wrote whole and what it wrote.
    fn written(&self, id: StepId) -> (String, String) {
        let st = self.s.step(id);
        if st.status != Status::Ok {
            return Default::default();
        }
        match st.kind {
            Kind::Write => {
                let i = self.info(id);
                (self.p.rel(&i.call.input.path), i.call.input.content.clone())
            }
            Kind::Shell => {
                let i = self.info(id);
                let c = &i.call;
                let sh = shell_shape(self.p, &c.input.command);
                if sh.kind == "write" {
                    (sh.what, sh.body)
                } else {
                    Default::default()
                }
            }
            _ => Default::default(),
        }
    }

    /// A step that wrote a document, its last write in the turn.
    fn doc(&self, id: StepId) -> bool {
        let st = self.s.step(id);
        if !matches!(st.kind, Kind::Write | Kind::Shell) {
            return false;
        }
        let (path, _) = self.written(id);
        let ext = path
            .rsplit_once('.')
            .map(|x| x.1.to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(
            ext.as_str(),
            "md" | "markdown" | "mdx" | "txt" | "rst" | "org" | "adoc"
        ) {
            return false;
        }
        for it in self.t.items.iter().rev() {
            if let Item::Step(j) = it
                && self.written(*j).0 == path
            {
                return *j == id;
            }
        }
        false
    }

    fn doc_preview(&mut self, id: StepId) {
        const MOST: usize = 12;
        let st = self.s.step(id);
        let (_, content) = self.written(id);
        let r = self.step_ref(st);
        if content.trim().is_empty() || self.step_open(id, &r) {
            return;
        }
        let lines: Vec<&str> = content.trim().split('\n').collect();
        let mut head: Vec<&str> = lines[..lines.len().min(MOST)].to_vec();
        if head.join("\n").matches("```").count() % 2 == 1 {
            head.push("```");
        }
        self.markdown(&head.join("\n"), GUTTER + 2, C_SUB, false);
        let n = lines.len() - lines.len().min(MOST);
        if n > 0 {
            let l = self
                .sp(GUTTER + 1)
                .and(faint(&format!("… {}", plural(n, "more line"))));
            self.add("", Color::Default, l, Txt::new());
        }
        self.gap();
    }

    fn artifact(&self, id: StepId) -> bool {
        let st = self.s.step(id);
        !st.images.is_empty()
            || st.kind == Kind::Read && thumbable(&self.p.abs(&self.info(id).call.input.path))
            || st.tool.ends_with("__show")
    }

    /// A run of steps in a turn gone past, as one row (history_settled.go:202).
    fn settled_run(&mut self, r: &str, run: &[StepId]) {
        self.tool += 1;
        if self.o.open.get(r).copied().unwrap_or(false) {
            let r = r.to_string();
            self.railed(GUTTER - 2, true, |d| {
                let l = d
                    .sp(GUTTER - 1)
                    .and(faint(&format!("▾ hide {}", plural(run.len(), "step"))));
                d.add(&r, Color::Default, l, Txt::new());
                for &id in run {
                    d.step(id, 1);
                }
            });
            self.tool -= 1;
            return;
        }
        let mut files = HashSet::new();
        let (mut add, mut del, mut failed) = (0, 0, 0);
        for &id in run {
            let st = self.s.step(id);
            if st.status == Status::Failed || self.tests_failed(id) {
                failed += 1;
            }
            if st.status != Status::Ok || !matches!(st.kind, Kind::Edit | Kind::Write) {
                continue;
            }
            let i = self.info(id);
            files.insert(i.call.input.path.clone());
            if i.out.created {
                add += count_lines(&i.call.input.content);
            } else {
                for p in &i.out.patches {
                    for l in &p.lines {
                        if l.starts_with('+') {
                            add += 1;
                        } else if l.starts_with('-') {
                            del += 1;
                        }
                    }
                }
            }
        }
        let mut names = Txt::new();
        for (i, g) in self.run_groups(run).into_iter().enumerate() {
            if i > 0 {
                names.push(faint(", "));
            }
            names.push(if g.1 { paint(C_RED, &g.0) } else { faint(&g.0) });
        }
        let mark = if failed > 0 {
            paint(C_RED, "✗")
        } else {
            dim("✓")
        };
        let mut left = self.sp(GUTTER - 1).and(mark).and_raw(" ").and(names);
        if !files.is_empty() {
            left.push_str("  ", Style::default());
            left.push(paint(C_GREEN, &format!("+{add}")));
            if del > 0 {
                left.push(paint(C_RED, &format!(" −{del}")));
            }
            left.push(faint(&format!(" in {}", plural(files.len(), "file"))));
        }
        left.push(faint(&format!(" · {}", plural(run.len(), "step"))));
        if failed > 0 {
            left.push(faint(" · "));
            left.push(paint(C_RED, &format!("{failed} failed")));
        }
        self.worked = true;
        self.add(r, Color::Default, left, Txt::new());
        let mut cs = Vec::new();
        for &id in run {
            for c in self.step_cards(id).iter() {
                if !self.superseded(id, c.kind) {
                    cs.push(c.clone());
                }
            }
        }
        for c in super::cards::join_cards(cs) {
            self.card_of(&c, GUTTER + 2);
        }
        self.tool -= 1;
    }

    /// A tests or build card a later run in the turn tells again: looking
    /// back, only how the last one came out matters
    /// (history_settled.go:249).
    fn superseded(&self, id: StepId, kind: &str) -> bool {
        if kind != "tests" && kind != "build" {
            return false;
        }
        let mut after = false;
        for it in &self.t.items {
            let Item::Step(x) = *it else { continue };
            if x == id {
                after = true;
                continue;
            }
            if !after || self.s.step(x).kind != Kind::Shell {
                continue;
            }
            if super::cards::reruns(kind, &self.info(x).call.input.command) {
                return true;
            }
        }
        false
    }

    /// items[lo..hi] of an open turn: steps folded into runs as the depth
    /// has it (render.go:803).
    fn draw_items(&mut self, lo: usize, hi: usize, keep: usize) {
        let items = &self.t.items;
        let prose = !self.o.verbose && self.o.depth == Depth::Prose;
        let runs = !self.o.verbose && self.o.depth == Depth::Runs;
        let step_of = |i: usize| match items[i] {
            Item::Step(id) => Some(id),
            _ => None,
        };
        let mut i = lo;
        while i < hi {
            if prose && step_of(i).is_some() {
                let mut j = i;
                let mut failed = 0;
                while j < hi
                    && let Some(id) = step_of(j)
                {
                    if self.s.step(id).status == Status::Failed {
                        failed += 1;
                    }
                    j += 1;
                }
                if failed > 0 {
                    let r = format!("{}:run:{i}", self.r);
                    let n = j - i;
                    self.work(|d| {
                        let l = d.sp(GUTTER - 1).and(paint(
                            C_RED,
                            &format!("✗ {failed} of {} failed", plural(n, "step")),
                        ));
                        d.add(&r, Color::Default, l, Txt::new());
                    });
                }
                i = j;
                continue;
            }
            if !self.o.verbose && step_of(i).is_some() && i < keep {
                let mut j = i;
                while j < keep && j < hi {
                    let Some(id) = step_of(j) else { break };
                    let st = self.s.step(id);
                    if !((runs || self.foldable(id) && Some(id) != self.latest)
                        && !self.tests_failed(id)
                        && st.status != Status::Failed)
                    {
                        break;
                    }
                    j += 1;
                }
                if j - i >= 2 || runs && j > i {
                    let ids: Vec<StepId> = (i..j).filter_map(step_of).collect();
                    let r = format!("{}:run:{i}", self.r);
                    if !self.o.open.get(&r).copied().unwrap_or(false) {
                        self.run(&r, &ids);
                        // What the run did that you'd want to see stays
                        // out, what's about one change as one card.
                        self.run_cards(&ids, GUTTER + 2);
                    } else {
                        self.railed(GUTTER - 2, true, |d| {
                            d.work(|d| {
                                let l = d
                                    .sp(GUTTER - 1)
                                    .and(faint(&format!("▾ hide {}", plural(ids.len(), "step"))));
                                d.add(&r, Color::Default, l, Txt::new());
                            });
                            for &id in &ids {
                                d.step(id, 1);
                            }
                        });
                    }
                    i = j;
                    continue;
                }
            }
            self.item(i);
            i += 1;
        }
    }

    /// One thing in an open turn.
    fn item(&mut self, i: usize) {
        match &self.t.items[i] {
            Item::Text { text, answer } => {
                if *answer {
                    self.answer_item(text);
                } else {
                    self.gap();
                    self.prose(text, 2, C_SUB);
                    self.gap();
                }
            }
            Item::Thinking(text) => {
                if self.o.verbose {
                    let l = self.sp(GUTTER - 1).and(dim("✻ thought"));
                    self.add("", Color::Default, l, Txt::new());
                    if !text.trim().is_empty() {
                        self.prose(text, GUTTER + 2, C_DIM);
                    }
                }
            }
            Item::Compact { compact, summary } => {
                let label = paint(C_BLUE, "◇ context compacted");
                let mut facts = Vec::new();
                if !compact.trigger.is_empty() {
                    facts.push(compact.trigger.to_string());
                }
                if compact.before > 0 && compact.after > 0 {
                    facts.push(format!(
                        "{} → {} tokens",
                        tokens(compact.before),
                        tokens(compact.after)
                    ));
                } else if compact.before > 0 {
                    facts.push(format!("from {} tokens", tokens(compact.before)));
                }
                if !summary.is_empty() && !self.o.verbose {
                    facts.push("ctrl+o shows the summary".into());
                }
                let mut left = self.spine().and_raw("   ").and(faint("── ")).and(label);
                if !facts.is_empty() {
                    left.push(dim(&format!("  {}", facts.join(" · "))));
                }
                let fill = self.cw as isize - left.width() as isize - 2;
                left.push_str(" ", Style::default());
                left.push(faint(&"─".repeat(fill.max(0) as usize)));
                self.add("", Color::Default, left, Txt::new());
                if self.o.verbose && !summary.is_empty() {
                    self.prose(summary, 6, C_DIM);
                }
            }
            Item::Notice { text, level } => {
                let (col, mark) = match &**level {
                    "warning" => (C_YELLOW, "●"),
                    "error" => (C_RED, "●"),
                    "ok" => (C_GREEN, "✓"),
                    _ => (C_DIM, "·"),
                };
                let w = (self.cw as isize - 10).max(0) as usize;
                for (k, r) in wrap(&Txt::raw(&one_line(text)), w.min(self.o.prose_cap()))
                    .into_iter()
                    .enumerate()
                {
                    let lead = if k > 0 {
                        Txt::raw("  ")
                    } else {
                        paint(col, mark).and_raw(" ")
                    };
                    let l = self.spine().and_raw("   ").and(lead).and(r.over(col));
                    self.add("", Color::Default, l, Txt::new());
                    if k > 0 {
                        self.wrapped();
                    }
                }
            }
            Item::Interject(x) => {
                self.gap();
                self.while_ = true;
                let r = format!("{}:u:{i}", self.r);
                let names = image_names(&x.images);
                self.user_box(&r, x.text.trim(), &names, false);
                self.while_ = false;
            }
            Item::Step(id) => self.step(*id, 0),
        }
    }

    /// The agent's narration: the work axis, secondary colour.
    fn prose(&mut self, s: &str, indent: usize, c: Style) {
        self.markdown(s.trim(), indent, c, false);
    }

    /// The turn's final words: full text colour, light markdown.
    pub(crate) fn answer(&mut self, s: &str) {
        self.gap();
        self.markdown(s.trim_end_matches([' ', '\t', '\n']), 2, C_TEXT, true);
    }

    // ---- what you said ----

    /// What you said: your edge, who said it and when, the words a column
    /// in, attachments as chips (userbox.go:70).
    fn user_box(&mut self, r: &str, ask: &str, imgs: &[String], multi: bool) {
        if !multi && let Some(parts) = queued_messages(ask) {
            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    // Each its own message, a row of the box between them.
                    self.add(r, Color::Default, paint(C_BLUE, "▍"), Txt::new());
                }
                self.user_box(r, part, &[], false);
                self.named = true; // one "you" for the lot
            }
            if !imgs.is_empty() && !numbered_images_cover(ask, imgs.len()) {
                self.user_box(r, "", imgs, false);
            }
            self.named = false;
            return;
        }
        let inner = 10.max(self.cw.saturating_sub(4));
        let mut chips: Vec<String> = Vec::new();
        let mut rows: Vec<Txt> = Vec::new();
        if multi {
            rows.push(paint(C_WHITE.bold(), "$ ").and(dim("shell command")));
        } else {
            for (text, paste) in paste_parts(ask) {
                if paste {
                    rows.extend(paste_rows(&text, inner));
                    continue;
                }
                let mut rest = String::new();
                let mut last = 0;
                let marked = text.contains('[');
                for m in ATTACH.find_iter(if marked { &text } else { "" }) {
                    rest.push_str(&text[last..m.start()]);
                    chips.push(chip_of(m.as_str()));
                    last = m.end();
                }
                rest.push_str(&text[last..]);
                rows.extend(ask_rows(&rest, inner, |s| styled_ask(s, C_WHITE)));
            }
        }
        for name in imgs {
            chips.push(format!("▣ {name}"));
        }
        rows.extend(chip_rows(&chips, inner));
        let edge = if !r.is_empty() && r == self.o.selected {
            self.spine()
        } else {
            paint(C_BLUE, "▍")
        };
        if !self.named {
            let mut who = paint(
                C_BLUE.bold(),
                if self.while_ {
                    "You, while it worked"
                } else {
                    "You"
                },
            );
            if self.t.start != 0 && !self.while_ {
                who.push(dim(&format!("  {}", clock(self.t.start, self.o.zone))));
            }
            let l = edge.clone().and_raw(" ").and(who);
            self.add(r, Color::Default, l, Txt::new());
        }
        for (i, row) in rows.into_iter().enumerate() {
            let l = edge.clone().and_raw(" ").and(row);
            self.add(r, Color::Default, l, Txt::new());
            if i > 0 {
                self.wrapped();
            }
        }
    }

    /// A turn you didn't start, as a quiet line of its own (userbox.go:224).
    fn other_ask(&mut self, ask: &str, noun: &str, how: Txt, wake: bool) {
        let t = self.t;
        let mut style: fn(&str) -> Txt = sub;
        let mut label = dim(&format!("◌ {}", t.from));
        if t.from.is_empty() && t.prompt.trim().is_empty() {
            label = dim("◌");
        }
        let mut ask = ask.to_string();
        if wake {
            style = text;
            label = dim(noun);
            if !ask.is_empty() {
                ask = format!("\"{ask}\"");
            }
        }
        let said = rush_ask(t);
        if !said.is_empty() {
            style = |s| paint(C_ORANGE, s);
            label = dim("↻ rush");
            ask = said;
        }
        let lw = label.width();
        let w = 10
            .max((self.cw as isize - GUTTER as isize - lw as isize - 3).max(0) as usize)
            .max(10);
        let w = w.min(self.o.prose_cap()).max(10);
        let mut rows = ask_rows(&ask, w, style);
        if t.prompt.trim().is_empty() && t.from.is_empty() {
            rows = vec![dim(unasked(t))];
        }
        if wake && let Some(last) = rows.last_mut() {
            last.push(how);
        }
        let r = self.r.clone();
        for (i, row) in rows.into_iter().enumerate() {
            if i == 0 {
                let l = self
                    .sp(GUTTER - 1)
                    .and(label.clone())
                    .and_raw("  ")
                    .and(row);
                self.add(&r, Color::Default, l, Txt::new());
            } else {
                let l = self.sp(GUTTER - 1 + lw + 2).and(row);
                self.add(&r, Color::Default, l, Txt::new());
                self.wrapped();
            }
        }
    }

    // ---- live ----

    /// What the agent is doing right now, at the end of a running turn
    /// (render.go:1131).
    fn live_line(&mut self) {
        let t = self.t;
        if let Some(c) = self.s.compacting() {
            self.compacting_line(c);
            return;
        }
        let (mut verb, mut since, mut waiting) =
            (pick(OPENINGS, t.start).to_string(), t.start, false);
        if let Some(last) = t.items.last() {
            let n = t.items.len() - 1;
            match last {
                Item::Thinking(_) if t.thinking != 0 => {
                    verb = pick(MUSINGS, t.thinking).into();
                    since = t.thinking;
                }
                Item::Text { .. }
                    if self.s.streaming().is_some_and(|(ti, ii)| {
                        ti as usize + 1 == t.n as usize && ii as usize == n
                    }) =>
                {
                    verb = pick(WRITINGS, t.start).into();
                }
                Item::Step(id) => {
                    let st = self.s.step(*id);
                    if st.status == Status::Running {
                        return; // the step's own row is spinning
                    }
                    if st.end != 0 {
                        verb = pick(WAITINGS, st.end).into();
                        since = st.end;
                        waiting = true;
                    }
                }
                _ => {}
            }
        }
        let pad = self.spine().and_raw("   ");
        self.air();
        let mut head = paint(C_ORANGE.bold(), &format!("{} {verb}…", spin(self.o.tick)));
        if since != 0 {
            let s = self.since(since);
            head.push_str("  ", Style::default());
            head.push(paint(C_ORANGE, &s));
        }
        let mut facts = Vec::new();
        if t.start != 0 && since != t.start {
            facts.push(format!("turn {}", self.since(t.start)));
        }
        if t.streamed > 0 {
            facts.push(format!("↓ {} tokens", tokens((t.streamed / 4) as i64)));
        }
        if waiting {
            if self.s.context > 0 {
                facts.push(format!("~{} tokens in", tokens(self.s.context)));
            }
            if self.cache_cold(since) {
                facts.push("cache expired, read uncached".into());
            }
        }
        if !facts.is_empty() {
            head.push_str("  ", Style::default());
            head.push(dim(&facts.join(" · ")));
        }
        self.add("", Color::Default, pad.clone().and(head), Txt::new());
        let heard = t.heard.max(t.start);
        let quiet = self.o.now - heard;
        if t.start != 0 && quiet >= STALL_AFTER {
            let s = dur(quiet);
            let l = pad.and_raw("  ").and(paint(
                C_YELLOW,
                &format!(
                    "⚠ stalled · nothing from the model for {s} · stop the turn and send again"
                ),
            ));
            self.add("", Color::Default, l, Txt::new());
        }
    }

    /// Whether the prompt cache had gone by at: an hour past the main
    /// agent's last request (overview.go:91).
    fn cache_cold(&self, _at: Ms) -> bool {
        let at = self
            .s
            .requests
            .iter()
            .rev()
            .find(|r| r.run.is_empty())
            .map_or(0, |r| r.at + 3_600_000);
        at != 0 && self.o.now > at
    }

    fn compacting_line(&mut self, since: Ms) {
        let pad = self.spine().and_raw("   ");
        self.air();
        let s = self.since(since);
        let l = pad
            .clone()
            .and(paint(
                C_ORANGE.bold(),
                &format!("{} {}…", spin(self.o.tick), pick(COMPACTIONS, since)),
            ))
            .and_raw("  ")
            .and(paint(C_ORANGE, &s));
        self.add("", Color::Default, l, Txt::new());
        let mut facts = Vec::new();
        let mut frac = -1.0f64;
        if self.s.context > 0 {
            let ctx = self.s.context;
            facts.push(format!("{} tokens to boil down", tokens(ctx)));
            let rate = self.s.compact_rate_ns();
            let (est, basis, over) = if rate > 0 {
                (
                    (rate * ctx / 1_000_000).max(5000),
                    ", as the last one went",
                    "longer than the last one",
                )
            } else {
                (20_000 + (ctx / 4000) * 1000, "", "taking longer than usual")
            };
            let gone = self.o.now - since;
            frac = (gone as f64 / est as f64).min(0.95);
            let left = est - gone;
            if left > 0 {
                facts.push(format!("~{} left{basis}", dur((left + 500) / 1000 * 1000)));
            } else {
                facts.push(over.into());
            }
        }
        self.fast = true;
        let tick = (self.o.now / 100) as usize / 2;
        let l = pad
            .and_raw("  ")
            .and(dots_bar(frac, 16, tick))
            .and_raw("  ")
            .and(dim(&facts.join(" · ")));
        self.add("", Color::Default, l, Txt::new());
    }

    fn err_card(&mut self, err: &str) {
        let room = self.cw.min(CAP_ROW) as isize - GUTTER as isize - 5;
        if room < 24 {
            let l = self.sp(GUTTER - 1).and(paint(C_RED, &format!("✗ {err}")));
            self.add("", Color::Default, l, dim("your next message picks it up"));
            return;
        }
        let room = (room as usize).min(72);
        let (title, todo) = err_advice(err);
        let rows = wrap(&sub(err), room);
        self.gap();
        let head = paint(C_RED, "✗ ").and(paint(C_TEXT.bold(), title));
        self.card(
            "",
            GUTTER,
            head,
            Txt::new(),
            rows,
            dim(todo),
            room,
            C_LOST_Q,
        );
    }

    /// A card as lines under the turn's rule: its head, its rows, its foot,
    /// a rail in colour frame down them (cards.go:1609).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn card(
        &mut self,
        r: &str,
        indent: usize,
        head: Txt,
        head_r: Txt,
        rows: Vec<Txt>,
        foot: Txt,
        inner: usize,
        frame: Style,
    ) {
        let from = self.lines.len();
        let mut body: Vec<Txt> = rows
            .into_iter()
            .map(|r| {
                if r.width() > inner {
                    r.take(inner - 1).and(faint("…"))
                } else {
                    r
                }
            })
            .collect();
        if !foot.is_empty() {
            body.push(foot.over(C_DIM));
        }
        let n = body.len();
        let rail = |i: isize| -> Txt {
            if n == 0 {
                Txt::raw(" ")
            } else if i < 0 {
                paint(frame, "╭")
            } else if i as usize == n - 1 {
                paint(frame, "╰")
            } else {
                paint(frame, "│")
            }
        };
        let l = self.sp(indent - 1).and(rail(-1)).and_raw(" ").and(head);
        self.add(r, Color::Default, l, head_r);
        for (i, b) in body.into_iter().enumerate() {
            let l = self
                .sp(indent - 1)
                .and(rail(i as isize))
                .and_raw(" ")
                .and(b);
            self.add("", Color::Default, l, Txt::new());
        }
        if !r.is_empty() {
            self.own(r, from);
        }
    }

    // ---- steps ----

    /// A run of clean steps as one row (render.go:1403).
    fn run(&mut self, r: &str, ids: &[StepId]) {
        self.tool += 1;
        let (mut first, mut last) = (0, 0);
        for &id in ids {
            let st = self.s.step(id);
            if first == 0 || st.start < first {
                first = st.start;
            }
            if st.end > last {
                last = st.end;
            }
        }
        let names: Vec<String> = self.run_groups(ids).into_iter().map(|g| g.0).collect();
        let left = self
            .sp(GUTTER - 1)
            .and(dim("✓"))
            .and_raw(" ")
            .and(dim(&names.join(", ")))
            .and(faint(&format!(" · {}", plural(ids.len(), "step"))));
        let mut right = Txt::new();
        if first != 0 && last != 0 && last - first >= 100 {
            right = dim(&dur(last - first)).and_raw("  ");
        }
        self.worked = true;
        self.add(r, Color::Default, left, right);
        self.tool -= 1;
    }

    /// A run's kinds of step in the order they came: the first four and
    /// how many more, each with whether one failed.
    fn run_groups(&mut self, ids: &[StepId]) -> Vec<(String, bool)> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut failed: HashMap<String, bool> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for &id in ids {
            let g = self.verb(id);
            let st = self.s.step(id);
            let c = counts.entry(g.clone()).or_insert(0);
            if *c == 0 {
                order.push(g.clone());
            }
            *c += 1;
            let bad = st.status == Status::Failed || self.tests_failed(id);
            *failed.entry(g).or_insert(false) |= bad;
        }
        let mut out = Vec::new();
        for (i, g) in order.iter().enumerate() {
            if i == 4 {
                let more = order[4..].iter().any(|h| failed[h]);
                out.push((format!("+{} more", order.len() - 4), more));
                break;
            }
            let mut name = g.clone();
            if let Some(c) = name.chars().next()
                && c.is_uppercase()
            {
                name = c.to_lowercase().collect::<String>() + &name[c.len_utf8()..];
            }
            out.push((group_words(&name, counts[g]), failed[g]));
        }
        out
    }

    /// A step in a word or two, for a folded run (render.go:1340).
    fn verb(&mut self, id: StepId) -> String {
        let st = self.s.step(id);
        match st.kind {
            Kind::Shell => {
                // A chain that committed or pushed is named for that.
                if let Some(c) = self.step_cards(id).first() {
                    return c.verb();
                }
                let cmd = self.info(id).call.input.command.clone();
                let sh = shell_shape(self.p, &cmd);
                match sh.kind {
                    "read" | "search" | "write" => sh.kind.to_string(),
                    "commit" => "git commit".into(),
                    "" => {
                        let ps = phrases(self.p, &cmd);
                        ps.first().map_or("shell".into(), |p| p.verb.clone())
                    }
                    _ => drop_cd(cmd.trim())
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                }
            }
            Kind::Read => "read".into(),
            Kind::Search | Kind::Glob => "search".into(),
            Kind::Fetch => "fetch".into(),
            Kind::WebSearch => "web search".into(),
            Kind::Subagent => agent_name(st),
            _ if &*st.tool == "Skill" || &*st.tool == "SlashCommand" => {
                let v = input_value(st);
                first_non_empty(&[str_of(&v, "skill"), str_of(&v, "command"), "skill"]).to_string()
            }
            _ => {
                if let Some(rest) = st.tool.strip_prefix("mcp__")
                    && let Some((_, name)) = rest.split_once("__")
                {
                    return name.to_string();
                }
                st.tool.to_string()
            }
        }
    }

    fn step_open(&self, id: StepId, r: &str) -> bool {
        if let Some(v) = self.o.open.get(r) {
            return *v;
        }
        self.o.verbose || Some(id) == self.latest && self.step_cards(id).is_empty()
    }

    fn status_mark(&self, st: &Step) -> Txt {
        match st.status {
            Status::Running => paint(C_ORANGE, spin(self.o.tick + st.id.len())),
            Status::Ok if st.kind == Kind::Shell && self.tests_failed_step(st) => paint(C_RED, "✗"),
            Status::Ok => dim("✓"),
            Status::Failed => paint(C_RED, "✗"),
            Status::Waiting => paint(C_YELLOW, "●"),
            Status::Denied => {
                if classified(st).is_some() {
                    paint(C_YELLOW, "⊘")
                } else {
                    dim("⊘")
                }
            }
            Status::Lost => dim("◌"),
        }
    }

    /// One step: its row, and under it what it did when it's open
    /// (render.go:2215).
    fn step(&mut self, id: StepId, depth: usize) {
        let st = self.s.step(id);
        if hidden(st) {
            return;
        }
        let r = self.step_ref(st);
        let from = self.lines.len();
        self.tool += 1;
        let helper = st.kind == Kind::Subagent;
        if helper {
            self.helper += 1;
        }
        let indent = GUTTER + depth * 2;
        self.step_rows(id, &r, indent, depth);
        if helper {
            self.helper -= 1;
        }
        self.tool -= 1;
        self.own(&r, from);
    }

    fn step_rows(&mut self, id: StepId, r: &str, indent: usize, depth: usize) {
        let st = self.s.step(id);
        if &*st.tool == SHOW && st.status != Status::Failed && self.figure(id, r, indent) {
            return;
        }
        if message_tool(&st.tool) {
            self.message(id, r, indent);
            return;
        }
        let lead = self.sp(indent - 1).and(self.status_mark(st)).and_raw(" ");
        let open = self.step_open(id, r);
        let mut label = self.label(id);
        let (mut cells, right) = self.cells(id);
        if !cells.is_empty() {
            cells = faint("  · ").and(cells);
            let room = self.cw as isize
                - lead.width() as isize
                - cells.width() as isize
                - right.width() as isize
                - 3;
            if room >= 12 && label.width() > room as usize {
                label = label.truncate(room as usize, &Txt::raw("…"));
            }
        }
        let left = lead.and(label).and(cells);
        self.worked = true;
        let brief = st.status == Status::Failed && !open;
        if brief {
            self.add(r, Color::Default, left, right);
            // A card says what went wrong better than a line of the output.
            if self.step_cards(id).is_empty() {
                self.error_line(id, indent + 4, r);
            }
        } else {
            let mut hint = if open {
                self.view_hint(id, r)
            } else {
                Txt::new()
            };
            if !hint.is_empty() && !right.is_empty() {
                hint.push(faint(" · "));
            }
            self.add(r, Color::Default, left, hint.and(right));
        }
        if open {
            let opened = self.o.open.contains_key(r);
            self.peek = !opened && !self.o.verbose;
            self.body(id, indent + 4);
            self.peek = false;
        } else if !brief {
            self.pictures(id, indent + 4);
        }
        self.cards(id, indent + 2);
        self.denial(id, indent + 2);
        let st = self.s.step(id);
        if !st.children.is_empty() && (st.status == Status::Running || open) {
            let mut kids: &[StepId] = &st.children;
            let r = r.to_string();
            self.railed(indent, false, |d| {
                if !d.o.verbose
                    && !d.o.open.get(&r).copied().unwrap_or(false)
                    && kids.len() > SPAWN_SHOWN
                {
                    let l = d.sp(indent + 1).and(faint(&format!(
                        "⋯ {} before",
                        plural(kids.len() - SPAWN_SHOWN, "step")
                    )));
                    d.add("", Color::Default, l, Txt::new());
                    kids = &kids[kids.len() - SPAWN_SHOWN..];
                }
                for &c in kids {
                    d.step(c, depth + 1);
                }
            });
        }
    }

    /// What a step's row says after its label, and on the right how long it
    /// took (render.go:2337).
    fn cells(&mut self, id: StepId) -> (Txt, Txt) {
        let st = self.s.step(id);
        if st.status == Status::Waiting {
            let mut wait = Txt::new();
            if st.start != 0 {
                let s = self.since(st.start);
                wait = dim(&s).and_raw("  ");
            }
            return (paint(C_YELLOW, "waiting on you"), wait);
        }
        if blocked(st) {
            return (paint(C_RED, "blocked by the harness"), Txt::new());
        }
        let mut parts: Vec<Txt> = Vec::new();
        // A card under the row says it better.
        let s = self.summary(id);
        if !s.is_empty() && self.step_cards(id).is_empty() {
            parts.push(s);
        }
        if st.kind == Kind::Shell && st.exit > 0 {
            parts.push(paint(
                C_RED,
                &match st.exit {
                    124 => "timed out".to_string(),
                    137 | 143 => "killed".to_string(),
                    n => format!("exit {n}"),
                },
            ));
        }
        let mut right = Txt::new();
        if st.status == Status::Running && st.start != 0 {
            let s = self.since(st.start);
            right = dim(&s);
            if self.o.now - st.start >= 30_000 {
                right.push(faint(&format!(" since {}", clock(st.start, self.o.zone))));
            }
        } else if st.end != 0 && st.start != 0 && st.end - st.start >= 100 {
            right = dim(&dur(st.end - st.start));
        }
        if !right.is_empty() {
            right.push_str("  ", Style::default());
        }
        let mut out = Txt::new();
        for (i, p) in parts.into_iter().enumerate() {
            if i > 0 {
                out.push(faint(" · "));
            }
            out.push(p);
        }
        (out, right)
    }

    /// A step's row: its glyph and what it worked on (render.go:2566).
    fn label(&mut self, id: StepId) -> Txt {
        let st = self.s.step(id);
        let info = self.info(id);
        let x = &info.call.input;
        let g = glyph_color(glyph_for(st));
        let base = if st.status != Status::Ok {
            C_SUB
        } else {
            C_DIM
        };
        let lbl = |s: &str| paint(base, s);
        match st.kind {
            Kind::Shell => {
                let cmd = &x.command;
                let desc = &one_line(&x.description);
                if !desc.is_empty() {
                    return lbl(desc).and_raw("  ").and(faint(&program(cmd)));
                }
                let sh = shell_shape(self.p, cmd);
                if !sh.kind.is_empty() {
                    return glyph_color(sh.glyph).and_raw(" ").and(lbl(&sh.what));
                }
                let l = self.chain(cmd, base);
                if !l.is_empty() {
                    return l;
                }
                return self.command(cmd, base);
            }
            Kind::Edit | Kind::Write | Kind::Notebook | Kind::Delete | Kind::Move | Kind::Read => {
                return g.and_raw(" ").and(lbl(&self.p.rel(&x.path)));
            }
            Kind::Search => {
                let mut l = g.and_raw(" ").and(lbl(&x.pattern));
                if !x.path.is_empty() {
                    l.push(faint(" in "));
                    l.push(lbl(&self.p.rel(&x.path)));
                }
                return l;
            }
            Kind::Glob => return g.and_raw(" ").and(lbl(&x.pattern)),
            Kind::Subagent => {
                return g
                    .and_raw(" ")
                    .and(lbl(&agent_name(st)))
                    .and_raw("  ")
                    .and(faint(&one_line(&x.description)));
            }
            Kind::Fetch => return g.and_raw(" ").and(lbl(&x.url)),
            Kind::WebSearch => return g.and_raw(" ").and(lbl(&x.query)),
            Kind::Question => {
                return paint(C_YELLOW, "?")
                    .and_raw(" ")
                    .and(text(&one_line(&info.call.title)));
            }
            _ => {}
        }
        let v = input_value(st);
        match &*st.tool {
            "Skill" | "SlashCommand" => {
                let name = first_non_empty(&[
                    str_of(&v, "skill"),
                    str_of(&v, "command"),
                    str_of(&v, "name"),
                ]);
                return g
                    .and_raw(" ")
                    .and(lbl(name))
                    .and_raw("  ")
                    .and(faint(&one_line(str_of(&v, "args"))));
            }
            "Artifact" => {
                let mut t = str_of(&v, "title").to_string();
                if t.is_empty() {
                    t = base_name(str_of(&v, "file_path")).to_string();
                }
                return g.and_raw(" ").and(lbl(&t));
            }
            _ => {}
        }
        if let Some(l) = tool_label(st, &v, &lbl) {
            return l;
        }
        let mut name = lbl(&st.tool);
        if let Some(rest) = st.tool.strip_prefix("mcp__")
            && let Some((server, tool)) = rest.split_once("__")
        {
            name = lbl(tool).and(faint(&format!(" · {server}")));
        }
        g.and_raw(" ")
            .and(name)
            .and_raw("  ")
            .and(faint(&first_value(&v)))
    }

    /// A chain, or a command too long to read at a glance, as what each of
    /// its commands does; empty for a short single command (bash.go:62).
    fn chain(&self, cmd: &str, c: Style) -> Txt {
        let cmd = cmd.trim();
        if cmd.contains([';', '&', '|', '\n']) || width(cmd) > 60 {
            let ps = phrases(self.p, cmd);
            if !ps.is_empty() && (segments(cmd).len() > 1 || width(cmd) > 60) {
                let l = chain_label(&ps, c);
                let g = chain_glyph(&ps);
                if g != "$" {
                    return glyph_color(g).and_raw(" ").and(l);
                }
                return l;
            }
        }
        Txt::new()
    }

    /// A command's first line in colour c, without a cd it opens with
    /// (render.go:2666).
    fn command(&self, cmd: &str, c: Style) -> Txt {
        let mut cmd = cmd.trim().to_string();
        for b in &self.p.bases {
            cmd = cmd.replace(&format!("{b}/"), "");
        }
        let lines: Vec<&str> = cmd.split('\n').collect();
        let first = drop_cd(lines[0]);
        let mut out = paint(c, first);
        if lines.len() > 1 {
            out.push(faint(&format!("  +{} lines", lines.len() - 1)));
        }
        out
    }

    /// How a step came out, for its row (render.go:2902).
    fn summary(&mut self, id: StepId) -> Txt {
        let st = self.s.step(id);
        let info = self.info(id);
        match st.kind {
            Kind::Shell => {
                let out = bash_out(st, &info.out);
                static GO_OK: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^ok\s+\S+"));
                static GO_FAIL: LazyLock<Regex> =
                    LazyLock::new(|| re(r"(?m)^(FAIL\s+\S+|--- FAIL)"));
                static GO_FAIL_NAME: LazyLock<Regex> =
                    LazyLock::new(|| re(r"(?m)^\s*--- FAIL: (\S+)"));
                static JS_SUM: LazyLock<Regex> = LazyLock::new(|| {
                    re(r"Tests?\s+(?:\x1b\[[0-9;]*m)?(?:(\d+) failed[^|\n]*\|\s*)?(\d+) passed")
                });
                static COMMIT: LazyLock<Regex> =
                    LazyLock::new(|| re(r"(?m)^\[[^\]]*?([0-9a-f]{7,40})\]"));
                let n = GO_FAIL.find_iter(&out).count();
                if n > 0 {
                    let mut s = format!("{n} failed");
                    let names: Vec<&str> = GO_FAIL_NAME
                        .captures_iter(&out)
                        .take(3)
                        .map(|m| m.get(1).unwrap().as_str())
                        .collect();
                    if !names.is_empty() {
                        s = format!("{s}: {}", names.join(", "));
                    }
                    return paint(C_RED, &s);
                }
                if let Some(m) = JS_SUM.captures(&out) {
                    let passed = &m[2];
                    if let Some(f) = m.get(1)
                        && f.as_str() != "0"
                    {
                        return paint(C_RED, &format!("{} failed", f.as_str()))
                            .and(faint(&format!(" · {passed} passed")));
                    }
                    return faint(&format!("{passed} passed"));
                }
                let n = GO_OK.find_iter(&out).count();
                if n > 0 {
                    return faint(&if n == 1 {
                        "ok".into()
                    } else {
                        format!("{n} ok")
                    });
                }
                if st.status != Status::Ok {
                    return Txt::new();
                }
                let sh = shell_shape(self.p, &info.call.input.command);
                let n = count_lines(&out);
                if sh.kind == "commit" {
                    let mut res = sh.res.clone();
                    if let Some(m) = COMMIT.captures(&out) {
                        res = format!("{res} {}", &m[1]).trim().to_string();
                    }
                    return faint(&res);
                }
                if sh.kind == "search" && sh.ctx && n > 0 {
                    return faint(&plural(n, "line"));
                }
                if sh.kind == "search" && n > 0 {
                    return faint(&if n == 1 {
                        "1 match".into()
                    } else {
                        format!("{n} matches")
                    });
                }
                if sh.kind == "search" {
                    return faint("no matches");
                }
                if !sh.res.is_empty() {
                    return faint(&sh.res);
                }
                if n == 0 {
                    return faint("no output");
                }
                if n > 1 {
                    return faint(&format!("{n} lines"));
                }
            }
            Kind::Edit | Kind::Write => {
                let o = &info.out;
                if o.created {
                    return paint(
                        C_GREEN,
                        &format!("new · {} lines", count_lines(&info.call.input.content)),
                    );
                }
                let (mut add, mut del) = (0, 0);
                for p in &o.patches {
                    for l in &p.lines {
                        if l.starts_with('+') {
                            add += 1;
                        } else if l.starts_with('-') {
                            del += 1;
                        }
                    }
                }
                if add + del > 0 {
                    return paint(C_GREEN, &format!("+{add}"))
                        .and_raw(" ")
                        .and(paint(C_RED, &format!("−{del}")));
                }
            }
            Kind::Read => {
                if let Some(f) = info.out.lines
                    && f.count > 0
                {
                    if f.count < f.total {
                        return faint(&format!("lines {}–{}", f.start, f.start + f.count - 1));
                    }
                    return faint(&format!("{} lines", f.total));
                }
            }
            Kind::Search | Kind::Glob => {
                if st.status == Status::Ok {
                    let mut n = count_lines(&st.output);
                    if st.output.trim().starts_with("No ") {
                        n = 0;
                    }
                    let noun = if st.kind == Kind::Glob {
                        "files"
                    } else {
                        "results"
                    };
                    return faint(&format!("{n} {noun}"));
                }
            }
            Kind::Subagent if !st.children.is_empty() => {
                return faint(&plural(st.children.len(), "step"));
            }
            _ => {}
        }
        tool_summary(st)
    }

    fn view_hint(&mut self, id: StepId, r: &str) -> Txt {
        let (v, more) = self.view_of(id, r);
        if !more || self.o.selected != r && (v == "text" || v == "image") {
            return Txt::new();
        }
        faint(&format!("{v} · v cycles"))
    }

    /// The view a step's output is drawn in, and whether it has another
    /// (views.go:63, the hex view off).
    fn view_of(&mut self, id: StepId, r: &str) -> (&'static str, bool) {
        let (vs, auto) = self.views(id);
        if let Some(v) = self.o.view.get(r)
            && let Some(x) = vs.iter().find(|x| **x == v.as_str())
        {
            return (x, vs.len() > 1);
        }
        (auto, vs.len() > 1)
    }

    fn views(&mut self, id: StepId) -> (Vec<&'static str>, &'static str) {
        let st = self.s.step(id);
        if matches!(st.kind, Kind::Edit | Kind::Write | Kind::Subagent)
            || message_tool(&st.tool)
            || stop_tool(&st.tool)
            || st.tool.ends_with("__show")
        {
            return (vec![], "");
        }
        let info = self.info(id);
        let path = if st.kind == Kind::Read {
            self.p.abs(&info.call.input.path)
        } else {
            String::new()
        };
        if !st.images.is_empty() || thumbable(&path) {
            return (vec!["image"], "image");
        }
        let t = match st.kind {
            Kind::Shell if !info.out.stdout.is_empty() => info.out.stdout.clone(),
            Kind::Read => unnumbered(&st.output),
            _ => st.output.to_string(),
        };
        if t.trim().is_empty() {
            return (vec![], "");
        }
        if super::code::structure(t.trim()) == 0 {
            return (vec!["text"], "text");
        }
        let vs = vec!["text", "pretty"];
        if st.status != Status::Failed && super::code::better_pretty(&t) {
            return (vs, "pretty");
        }
        (vs, "text")
    }

    /// What a step ran and did, under its row (render.go:3030).
    fn body(&mut self, id: StepId, indent: usize) {
        self.lg = None;
        self.by_path = false;
        self.reset_hl();
        self.body_rows(id, indent);
        self.lg = None;
        self.by_path = false;
        self.spans.clear();
        self.view = "";
        self.nowrap = false;
    }

    fn body_rows(&mut self, id: StepId, indent: usize) {
        let st = self.s.step(id);
        let info = self.info(id);
        let x = &info.call.input;
        let r = self.step_ref(st);
        self.nowrap = self
            .o
            .open
            .get(&format!("{r}:nowrap"))
            .copied()
            .unwrap_or(false);
        if stop_tool(&st.tool) && st.status != Status::Failed {
            let cmd = stopped_command(st);
            if !cmd.is_empty() {
                self.shell_body(Some(id), &cmd, indent);
            }
            return;
        }
        if st.kind == Kind::Question
            && let Some(qa) = answered(st)
        {
            self.answers(&qa, indent);
            return;
        }
        let (v, _) = self.view_of(id, &r);
        if v == "pretty" || v == "text" && self.o.view.get(&r).is_some_and(|x| x == "text") {
            self.view = v;
        }
        match st.kind {
            Kind::Read => self.lg = lang_for(&x.path),
            Kind::Search => self.by_path = true,
            Kind::Shell => {
                let sh = shell_shape(self.p, &x.command);
                match sh.kind {
                    "read" => self.lg = lang_for(&sh.what),
                    "search" => {
                        self.by_path = true;
                        if let Some((_, p)) = sh.what.split_once(" in ") {
                            self.lg = lang_for(p.split(", ").next().unwrap_or(""));
                        }
                    }
                    "" => self.spans = chain_spans(self.p, &x.command),
                    _ => {}
                }
            }
            _ => {}
        }
        if self.pictures(id, indent) && st.output.trim().is_empty() {
            return;
        }
        match st.kind {
            Kind::Edit | Kind::Write => {
                if self.diff(id, indent) {
                    return;
                }
            }
            Kind::Shell => {
                let mut drawn = 0;
                let cmd = x.command.trim();
                if !cmd.is_empty() && (!self.peek || shell_shape(self.p, cmd).kind == "write") {
                    drawn = self.shell_body(Some(id), cmd, indent);
                    self.marks = echo_marks(cmd);
                }
                let o = &info.out;
                if !o.stdout.is_empty() || !o.stderr.is_empty() {
                    if drawn > 0 && !format!("{}{}", o.stdout, o.stderr).trim().is_empty() {
                        self.cmd_rule(indent, drawn);
                    }
                    self.output(
                        &o.stdout,
                        indent,
                        st.status == Status::Failed && o.stderr.is_empty(),
                    );
                    self.spans.clear();
                    self.view = "";
                    if !o.stderr.trim().is_empty() {
                        if !o.stdout.trim().is_empty() {
                            let l = self.sp(indent - 1).and(paint(C_RED, "stderr"));
                            self.add("", Color::Default, l, Txt::new());
                        }
                        self.output(o.stderr.trim_start_matches('\n'), indent, true);
                    }
                    self.marks.clear();
                    return;
                }
                self.marks.clear();
                if st.status == Status::Ok && st.output.trim().is_empty() && !self.peek {
                    let l = self.sp(indent - 1).and(faint("no output"));
                    self.add("", Color::Default, l, Txt::new());
                    return;
                }
            }
            _ => {}
        }
        if classified(st).is_some() && !self.o.verbose {
            return;
        }
        let mut out = st.output.to_string();
        if self.view == "pretty" && st.kind == Kind::Read {
            out = unnumbered(&out);
        }
        static EXIT: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^(?:Error: )?Exit code (\d+)"));
        let out = out
            .replace("<tool_use_error>", "")
            .replace("</tool_use_error>", "");
        let out = EXIT.replace_all(&out, "");
        self.output(
            out.trim_start_matches('\n'),
            indent,
            st.status == Status::Failed,
        );
    }

    fn answers(&mut self, qa: &[(String, String)], indent: usize) {
        let w = self.cw.saturating_sub(indent + 2);
        for (i, (q, a)) in qa.iter().enumerate() {
            if i > 0 {
                self.blank();
            }
            let pad = self.sp(indent - 1);
            self.add_rows(Color::Default, pad.clone(), faint("? "), faint(q), w, 0);
            self.add_rows(Color::Default, pad, paint(C_GREEN, "→ "), text(a), w, 0);
        }
    }

    /// The images a step read or was given back, as chips: rush-convo draws
    /// no thumbnails (picture.go).
    fn pictures(&mut self, id: StepId, indent: usize) -> bool {
        let st = self.s.step(id);
        let path = if st.kind == Kind::Read {
            self.p.abs(&self.info(id).call.input.path)
        } else {
            String::new()
        };
        let n = if st.images.is_empty() && thumbable(&path) {
            1
        } else {
            st.images.len()
        };
        for _ in 0..n {
            let chip = if path.is_empty() {
                paint(C_BLUE, "▣ ").and(paint(C_TEXT, "image"))
            } else {
                image_chip("", &path)
            };
            let l = self.sp(indent - 1).and(chip);
            self.add("", Color::Default, l, Txt::new());
        }
        n > 0
    }

    /// An opened shell step's command: on one line when it fits, else laid
    /// out a command a line; returns the widest row (render.go:3165).
    fn shell_body(&mut self, step: Option<StepId>, cmd: &str, indent: usize) -> usize {
        let pad = self.sp(indent - 1);
        let room = self.cw as isize - indent as isize - 4;
        let room = room.max(0) as usize;
        let mut raw = shell_lines(cmd);
        let script = compound(&raw);
        let home = self.p.home.clone();
        let start = CwdAt {
            dir: self.p.bases.first().cloned().unwrap_or_default(),
            sure: false,
        };
        if !cmd.contains('\n') {
            let mut cur = start.clone();
            let one = short_paths(&expand_tabs(cmd), &home, &mut cur, !script);
            if width(&one) <= room {
                let l = pad.and(faint("$ ")).and(quiet_tint(&one));
                self.add("", BG_WELL, l, Txt::new());
                return 2 + width(&one);
            }
        }
        let mut cur = start;
        for l in &mut raw {
            if !l.verbatim {
                l.text = short_paths(&l.text, &home, &mut cur, !script && l.depth == 0);
            }
        }
        let mut lines: Vec<super::shell::ShLine> = Vec::new();
        let mut gut = 2;
        if script {
            lines = script_lines(&raw);
            raw.clear();
        }
        for l in raw {
            if let Some(last) = lines.last_mut()
                && l.text.starts_with("| ")
                && !last.verbatim
                && width(&last.text) + 1 + width(&l.text) + last.depth * 2 <= room
            {
                last.text.push(' ');
                last.text.push_str(&l.text);
                last.to = l.to;
                continue;
            }
            if !l.verbatim && (l.text.starts_with("&& ") || l.text.starts_with("|| ")) {
                gut = 3;
            }
            lines.extend(subshell_lines(l));
        }
        // A heredoc's lines are numbered as what reads it counts them; the
        // word that ends it isn't one. A script fed that way which failed
        // shows the line it stopped at, and what went wrong there.
        let mut no = vec![0usize; lines.len()];
        let (mut body, mut most, mut docs) = (0, 0, 0);
        for i in 0..lines.len() {
            if !lines[i].verbatim {
                continue;
            }
            body += 1;
            if i == 0 || !lines[i - 1].verbatim {
                docs += 1;
            }
            if i + 1 < lines.len() && lines[i + 1].verbatim {
                no[i] = 1;
                if i > 0 && lines[i - 1].verbatim {
                    no[i] = no[i - 1] + 1;
                }
                most = most.max(no[i]);
            }
        }
        let (mut fault, mut says) = (0, String::new());
        if docs == 1
            && let Some(id) = step
        {
            let st = self.s.step(id);
            if st.status == Status::Failed || self.tests_failed(id) {
                (fault, says) = stdin_fault(&bash_out(st, &self.info(id).out));
            }
        }
        let num_w = most.to_string().len();
        let verbose = self.o.verbose;
        let shown = |i: usize| -> bool {
            let n = no[i];
            // n is 0 for the end word.
            verbose || body <= 8 || n <= 6 || fault > 0 && n + 1 >= fault && n <= fault + 1
        };
        let blank = " ".repeat(gut);
        let mut widest = 0;
        let mut lg: Option<&'static Lang> = None;
        let mut hs = HlState::default();
        for i in 0..lines.len() {
            let l = &lines[i];
            let mut lead = Txt::raw(&blank);
            let mut textv = l.text.clone();
            if i == 0 || !script && !l.verbatim && l.depth == 0 {
                lead = faint("$").and_raw(&blank[1..]);
            }
            let op = l.text.get(..3).unwrap_or("");
            if !script && !l.verbatim && (op == "&& " || op == "|| ") {
                lead = paint(C_ORANGE, &op[..2]).and_raw(" ");
                textv = l.text[3..].to_string();
            }
            if !l.verbatim && l.text.contains("<<") {
                lg = super::shell::heredoc_lang(&l.text);
                hs = HlState::default();
            }
            if l.verbatim && !shown(i) {
                if i > 0 && shown(i - 1) {
                    let mut hid = 0;
                    let mut j = i;
                    while j < lines.len() && lines[j].verbatim && !shown(j) {
                        hid += 1;
                        j += 1;
                    }
                    let left = pad
                        .clone()
                        .and(lead)
                        .and(blanks(num_w + 2))
                        .and(folded(hid));
                    self.add(SHOW_ALL_REF, BG_WELL, left, Txt::new());
                }
                continue;
            }
            let mut num = Txt::new();
            if l.verbatim {
                num = blanks(num_w + 2);
                if no[i] > 0 {
                    num = faint(&format!("{:>w$}  ", no[i], w = num_w));
                }
                if no[i] > 0 && no[i] == fault {
                    num = paint(C_RED.bold(), &format!("{:>w$}", no[i], w = num_w))
                        .and(paint(C_RED, "▸ "));
                }
            }
            let hang = " ".repeat(l.depth * 2);
            let mut colored = quiet_tint(&expand_tabs(&textv));
            if script && !l.verbatim {
                let mut st = HlState::default();
                colored = highlight(lang_for("sh"), &mut st, &expand_tabs(&textv), C_SUB, None);
            }
            if l.verbatim {
                colored = highlight(lg, &mut hs, &expand_tabs(&textv), C_OUT, None);
                if no[i] == 0 {
                    colored = faint(&textv);
                }
            }
            let fit = (room as isize - hang.len() as isize - num.width() as isize).max(0) as usize;
            let rows = if !script && !l.verbatim {
                let toks = quiet_tokens(&expand_tabs(&textv));
                token_rows(&toks, fit, fit.saturating_sub(2))
            } else {
                wrap(&colored, fit)
            };
            for (j, r) in rows.into_iter().enumerate() {
                let (mut lead, mut num, mut r) = (lead.clone(), num.clone(), r);
                if j > 0 {
                    lead = Txt::raw(&blank);
                    r = Txt::raw("  ").and(r);
                    if !num.is_empty() {
                        num = blanks(num_w + 2);
                    }
                }
                let body = lead.and(num).and_raw(&hang).and(r);
                widest = widest.max(body.width());
                let left = pad.clone().and(body);
                self.add("", BG_WELL, left, Txt::new());
                if j > 0 {
                    self.wrapped();
                }
            }
            if l.verbatim && no[i] > 0 && no[i] == fault && !says.is_empty() {
                let w = room.saturating_sub(num_w + 2);
                for r in wrap(&paint(C_RED, &format!("✗ {says}")), w) {
                    let left = pad.clone().and_raw(&blank).and(blanks(num_w + 2)).and(r);
                    self.add("", BG_WELL, left, Txt::new());
                }
            }
        }
        widest
    }

    /// A faint rule between a command and what it printed.
    fn cmd_rule(&mut self, indent: usize, w: usize) {
        let w = w.max(12).min(self.cw.saturating_sub(indent + 2));
        let l = self.sp(indent - 1).and(faint(&"─".repeat(w)));
        self.add("", Color::Default, l, Txt::new());
    }

    /// A failure in brief: the line of its output that says what went wrong
    /// (render.go:3437).
    fn error_line(&mut self, id: StepId, indent: usize, r: &str) {
        static ERR: LazyLock<Regex> = LazyLock::new(|| {
            re(
                r"(?i)(error|fatal|panic|exception|traceback|failed|\bfail\b|not found|no such|denied|cannot|can't|undefined|unexpected|invalid|refused|timed out)",
            )
        });
        static TALLY: LazyLock<Regex> = LazyLock::new(|| {
            re(r"^(FAIL|ok|PASS)(\s+\S+(\s+[\d.]+s|\s+\[[^\]]+\])?)?$|^exit status \d+$")
        });
        static FILE_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"^\S+\.\w+:\d+(:\d+)?: \S"));
        static EXIT: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^(?:Error: )?Exit code (\d+)"));
        let st = self.s.step(id);
        let info = self.info(id);
        let mut txt = st.output.to_string();
        if st.kind == Kind::Shell && !(info.out.stdout.is_empty() && info.out.stderr.is_empty()) {
            txt = format!("{}\n{}", info.out.stdout, info.out.stderr);
        }
        if st.status == Status::Failed
            && let Some(m) = BLOCKED.captures(&st.output)
        {
            txt = m[1].to_string();
        }
        let txt = txt
            .replace("<tool_use_error>", "")
            .replace("</tool_use_error>", "");
        let (mut last, mut hit, mut at) = (String::new(), String::new(), String::new());
        for l in collapse_cr(&txt).split('\n') {
            let l = expand_tabs(l);
            let l = l.trim();
            if l.is_empty() || EXIT.is_match(l) && !ERR.is_match(l) || TALLY.is_match(l) {
                continue;
            }
            last = l.to_string();
            if ERR.is_match(l) {
                hit = l.to_string();
            }
            if at.is_empty() && FILE_LINE.is_match(l) {
                at = l.to_string();
            }
        }
        if !at.is_empty() {
            hit = at;
        }
        if hit.is_empty() {
            hit = last;
        }
        if hit.is_empty() {
            return;
        }
        let pad = self.sp(indent - 3);
        let right = if self.o.selected == r {
            dim("enter shows all")
        } else {
            Txt::new()
        };
        let w = (self.cw as isize - indent as isize - 20).max(20) as usize;
        let l = pad.and(faint("▸ ")).and(dim(&truncate_cells(&hit, w)));
        self.add(r, Color::Default, l, right);
    }

    /// Text a tool printed, in a well: head and tail when long, all of it in
    /// verbose mode, red when it's a failure (render.go:3509).
    fn output(&mut self, s: &str, indent: usize, failed: bool) {
        let s = unscreen(&collapse_cr(s.trim_end_matches('\n')));
        let s = self.notices(&s, indent);
        if s.trim().is_empty() {
            return;
        }
        let mut s = s;
        let mut is_json = false;
        let mut hl = lang_for("json");
        let pretty = if self.view == "pretty" {
            pretty_text(&s)
        } else {
            None
        };
        if let Some((p, lg)) = pretty {
            s = p;
            is_json = true;
            hl = lg;
            self.hs = HlState::default();
            self.lg = None;
            self.by_path = false;
            self.spans.clear();
        } else if !failed
            && self.lg.is_none()
            && !self.by_path
            && let Some(p) = pretty_json(&s)
        {
            is_json = true;
            self.hs = HlState::default();
            if self.view != "text" {
                s = p;
            }
        }
        let lines: Vec<&str> = s.split('\n').collect();
        let pad = self.sp(indent - 1);
        let edge = faint("▏");
        let cw = self.cw;
        self.widen(&lines, indent + 2);
        let w = self.cw as isize - indent as isize - 2;
        let w = w.max(0) as usize;
        let most = if self.nowrap { 1 } else { 6 };
        // Which of a chain's parts each line is in.
        let mut span_of: Vec<usize> = Vec::new();
        if !self.spans.is_empty() {
            let (mut j, mut k) = (0usize, 0usize);
            while span_of.len() < lines.len() {
                let sp = self.spans[j];
                let l = clean_output(lines[span_of.len()]);
                if sp.n > 0 && k >= sp.n
                    || sp.n == 0
                        && j + 1 < self.spans.len()
                        && self.spans[j + 1].starts(&l)
                        && !sp.starts(&l)
                {
                    j += 1;
                    if j == self.spans.len() {
                        break;
                    }
                    k = 0;
                }
                span_of.push(j);
                k += 1;
            }
        }
        let mut ud: Option<UDiff> = None;
        if !failed && !span_of.is_empty() {
            let spans = self.spans.clone();
            let so = span_of.clone();
            ud = parse_diff(&lines, |i| i < so.len() && spans[so[i]].diff);
        }
        let mut found = false;
        if ud.is_none() && !failed && !is_json && has_diff(&lines) {
            ud = parse_diff(&lines, |_| true);
            found = ud.is_some();
        }
        // Each line's prefix length, path and language, when it's code.
        let code = |d: &Self,
                    i: usize,
                    l: &str,
                    ud: &Option<UDiff>|
         -> (usize, String, Option<&'static Lang>) {
            let (mut lg, mut by_path) = (d.lg, d.by_path);
            if i < span_of.len() {
                lg = d.spans[span_of[i]].lg;
                by_path = d.spans[span_of[i]].by_path;
            }
            if failed
                || lg.is_none() && !by_path
                || i < span_of.len() && d.spans[span_of[i]].diff
                || found && ud.as_ref().is_some_and(|u| u.ls[i].kind != 0)
            {
                return (0, String::new(), None);
            }
            let (n, path) = rush_hl::code::code_prefix(l);
            if by_path && !path.is_empty() {
                lg = lang_for(path);
            }
            (n, path.to_string(), lg)
        };
        let group = |i: usize, n: usize| -> usize {
            let g = if i < span_of.len() { span_of[i] + 1 } else { 0 };
            if n > 0 { g * 2 + 1 } else { g * 2 }
        };
        let ng = 2 * self.spans.len() + 2;
        let mut prefix_w = vec![0usize; ng];
        let mut shared: Vec<isize> = vec![-1; ng];
        let mut num_w: HashMap<String, usize> = HashMap::new();
        let mut hunk = vec![-1isize; lines.len()];
        let mut hunk_ind: HashMap<isize, usize> = HashMap::new();
        let (mut prev_path, mut prev_no, mut h) = (String::new(), -2isize, 0isize);
        for (i, l) in lines.iter().enumerate() {
            let l = expand_tabs(&clean_output(l));
            let (n, path, lg) = code(self, i, &l, &ud);
            if lg.is_none() || l[n..].trim().is_empty() {
                continue;
            }
            let g = group(i, n);
            let no = rush_hl::code::line_no(&l[..n]);
            if !path.is_empty() {
                let e = num_w.entry(path.clone()).or_insert(0);
                *e = (*e).max(no.to_string().len());
            } else {
                prefix_w[g] = prefix_w[g].max(width(&l[..n]));
            }
            let ind = l[n..].len() - l[n..].trim_start_matches(' ').len();
            if n > 0 && no > 0 {
                let no = no as isize;
                if path != prev_path || no <= prev_no || no > prev_no + 20 {
                    h += 1;
                    hunk_ind.insert(h, ind);
                }
                prev_path = path;
                prev_no = no;
                hunk[i] = h;
                let e = hunk_ind.entry(h).or_insert(ind);
                *e = (*e).min(ind);
                continue;
            }
            if shared[g] < 0 || (ind as isize) < shared[g] {
                shared[g] = ind as isize;
            }
        }
        let mut last: isize = -1;
        let mut head = String::new();
        let marks = std::mem::take(&mut self.marks);
        let emit = |d: &mut Self,
                    i: usize,
                    raw: &str,
                    last: &mut isize,
                    head: &mut String,
                    ud: &mut Option<UDiff>| {
            let l = expand_tabs(&clean_output(raw));
            let put = |d: &mut Self, lead: Txt, body: Txt| {
                d.hang_rows(BG_WELL, pad.clone(), edge.clone(), lead, body, w, most);
            };
            if marks.contains(l.trim()) {
                put(d, Txt::new(), mark_heading(l.trim(), w));
                return;
            }
            if i < span_of.len() && !failed {
                let k = span_of[i] as isize;
                if k != *last {
                    d.reset_hl();
                    *last = k;
                }
            }
            // A diff's line, drawn as an edit is.
            if let Some(u) = ud.as_mut() {
                if found {
                    let lg = d.lg;
                    if u.line(d, pad.clone().and(edge.clone()), w, lg, &lines, i) {
                        return;
                    }
                } else if i < span_of.len() && d.spans[span_of[i]].diff {
                    let lg = d.spans[span_of[i]].lg;
                    if !u.line(d, pad.clone().and(edge.clone()), w, lg, &lines, i) {
                        put(d, Txt::new(), paint(C_OUT, &l));
                    }
                    return;
                }
            }
            let (n, path, lg) = code(d, i, &l, ud);
            if let Some(lg) = lg {
                d.carry(&path, &l[..n]);
                let g = group(i, n);
                let mut pre = format!(
                    "{}{}",
                    &l[..n],
                    " ".repeat(prefix_w[g].saturating_sub(width(&l[..n])))
                );
                let mut body = l[n..].to_string();
                let mut sh = shared[g];
                if hunk[i] >= 0 {
                    sh = hunk_ind[&hunk[i]] as isize;
                }
                if sh > 0 && (body.len() - body.trim_start_matches(' ').len()) as isize >= sh {
                    body = body[sh as usize..].to_string();
                }
                if lg.name == "markdown" && d.hs.fence().is_none() && !md_table(&body) {
                    body = squeeze(&body);
                }
                if !path.is_empty() {
                    if path != *head {
                        put(d, Txt::new(), paint(C_SUB, &path));
                        *head = path.clone();
                    }
                    pre = format!(
                        "{:>w$}  ",
                        rush_hl::code::line_no(&l[..n]),
                        w = num_w.get(&path).copied().unwrap_or(0)
                    );
                }
                let mut hs = d.hs;
                let b = highlight(Some(lg), &mut hs, &body, C_OUT, None);
                d.hs = hs;
                put(d, faint(&pre), b);
                return;
            }
            if is_json {
                let mut hs = d.hs;
                let b = highlight(hl, &mut hs, &l, C_OUT, None);
                d.hs = hs;
                put(d, Txt::new(), b);
                return;
            }
            let (n, path) = rush_hl::code::code_prefix(&l);
            if !failed && !path.is_empty() {
                if width(&l[..n]) <= w / 3 {
                    put(d, faint(&l[..n]), paint(C_OUT, &l[n..]));
                } else {
                    put(d, Txt::new(), faint(&l[..n]).and(paint(C_OUT, &l[n..])));
                }
                return;
            }
            put(d, Txt::new(), paint(C_OUT, &l));
        };
        let more = |d: &mut Self, n: usize, head: &mut String| {
            d.reset_hl();
            head.clear();
            let l = pad.clone().and(edge.clone()).and(folded(n));
            d.add(SHOW_ALL_REF, BG_WELL, l, Txt::new());
        };
        let verbose = self.o.verbose;
        if !verbose && ud.as_ref().is_some_and(|u| u.whole) {
            let mut rows = 0;
            for (i, l) in lines.iter().enumerate() {
                if rows >= 40 {
                    more(self, lines.len() - i, &mut head);
                    self.cw = cw;
                    return;
                }
                if ud.as_ref().is_some_and(|u| u.drawn(i)) {
                    rows += 1;
                }
                emit(self, i, l, &mut last, &mut head, &mut ud);
            }
            self.cw = cw;
            return;
        }
        if !verbose && lines.len() > 8 {
            static ERR: LazyLock<Regex> = LazyLock::new(|| {
                re(
                    r"(?i)(error|fatal|panic|exception|traceback|failed|\bfail\b|not found|no such|denied|cannot|can't|undefined|unexpected|invalid|refused|timed out)",
                )
            });
            for (i, l) in lines[..3].iter().enumerate() {
                emit(self, i, l, &mut last, &mut head, &mut ud);
            }
            let mid = &lines[3..lines.len() - 5];
            let heads = mid.iter().filter(|l| marks.contains(l.trim())).count();
            let (mut cut, mut kept) = (0, 0);
            for (i, l) in mid.iter().enumerate() {
                if failed && kept < 3 && ERR.is_match(l) || heads <= 2 && marks.contains(l.trim()) {
                    if cut > 0 {
                        more(self, cut, &mut head);
                        cut = 0;
                    }
                    emit(self, 3 + i, l, &mut last, &mut head, &mut ud);
                    kept += 1;
                    continue;
                }
                cut += 1;
            }
            if cut > 0 {
                more(self, cut, &mut head);
            }
            let n = lines.len();
            for (i, l) in lines[n - 5..].iter().enumerate() {
                emit(self, n - 5 + i, l, &mut last, &mut head, &mut ud);
            }
            self.cw = cw;
            return;
        }
        for (i, l) in lines.iter().enumerate() {
            if i >= 2000 {
                let l = pad
                    .clone()
                    .and(edge.clone())
                    .and(faint(&format!("… {} more lines", lines.len() - i)));
                self.add("", BG_WELL, l, Txt::new());
                break;
            }
            emit(self, i, l, &mut last, &mut head, &mut ud);
        }
        self.cw = cw;
    }

    pub(crate) fn reset_hl(&mut self) {
        self.hs = HlState::default();
        self.hs_path.clear();
        self.hs_n = 0;
    }

    /// Keeps the highlighter's state only when this line goes on from the
    /// one before in the same file.
    fn carry(&mut self, path: &str, prefix: &str) {
        let n = rush_hl::code::line_no(prefix);
        if path != self.hs_path || n != 0 && self.hs_n != 0 && n != self.hs_n + 1 {
            self.hs = HlState::default();
        }
        self.hs_path = path.to_string();
        self.hs_n = n;
    }

    /// Claude Code's own lines in tool output (a command put in the
    /// background, the folder it's left in) as short notes (shell.go:633).
    fn notices(&mut self, s: &str, indent: usize) -> String {
        const BG: &str = "Command running in background with ID: ";
        const WRITTEN: &str = "Output is being written to: ";
        const NOTIFIED: &str = ". You will be notified";
        const CWD: &str = "Session cwd remains ";
        if !s.contains(BG) && !s.contains(CWD) {
            return s.to_string();
        }
        let mut rest = Vec::new();
        for l in s.split('\n') {
            let t = l.trim();
            if let Some(after) = t.strip_prefix(BG) {
                let (id, after) = after.split_once('.').unwrap_or((after, ""));
                let pl = self
                    .sp(indent - 1)
                    .and(dim(&format!("↳ running in the background · {id}")));
                self.add("", Color::Default, pl, Txt::new());
                if let Some((_, p)) = after.split_once(WRITTEN) {
                    let p = p.split(NOTIFIED).next().unwrap_or(p);
                    let p = p.trim().trim_end_matches('.');
                    let pad = self.sp(indent - 1);
                    let w = self.cw.saturating_sub(indent + 3);
                    self.add_rows(Color::Default, pad, Txt::raw("  "), faint(p), w, 2);
                }
            } else if let Some(dir) = t.strip_prefix(CWD) {
                let dir = dir.trim();
                if dir.trim_end_matches('/') != self.p.cwd.trim_end_matches('/') {
                    let l = self
                        .sp(indent - 1)
                        .and(faint(&format!("cwd {}", self.p.rel(dir))));
                    self.add("", Color::Default, l, Txt::new());
                }
            } else {
                rest.push(l);
            }
        }
        rest.join("\n")
    }

    /// An edit's change as a diff: line numbers, each side highlighted, the
    /// words that changed in a pair picked out (render.go:3916).
    fn diff(&mut self, id: StepId, indent: usize) -> bool {
        let info = self.info(id);
        let o = &info.out;
        let lg = lang_for(&info.call.input.path);
        let pad = self.sp(indent - 1);
        let w = (self.cw as isize - indent as isize - 9).max(0) as usize;
        let verbose = self.o.verbose;
        if o.created {
            let mut hs = HlState::default();
            let content = info.call.input.content.trim_end_matches('\n');
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, l) in lines.iter().enumerate() {
                if i >= 30 && !verbose {
                    let t = pad.clone().and(faint(&format!(
                        "  … {} more lines · ctrl+o shows the rest",
                        lines.len() - i
                    )));
                    self.add("", BG_WELL, t, Txt::new());
                    break;
                }
                self.diff_line(pad.clone(), w, lg, &mut hs, '+', i + 1, l, "", false);
            }
            return true;
        }
        if o.patches.is_empty() {
            return false;
        }
        let mut shown = 0;
        let rest = |d: &mut Self| {
            let t = pad.clone().and(faint("  … ctrl+o shows the rest"));
            d.add("", BG_WELL, t, Txt::new());
        };
        for (pi, p) in o.patches.iter().enumerate() {
            if pi > 0 {
                let t = pad.clone().and(faint("  ⋯"));
                self.add("", BG_WELL, t, Txt::new());
            }
            let (mut old_st, mut new_st) = (HlState::default(), HlState::default());
            let (mut old_n, mut new_n) = (p.old_start.max(0) as usize, p.new_start.max(0) as usize);
            let ls = &p.lines;
            let mut i = 0;
            while i < ls.len() {
                if ls[i].starts_with('-') {
                    let mut j = i;
                    while j < ls.len() && ls[j].starts_with('-') {
                        j += 1;
                    }
                    let mut k = j;
                    while k < ls.len() && ls[k].starts_with('+') {
                        k += 1;
                    }
                    let (dels, adds) = (&ls[i..j], &ls[j..k]);
                    for (n, l) in dels.iter().enumerate() {
                        if shown >= 30 && !verbose {
                            rest(self);
                            return true;
                        }
                        shown += 1;
                        let pair = adds.get(n).map_or("", |a| &a[1..]);
                        self.diff_line(
                            pad.clone(),
                            w,
                            lg,
                            &mut old_st,
                            '-',
                            old_n,
                            &l[1..],
                            pair,
                            n < adds.len(),
                        );
                        old_n += 1;
                    }
                    for (n, l) in adds.iter().enumerate() {
                        if shown >= 30 && !verbose {
                            rest(self);
                            return true;
                        }
                        shown += 1;
                        let pair = dels.get(n).map_or("", |a| &a[1..]);
                        self.diff_line(
                            pad.clone(),
                            w,
                            lg,
                            &mut new_st,
                            '+',
                            new_n,
                            &l[1..],
                            pair,
                            n < dels.len(),
                        );
                        new_n += 1;
                    }
                    i = k;
                    continue;
                }
                if shown >= 30 && !verbose {
                    rest(self);
                    return true;
                }
                shown += 1;
                let l = if ls[i].is_empty() {
                    " "
                } else {
                    ls[i].as_str()
                };
                if let Some(code) = l.strip_prefix('+') {
                    self.diff_line(pad.clone(), w, lg, &mut new_st, '+', new_n, code, "", false);
                    new_n += 1;
                    i += 1;
                    continue;
                }
                let code = &l[l.chars().next().map_or(0, char::len_utf8)..];
                let hl = highlight(lg, &mut new_st, &expand_tabs(code), C_SUB, None);
                for (k, r) in self.code_rows(&hl, w, 6).into_iter().enumerate() {
                    let t = if k == 0 {
                        pad.clone()
                            .and(faint(&format!("{new_n:>5} ")))
                            .and_raw("  ")
                            .and(r)
                    } else {
                        pad.clone().and(blanks(8)).and(r)
                    };
                    self.add("", BG_WELL, t, Txt::new());
                    if k > 0 {
                        self.wrapped();
                    }
                }
                old_st = new_st;
                old_n += 1;
                new_n += 1;
                i += 1;
            }
        }
        true
    }

    /// One added or removed line at number n, the words that changed from
    /// its pair a step brighter (render.go:4041).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn diff_line(
        &mut self,
        pad: Txt,
        w: usize,
        lg: Option<&'static Lang>,
        st: &mut HlState,
        sign: char,
        n: usize,
        code: &str,
        pair: &str,
        paired: bool,
    ) {
        let (body, other) = (expand_tabs(code), expand_tabs(pair));
        let (bg, hi, mark) = if sign == '-' {
            (BG_DEL, BG_DEL_HI, minus_sign())
        } else {
            (BG_ADD, BG_ADD_HI, plus_sign())
        };
        let mut em = None;
        if paired && let Some((from, to)) = rush_hl::code::changed(&body, &other) {
            em = Some(Emph { from, to, on: hi });
        }
        let hl = highlight(lg, st, &body, C_TEXT, em);
        for (i, r) in self.code_rows(&hl, w, 6).into_iter().enumerate() {
            let t = if i == 0 {
                pad.clone()
                    .and(faint(&format!("{n:>5} ")))
                    .and(mark.clone())
                    .and_raw(" ")
                    .and(r)
            } else {
                pad.clone().and(blanks(8)).and(r)
            };
            self.add("", bg, t, Txt::new());
            if i > 0 {
                self.wrapped();
            }
        }
    }

    /// Lets a block whose lines don't fit use the pane's whole width; the
    /// caller puts cw back (render.go:4082).
    pub(crate) fn widen(&mut self, lines: &[&str], lead: usize) {
        if self.cw >= self.o.width {
            return;
        }
        for l in lines {
            if l.len() + 3 * l.matches('\t').count() + lead <= self.cw {
                continue;
            }
            if width(&expand_tabs(l)) + lead > self.cw {
                self.cw = self.o.width;
                break;
            }
        }
    }

    pub(crate) fn code_rows(&self, s: &Txt, w: usize, most: usize) -> Vec<Txt> {
        code_rows(s, w, most, self.o.verbose && !self.nowrap)
    }

    /// body after pad and lead, wrapped to w with the rows it carries on to
    /// under its start (render.go:4099).
    pub(crate) fn add_rows(
        &mut self,
        bg: Color,
        pad: Txt,
        lead: Txt,
        body: Txt,
        w: usize,
        most: usize,
    ) {
        let lw = lead.width();
        for (i, r) in self.code_rows(&body, w, most).into_iter().enumerate() {
            if i == 0 {
                let l = pad.clone().and(lead.clone()).and(r);
                self.add("", bg, l, Txt::new());
                continue;
            }
            let l = pad.clone().and(blanks(lw)).and(r);
            self.add("", bg, l, Txt::new());
            self.wrapped();
        }
    }

    /// add_rows for output on a rail: the rows a line carries on to hang on
    /// a dotted rail two cells in (render.go:4115).
    #[allow(clippy::too_many_arguments)]
    fn hang_rows(
        &mut self,
        bg: Color,
        pad: Txt,
        edge: Txt,
        lead: Txt,
        body: Txt,
        w: usize,
        most: usize,
    ) {
        let w = w.saturating_sub(lead.width());
        if body.width() <= w {
            let l = pad.and(edge).and(lead).and(body);
            self.add("", bg, l, Txt::new());
            return;
        }
        let lw = lead.width();
        for (i, r) in self
            .code_rows(&body, w.saturating_sub(2), most)
            .into_iter()
            .enumerate()
        {
            if i == 0 {
                let l = pad.clone().and(edge.clone()).and(lead.clone()).and(r);
                self.add("", bg, l, Txt::new());
                continue;
            }
            let l = pad.clone().and(faint("┊")).and(blanks(lw + 2)).and(r);
            self.add("", bg, l, Txt::new());
            self.wrapped();
        }
    }

    /// Auto mode's refusal as a card under its step (denied.go:31).
    fn denial(&mut self, id: StepId, indent: usize) {
        let st = self.s.step(id);
        let Some(why) = classified(st) else { return };
        let room = self.cw.min(CAP_ROW) as isize - indent as isize - 4;
        if room < 24 {
            return;
        }
        let room = (room as usize).min(72);
        let mut head = paint(C_YELLOW, "⊘")
            .and_raw(" ")
            .and(paint(C_YELLOW.bold(), "auto mode blocked this"));
        let mut rows = Vec::new();
        let category = !why.is_empty() && !why.contains(['.', ',', '(']) && why.len() <= 40;
        if category {
            head.push(faint(" · "));
            head.push(text(&why));
        } else if why.contains("transient") {
            rows = vec![dim("the classifier errored, and blocked to be safe")];
        } else if !why.is_empty() && why != "Blocked by classifier" {
            rows = wrap(&dim(&why), room);
        }
        let foot = if why.contains("transient") {
            dim("retrying often gets through")
        } else {
            dim("a permission rule allows it")
        };
        if rows.is_empty() {
            rows = vec![dim(
                "Claude was told to find a safer way, or stop and ask you",
            )];
        }
        self.card("", indent, head, Txt::new(), rows, foot, room, C_WARN_Q);
    }
}

/// Joins the refs in a row's owner.
pub const OWNER_SEP: &str = "\x00";

/// Whether a line, trimmed, is nothing or just the spine's glyph.
pub(crate) fn trimmed_is(l: &Line, spine: &str) -> bool {
    let mut seen = String::new();
    for s in &l.spans {
        let t = s.text.trim();
        if t.is_empty() {
            continue;
        }
        seen.push_str(t);
        if seen.len() > spine.len() {
            return false;
        }
    }
    seen == spine
}

/// Sets the cell at col of a drawn row to a faint rail glyph.
fn put_rail(l: &mut Line, col: usize, g: &str) {
    let t = Txt(std::mem::take(&mut l.spans));
    let w = t.width();
    let mut t = if w < col + 1 {
        t.and(blanks(col + 1 - w))
    } else {
        t
    };
    let at = t.skip(col);
    let bg = at.0.first().map_or(Color::Default, |s| s.style.bg);
    let mut out = t.take(col);
    out.push(paint(C_FAINT.on(bg), g));
    out.push(t.skip(col + 1));
    t = out;
    l.spans = t.0;
}

/// Drops a blank row that follows another (render.go:476).
pub(crate) fn squeezed(ls: &mut Vec<RenderedLine>) {
    let mut prev = false;
    ls.retain(|l| {
        let blank = l.r#ref.is_empty()
            && l.line
                .spans
                .iter()
                .all(|s| s.text.chars().all(|c| matches!(c, ' ' | '▏' | '│')));
        let drop = blank && prev;
        prev = blank;
        !drop
    });
}

/// Whether a step waits on you.
pub(crate) fn waiting(s: &Session, t: &Turn) -> bool {
    t.steps
        .iter()
        .any(|&id| s.step(id).status == Status::Waiting)
}

pub(crate) const SPAWN_SHOWN: usize = 4;
const STALL_AFTER: Ms = 120_000;

/// One of words for the stretch that began at since, the same for all of it.
fn pick(words: &[&'static str], since: Ms) -> &'static str {
    words[(since as u64 % words.len() as u64) as usize]
}

const OPENINGS: &[&str] = &[
    "waiting for the agent",
    "waking it up",
    "getting its bearings",
    "reading the room",
    "finding its feet",
    "rolling up its sleeves",
    "clearing its throat",
    "warming up",
    "pulling up the cuck chair",
    "stretching its legs",
    "finding its glasses",
    "switching it off and on again",
];
const WAITINGS: &[&str] = &[
    "working out what's next",
    "reading the results",
    "chewing it over",
    "weighing it up",
    "taking stock",
    "joining the dots",
    "sizing it up",
    "having a think",
    "watching the langs",
    "watching from the cuck chair",
    "checking its working",
    "squinting at the output",
    "blaming the compiler",
    "reading the tea leaves",
    "nodding knowingly",
];
const WRITINGS: &[&str] = &[
    "writing",
    "scribbling",
    "putting pen to paper",
    "drafting",
    "typing away",
    "jotting it down",
    "spinning a yarn",
    "writing it up",
    "licking the pencil",
    "writing its memoirs",
    "embellishing slightly",
];
const COMPACTIONS: &[&str] = &[
    "compacting the context",
    "boiling it all down",
    "sitting on the suitcase",
    "vacuum-packing",
    "writing the cliff notes",
    "squeezing the sponge",
    "folding the laundry",
    "tidying the attic",
    "cramming it in the loft",
    "taking the minutes",
    "reducing the stock",
    "packing it into a flask",
    "rolling up the maps",
    "wringing it out",
    "making a précis",
    "putting it in a nutshell",
];

fn dots_bar(frac: f64, w: usize, tick: usize) -> Txt {
    let at = (tick % (w + 3)) as isize;
    let fill = (frac * w as f64) as isize;
    let mut t = Txt::new();
    for i in 0..w as isize {
        if frac >= 0.0 && i < fill || frac < 0.0 && i <= at && i > at - 3 {
            t.push(paint(C_ORANGE, "▰"));
        } else {
            t.push(faint("▱"));
        }
    }
    t
}

/// What an error the agent stopped on is, and what gets past it
/// (errcard.go:31, without the provider's kinds).
fn err_advice(err: &str) -> (&'static str, &'static str) {
    let t = err.to_lowercase();
    if t.trim() == "during execution" || t.contains("error_during_execution") {
        return (
            "Claude Code couldn't run the turn",
            "it said no more than that; after a login switch it's often the conversation not where it looked · send a message to try again",
        );
    }
    if t.contains("exited mid-turn") {
        return (
            "The agent stopped mid-turn",
            "your next message starts it again where it left off",
        );
    }
    (
        "The turn stopped on an error",
        "your next message picks it up",
    )
}

fn group_words(name: &str, n: usize) -> String {
    match name {
        "read" => format!("read {}", plural(n, "file")),
        "edit" | "multiedit" => format!("edited {}", plural(n, "file")),
        "write" => format!("wrote {}", plural(n, "file")),
        "search" if n == 1 => "searched".into(),
        "search" => format!("searched {n} times"),
        "fetch" => format!("fetched {}", plural(n, "page")),
        "web search" => "searched the web".into(),
        _ if n > 1 => format!("{name} ×{n}"),
        _ => name.to_string(),
    }
}

pub(crate) fn hidden(st: &Step) -> bool {
    st.kind == Kind::Todo
        || matches!(
            &*st.tool,
            "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet"
        )
}

pub(crate) fn message_tool(t: &str) -> bool {
    matches!(t, "SendMessage" | "SubagentHandback" | "PushNotification")
}

fn stop_tool(t: &str) -> bool {
    t == "TaskStop" || t == "KillShell"
}

pub(crate) fn thumbable(path: &str) -> bool {
    let ext = path
        .rsplit_once('.')
        .map(|x| x.1.to_ascii_lowercase())
        .unwrap_or_default();
    path.contains('.') && matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
}

pub(crate) fn glyph_for(st: &Step) -> &'static str {
    match st.kind {
        Kind::Shell => "$",
        Kind::Edit | Kind::Write | Kind::Notebook | Kind::Delete | Kind::Move => "✎",
        Kind::Read => "◧",
        Kind::Search | Kind::Glob => "⌕",
        Kind::Subagent => "⇉",
        Kind::Fetch | Kind::WebSearch => "↗",
        _ => match &*st.tool {
            "Artifact" => "◆",
            "Skill" | "SlashCommand" => "✦",
            t if t.ends_with("__show") => "◇",
            _ => "•",
        },
    }
}

pub(crate) fn glyph_color(g: &str) -> Txt {
    match g {
        "⇉" => paint(C_HELPER, g),
        "◧" | "↗" => paint(C_BLUE, g),
        "⌕" => paint(C_YELLOW, g),
        "✎" | "✦" => paint(C_ORANGE, g),
        _ => faint(g),
    }
}

fn count_lines(s: &str) -> usize {
    let s = s.trim_end_matches('\n');
    if s.trim().is_empty() {
        return 0;
    }
    s.matches('\n').count() + 1
}

/// The line of a script fed on stdin a traceback names, innermost frame
/// last, and the line that says what went wrong (codedoc.go:49).
fn stdin_fault(out: &str) -> (usize, String) {
    static AT: LazyLock<Regex> =
        LazyLock::new(|| re(r#"(?m)(?:File "<stdin>", line |^-:|at - line |\[stdin\]:)(\d+)"#));
    static SAYS: LazyLock<Regex> =
        LazyLock::new(|| re(r"^(?:\w+\.)*\w*(?:Error|Exception|Interrupt|Exit)\b.*"));
    let Some(line) = AT
        .captures_iter(out)
        .last()
        .and_then(|m| m[1].parse::<usize>().ok())
        .filter(|&n| n > 0)
    else {
        return (0, String::new());
    };
    let says = out
        .split('\n')
        .map(str::trim)
        .rfind(|l| SAYS.is_match(l))
        .unwrap_or("");
    (line, says.to_string())
}

pub(crate) fn bash_out(st: &Step, o: &Output) -> String {
    if !o.stdout.is_empty() || !o.stderr.is_empty() {
        return format!("{}\n{}", o.stdout, o.stderr);
    }
    st.output.to_string()
}

static BLOCKED: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?s)<tool_use_error>\s*Blocked:?\s*(.*?)(?:</tool_use_error>|$)"));

fn blocked(st: &Step) -> bool {
    st.status == Status::Failed && BLOCKED.is_match(&st.output)
}

/// Why auto mode's classifier refused a step, if it did (denied.go:13).
pub(crate) fn classified(st: &Step) -> Option<String> {
    static CLASSIFIER: LazyLock<Regex> = LazyLock::new(|| {
        re(
            r"(?s)denied by the Claude Code auto mode classifier\.\s*Reason:\s*(.*?)\.?\s*(?:If you have other tasks|IMPORTANT:|$)",
        )
    });
    if st.status != Status::Denied && st.status != Status::Failed {
        return None;
    }
    let out = st
        .output
        .replace("<tool_use_error>", "")
        .replace("</tool_use_error>", "");
    let m = CLASSIFIER.captures(&out)?;
    let mut why = m[1].trim().to_string();
    if why.starts_with('[') && why.ends_with(']') {
        why = why[1..why.len() - 1].trim().to_string();
    }
    Some(why)
}

/// A read's output without the line numbers before each line.
pub(crate) fn unnumbered(s: &str) -> String {
    s.split('\n')
        .map(|l| {
            let (n, path) = rush_hl::code::code_prefix(l);
            if n > 0 && path.is_empty() { &l[n..] } else { l }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn md_table(l: &str) -> bool {
    l.trim()
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim()
        .starts_with('|')
}

/// A line's indentation kept and each wider gap after it two spaces.
fn squeeze(s: &str) -> String {
    let t = s.trim_start_matches(' ');
    static WIDE: LazyLock<Regex> = LazyLock::new(|| re(r"   +"));
    format!("{}{}", &s[..s.len() - t.len()], WIDE.replace_all(t, "  "))
}

fn input_value(st: &Step) -> serde_json::Value {
    serde_json::from_str(st.input_json()).unwrap_or(serde_json::Value::Null)
}

fn str_of<'v>(v: &'v serde_json::Value, k: &str) -> &'v str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn first_value(v: &serde_json::Value) -> String {
    for k in [
        "description",
        "prompt",
        "query",
        "path",
        "url",
        "name",
        "skill",
    ] {
        let s = str_of(v, k);
        if !s.is_empty() {
            return one_line(s);
        }
    }
    String::new()
}

fn stopped_command(st: &Step) -> String {
    let v: serde_json::Value = serde_json::from_str(st.result_json()).unwrap_or_default();
    str_of(&v, "command").trim().to_string()
}

/// The row for one of Claude Code's own tools that has one (messages.go:216).
fn tool_label(st: &Step, v: &serde_json::Value, lbl: &dyn Fn(&str) -> Txt) -> Option<Txt> {
    let g = |s: &str| glyph_color(s).and_raw(" ");
    let msg = || {
        first_non_empty(&[
            str_of(v, "message"),
            str_of(v, "text"),
            str_of(v, "content"),
        ])
        .to_string()
    };
    Some(match &*st.tool {
        "SendMessage" => {
            let to = first_non_empty(&[str_of(v, "to"), str_of(v, "recipient"), "an agent"]);
            let gist = &one_line(str_of(v, "summary")).to_string();
            let m = msg();
            g("→")
                .and(lbl(&format!("to {to}")))
                .and_raw("  ")
                .and(faint(&if gist.is_empty() {
                    one_line(&m)
                } else {
                    gist.clone()
                }))
        }
        "SubagentHandback" => g("↩")
            .and(lbl("reported back"))
            .and_raw("  ")
            .and(faint(&one_line(&msg()))),
        "PushNotification" => g("◉")
            .and(lbl("notified you"))
            .and_raw("  ")
            .and(faint(&one_line(str_of(v, "message")))),
        "ListAgents" => g("⇉").and(lbl("listed agents")),
        "TaskStop" | "KillShell" => {
            let mut what = stopped_command(st);
            if let Some((first, _)) = what.split_once('\n') {
                what = format!("{} …", first.trim());
            }
            if what.is_empty() {
                static TOOK: LazyLock<Regex> = LazyLock::new(|| re(r"task: \S+ \((.*)\)$"));
                if let Some(m) = TOOK.captures(&one_line(&st.output)) {
                    what = m[1].to_string();
                }
            }
            if what.is_empty() {
                what = first_non_empty(&[
                    str_of(v, "task_id"),
                    str_of(v, "shell_id"),
                    "a background task",
                ])
                .to_string();
            }
            g("⏹").and(lbl("stopped ")).and(faint(&what))
        }
        "TaskOutput" | "BashOutput" => g("◔").and(lbl("checked ")).and(faint(first_non_empty(&[
            str_of(v, "task_id"),
            str_of(v, "bash_id"),
            "a background task",
        ]))),
        "Monitor" => {
            let mut what = one_line(str_of(v, "description"));
            if what.is_empty() {
                what = program(first_non_empty(&[str_of(v, "command"), str_of(v, "until")]));
            }
            g("◔").and(lbl("watching ")).and(faint(&what))
        }
        "ScheduleWakeup" => {
            if v.get("stop").and_then(|x| x.as_bool()).unwrap_or(false) {
                return Some(g("◔").and(lbl("ended the loop")));
            }
            let mut l = lbl("wake up");
            if let Some(s) = v.get("delaySeconds").and_then(|x| x.as_f64())
                && s > 0.0
            {
                l = lbl(&format!("wake in {}", dur((s * 1000.0) as Ms)));
            }
            g("◔")
                .and(l)
                .and_raw("  ")
                .and(faint(&one_line(str_of(v, "reason"))))
        }
        "ToolSearch" => {
            let q = str_of(v, "query");
            if let Some(names) = q.strip_prefix("select:") {
                return Some(
                    g("⌕")
                        .and(lbl("loaded "))
                        .and(faint(&names.replace(',', ", "))),
                );
            }
            g("⌕").and(lbl("looked up tools ")).and(faint(&one_line(q)))
        }
        "EnterWorktree" => {
            let p = first_non_empty(&[str_of(v, "path"), str_of(v, "name")]);
            g("⎇")
                .and(lbl("into worktree "))
                .and(faint(base_after_slash(p)))
        }
        "ExitWorktree" => g("⎇")
            .and(lbl("left the worktree"))
            .and_raw("  ")
            .and(faint(str_of(v, "action"))),
        "EnterPlanMode" => g("◇").and(lbl("planning")),
        "ExitPlanMode" => g("◇").and(lbl("has a plan for you")),
        _ => return None,
    })
}

fn base_after_slash(p: &str) -> &str {
    p.rsplit_once('/').map_or(p, |x| x.1)
}

/// How one of Claude Code's own tools came out (messages.go:281).
fn tool_summary(st: &Step) -> Txt {
    if st.status != Status::Ok {
        return Txt::new();
    }
    let r: serde_json::Value = serde_json::from_str(st.result_json()).unwrap_or_default();
    match &*st.tool {
        "ListAgents" => {
            static PEERS: LazyLock<Regex> = LazyLock::new(|| re(r"\((\d+)\):"));
            if !PEERS.is_match(&st.output) {
                return Txt::new();
            }
            let n: usize = PEERS
                .captures_iter(&st.output)
                .map(|m| m[1].parse::<usize>().unwrap_or(0))
                .sum();
            faint(&plural(n, "agent"))
        }
        "ToolSearch" => {
            if let Some(ms) = r.get("matches").and_then(|x| x.as_array()) {
                if ms.is_empty() {
                    return faint("none found");
                }
                if str_of(&input_value(st), "query").starts_with("select:") {
                    return Txt::new();
                }
                return faint(&plural(ms.len(), "tool"));
            }
            Txt::new()
        }
        "Monitor" => {
            if let Some(ms) = r.get("timeoutMs").and_then(|x| x.as_f64())
                && ms > 0.0
            {
                if r.get("persistent")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false)
                {
                    return faint("until stopped");
                }
                return faint(&format!("up to {}", dur(ms as Ms)));
            }
            Txt::new()
        }
        "EnterWorktree" => match r.get("worktreeBranch").and_then(|x| x.as_str()) {
            Some(b) if !b.is_empty() => paint(C_BLUE, b),
            _ => Txt::new(),
        },
        _ => Txt::new(),
    }
}

/// A question's answers, each with its question (messages.go:330).
fn answered(st: &Step) -> Option<Vec<(String, String)>> {
    let v = input_value(st);
    let qs: Vec<&str> = v
        .get("questions")?
        .as_array()?
        .iter()
        .map(|q| str_of(q, "question"))
        .collect();
    if qs.is_empty() {
        return None;
    }
    let out = &*st.output;
    let mut at = vec![0usize; qs.len()];
    for (i, q) in qs.iter().enumerate() {
        let from = if i > 0 { at[i - 1] + 1 } else { 0 };
        let n = out.get(from..)?.find(&format!("\"{q}\"=\""))?;
        at[i] = from + n;
    }
    let mut qa = Vec::new();
    for (i, q) in qs.iter().enumerate() {
        let start = at[i] + q.len() + 4;
        let mut seg = out.get(start..).unwrap_or("");
        if i + 1 < at.len() {
            seg = out.get(start..at[i + 1]).unwrap_or("");
        } else if let Some(n) = seg.rfind("\". ") {
            seg = &seg[..n + 1];
        }
        qa.push((
            q.to_string(),
            seg.trim_end_matches([',', ' '])
                .trim_end_matches('"')
                .to_string(),
        ));
    }
    Some(qa)
}

// ---- what you said, in rows ----

static ATTACH: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\[Image #\d+[^\]]*\]|\[image: [^\]]+\]|\[(?:[Pp]asted text )?#(\d+) (?:· )?(?:\+\d+ lines|\d+ lines?(?:: [^\]\n]*)?)\]",
    )
});

/// A chip for what you attached, without the brackets.
fn chip_of(m: &str) -> String {
    if let Some(rest) = m.strip_prefix("[image: ") {
        return format!("▣ {}", image_label(rest.trim_end_matches(']')));
    }
    if m.starts_with("[Image") {
        return format!("▣ {}", m.trim_matches(['[', ']']));
    }
    format!("▤ {}", m.trim_matches(['[', ']']))
}

/// Chips laid out dim, as many to a row as fit in w.
fn chip_rows(chips: &[String], w: usize) -> Vec<Txt> {
    let mut rows = Vec::new();
    let mut row = Txt::new();
    let mut n = 0;
    for c in chips {
        let cw = width(c);
        if n > 0 && n + 3 + cw > w {
            rows.push(std::mem::take(&mut row));
            n = 0;
        }
        if n > 0 {
            row.push(Txt::raw("   "));
            n += 3;
        }
        row.push(dim(c).truncate(w, &Txt::raw("…")));
        n += cw.min(w);
    }
    if n > 0 {
        rows.push(row);
    }
    rows
}

/// Your words wrapped to w, one paragraph a line (userbox.go:21).
fn ask_rows(ask: &str, w: usize, style: impl Fn(&str) -> Txt) -> Vec<Txt> {
    let mut rows: Vec<Txt> = Vec::new();
    for (i, l) in ask.trim().split('\n').enumerate() {
        let l = l.trim();
        if l.is_empty() {
            if i > 0 && rows.last().is_some_and(|r| !r.is_empty()) {
                rows.push(Txt::new());
            }
            continue;
        }
        rows.extend(wrap(&style(&one_line(l)), w));
    }
    rows
}

static PASTED: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?s)\s*<pasted_content(?: id="[^"]*")?>\n?(.*?)\n?</pasted_content(?: id="[^"]*")?>\s*"#)
});

/// A message split at the pastes Claude Code marks in it.
fn paste_parts(s: &str) -> Vec<(String, bool)> {
    if !s.contains("<pasted_content") {
        return vec![(s.to_string(), false)];
    }
    let mut out = Vec::new();
    let mut at = 0;
    for m in PASTED.captures_iter(s) {
        let all = m.get(0).unwrap();
        if all.start() > at {
            out.push((s[at..all.start()].to_string(), false));
        }
        out.push((m[1].to_string(), true));
        at = all.end();
    }
    if at < s.len() || out.is_empty() {
        out.push((s[at..].to_string(), false));
    }
    out
}

/// A paste as text: a dim label, then its lines (userbox.go:186).
fn paste_rows(text: &str, w: usize) -> Vec<Txt> {
    const FULL: usize = 15;
    const HEAD: usize = 12;
    let t = text.replace('\r', "");
    let mut lines: Vec<String> = t.trim().split('\n').map(String::from).collect();
    let bordered = lines
        .iter()
        .filter(|l| l.trim_start_matches(' ').starts_with('│'))
        .count();
    let strip = bordered * 2 > lines.len();
    for l in &mut lines {
        let mut x = l.clone();
        if strip {
            x = x
                .trim_start_matches([' ', '│'])
                .trim_end_matches([' ', '│'])
                .to_string();
        }
        *l = expand_tabs(&x).trim_end_matches(' ').to_string();
    }
    let mut more = 0;
    if lines.len() > FULL {
        more = lines.len() - HEAD;
        lines.truncate(HEAD);
    }
    let mut label = "▤ pasted".to_string();
    if more > 0 {
        label = format!("{label} · {}", plural(lines.len() + more, "line"));
    }
    let mut rows = vec![dim(&label)];
    for l in &lines {
        if l.is_empty() {
            rows.push(Txt::new());
            continue;
        }
        rows.extend(wrap(&sub(l), w));
    }
    if more > 0 {
        rows.push(dim(&format!("… {more} more lines · click to show all")));
    }
    rows
}

/// Your words with what acts singled out: slash commands and @files in
/// bold white, image and paste markers as chips, links underlined
/// (render.go:1739).
pub(crate) fn styled_ask(s: &str, base: Style) -> Txt {
    if let Some(cmd) = s.strip_prefix("! ") {
        let mut t = paint(C_WHITE.bold(), "$ ");
        for x in quiet_tokens(cmd) {
            t.push(x);
        }
        return t;
    }
    static SPECIAL: LazyLock<Regex> = LazyLock::new(|| {
        re(
            r"\[Image #\d+\]|\[image: [^\]]+\]|\[(?:[Pp]asted text )?#\d+ [^\]\n]*lines?[^\]\n]*\]|https?://[^\s)>\]]+|(^|\s)/[a-z][\w:-]*(?:$|[\s.,;:!?)])|@[\w./-]+",
        )
    });
    let mut out = Txt::new();
    let mut last = 0;
    let special = s.contains(['[', '/', '@']);
    for m in SPECIAL.find_iter(if special { s } else { "" }) {
        let x = m.as_str();
        if x.starts_with("http") {
            continue; // inline links it
        }
        out.push(super::markdown::inline(&s[last..m.start()], base));
        if let Some(rest) = x.strip_prefix("[image: ") {
            out.push(paint(C_BLUE, "▣ "));
            out.push(paint(C_TEXT, &image_label(rest.trim_end_matches(']'))));
        } else if x.starts_with("[Image") {
            out.push(paint(C_BLUE, "▣ "));
            out.push(paint(C_TEXT, x.trim_matches(['[', ']'])));
        } else if x.starts_with("[Pasted") || x.starts_with("[pasted") || x.starts_with("[#") {
            out.push(paint(C_BLUE, "▤ "));
            out.push(paint(C_TEXT, x.trim_matches(['[', ']'])));
        } else {
            out.push(paint(C_WHITE.bold(), x));
        }
        last = m.end();
    }
    out.push(super::markdown::inline(&s[last..], base));
    out.over(base)
}

/// Each paste in a message as the chip it was in the box.
fn fold_pastes(s: &str) -> String {
    if !s.contains("<pasted_content") {
        return s.trim().to_string();
    }
    let mut n = 0;
    let out = PASTED.replace_all(s, |c: &regex::Captures| {
        n += 1;
        format!(" {} ", paste_chip(n, &c[1]))
    });
    out.trim().to_string()
}

/// A paste as its chip: number, lines, how it starts and ends.
fn paste_chip(n: usize, text: &str) -> String {
    let lines = text.trim_end_matches('\n').matches('\n').count() + 1;
    let unit = if lines == 1 { "line" } else { "lines" };
    let flat: Vec<char> = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| *c != '[' && *c != ']')
        .collect();
    const EACH: usize = 8;
    let mut preview: String = flat.iter().collect();
    if flat.len() > 2 * EACH + 3 {
        let mut start = &flat[..EACH];
        let mut end = &flat[flat.len() - EACH..];
        for i in (0..start.len()).rev() {
            if start[i] == ' ' && i >= EACH / 2 {
                start = &start[..i];
                break;
            }
        }
        if let Some(i) = end.iter().position(|c| *c == ' ')
            && end.len() - i > EACH / 2
        {
            end = &end[i + 1..];
        }
        preview = format!(
            "{} … {}",
            start.iter().collect::<String>().trim(),
            end.iter().collect::<String>().trim()
        );
    }
    if preview.is_empty() {
        return format!("[pasted text #{n} · {lines} {unit}]");
    }
    format!("[pasted text #{n} · {lines} {unit}: {preview}]")
}

/// A turn's images by name: their files, or Image #1, #2.
fn image_names(images: &[Box<str>]) -> Vec<String> {
    images
        .iter()
        .enumerate()
        .map(|(i, im)| {
            if im.is_empty() || &**im == "image" {
                format!("Image #{}", i + 1)
            } else {
                image_label(im)
            }
        })
        .collect()
}

/// An image file's name for a chip, a screenshot's cut to its time.
pub fn image_label(path: &str) -> String {
    static SHOT: LazyLock<Regex> = LazyLock::new(|| {
        re(r"^(?:Screenshot|Screen Shot|CleanShot) \d{4}-\d{2}-\d{2} at (.+)\.(?i:png|jpe?g)$")
    });
    static CLIP: LazyLock<Regex> =
        LazyLock::new(|| re(r"^clipboard-\d{8}-(\d{2})(\d{2})\d{2}(?:\.\d+)?\.png$"));
    let name = base_name(path);
    if let Some(m) = SHOT.captures(&name.replace('\u{202f}', " ")) {
        return format!("screenshot {}", &m[1]);
    }
    if let Some(m) = CLIP.captures(name) {
        return format!("screenshot {}:{}", &m[1], &m[2]);
    }
    name.to_string()
}

/// An image Claude points at, as ▣ and its name.
fn image_chip(alt: &str, src: &str) -> Txt {
    let name = base_name(src);
    let alt = if alt.is_empty() { name } else { alt };
    let mut chip = paint(C_BLUE, "▣ ").and(paint(C_TEXT, alt));
    if name != alt {
        chip.push(paint(C_DIM, &format!(" {name}")));
    }
    chip
}

/// What a turn no message started says in its place.
fn unasked(t: &Turn) -> &str {
    if !t.cause.is_empty() {
        &t.cause
    } else if t.n == 1 {
        "continued from earlier"
    } else {
        "woke up without a message"
    }
}

/// What a turn rush started says in place of its words to the agent.
fn rush_ask(t: &Turn) -> String {
    if t.prompt
        .starts_with("continue: you're on another account now, with room")
    {
        return "moved to an account with room · carrying on".into();
    }
    if let Some(rest) = t
        .prompt
        .strip_prefix("Your previous run stopped with a provider error (")
    {
        let why = rest.split(')').next().unwrap_or("");
        return format!("provider error ({why}) · trying again");
    }
    String::new()
}

/// A turn a task woke: its noun and how it ended (render.go:661).
fn woke(t: &Turn) -> (&'static str, Txt, bool) {
    let (kind, status) = t.from.split_once(" · ").unwrap_or((&t.from, ""));
    let noun = match kind {
        "background shell" => "Background command",
        "background subagent" => "Background agent",
        "background workflow" => "Background workflow",
        "monitor" => "Monitor",
        _ => return ("", Txt::new(), false),
    };
    if t.cause.is_empty() {
        return ("", Txt::new(), false);
    }
    let how = match status {
        "" => Txt::new(),
        "completed" => Txt::raw(" ").and(paint(C_GREEN, status)),
        "failed" => Txt::raw(" ").and(paint(C_RED, status)),
        _ => Txt::raw(" ").and(dim(status)),
    };
    (noun, how, true)
}

/// rush's show tool.
const SHOW: &str = "mcp__rush__show";

/// The drawing a show step carries, a string a row, blank rows at either
/// end dropped; None while it has none (render.go:4225).
fn drawing(st: &Step) -> Option<Vec<String>> {
    let v = input_value(st);
    let d = v.get("drawing")?.as_str()?;
    let d = expand_tabs(&collapse_cr(d));
    let mut rows: Vec<String> = d
        .split('\n')
        .map(|r| r.trim_end_matches(' ').to_string())
        .collect();
    while rows.first().is_some_and(|r| r.trim().is_empty()) {
        rows.remove(0);
    }
    while rows.last().is_some_and(|r| r.trim().is_empty()) {
        rows.pop();
    }
    if rows.is_empty() { None } else { Some(rows) }
}

/// Who a message went to: an agent's id shortened, else the name it was
/// sent to (messages.go:33, without the subagents' descriptions).
fn recipient(to: &str) -> String {
    if to.is_empty() {
        return "an agent".into();
    }
    let hex = to.strip_prefix('a').unwrap_or(to);
    if hex.len() >= 12
        && hex
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return format!("agent {}", &to[..7]);
    }
    to.to_string()
}

/// How a message went, for its card's foot (messages.go:100).
fn delivery(st: &Step) -> Txt {
    match st.status {
        Status::Running => return paint(C_ORANGE, "sending…"),
        Status::Waiting => return paint(C_YELLOW, "waiting on you"),
        Status::Denied => return dim("not sent"),
        Status::Failed => {
            let m = one_line(
                &st.output
                    .replace("<tool_use_error>", "")
                    .replace("</tool_use_error>", ""),
            );
            let m = m.strip_prefix("Error: ").unwrap_or(&m).to_string();
            return paint(C_RED, if m.is_empty() { "didn't send" } else { &m });
        }
        _ => {}
    }
    let mut r: serde_json::Value = serde_json::from_str(st.result_json()).unwrap_or_default();
    let empty = |r: &serde_json::Value| r.as_object().is_none_or(|o| o.is_empty());
    if empty(&r) {
        r = serde_json::from_str(&st.output).unwrap_or_default();
    }
    let b = |k: &str| r.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    match &*st.tool {
        "PushNotification" => {
            if b("pushSent") {
                return dim("sent to your phone");
            }
            if b("localSent") {
                return dim("shown in the terminal");
            }
            return Txt::new();
        }
        "SubagentHandback" => return dim("delivered"),
        _ => {}
    }
    if !str_of(&r, "resumedAgentId").is_empty() {
        return dim("woke it up");
    }
    if str_of(&r, "message").contains("next tool round") {
        return dim("read at its next step");
    }
    dim("sent")
}

/// A card's words: its title, then the body's first paragraphs, five rows
/// at most (cards.go:1572).
pub(crate) fn card_text(title: &str, body: &[&str], w: usize) -> Vec<Txt> {
    let mut rows = wrap(&paint(C_WHITE, &one_line(title)), w);
    if rows.len() > 2 {
        let second = rows[1].take(w - 1).and(dim("…"));
        rows.truncate(1);
        rows.push(second);
    }
    let mut paras = Vec::new();
    let mut cur = String::new();
    for l in body {
        if l.trim().is_empty() {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur = format!("{cur} {}", l.trim()).trim().to_string();
    }
    if !cur.is_empty() {
        paras.push(cur);
    }
    let mut bl = Vec::new();
    for p in paras {
        bl.extend(wrap(&sub(&p), w));
    }
    let left = 5usize.saturating_sub(rows.len());
    if bl.len() > left && left > 0 {
        bl.truncate(left);
        let last = bl.pop().unwrap();
        bl.push(last.take(w - 1).and(dim("…")));
    } else if left == 0 {
        bl.clear();
    }
    rows.extend(bl);
    rows
}

/// rush's envelope of messages queued while the agent worked, each its own
/// (userbox.go:300, host/queue.go:11).
fn queued_messages(text: &str) -> Option<Vec<String>> {
    let (head, rest) = text.split_once("\n\n")?;
    let (count, suffix) = head.split_once(" messages, ")?;
    if suffix != "queued while you worked. Each is its own message; take them in order." {
        return None;
    }
    let n: usize = count.parse().ok()?;
    if !(2..=10000).contains(&n) {
        return None;
    }
    let mut parts = Vec::new();
    let mut rest = rest.to_string();
    for i in 1..=n {
        let marker = format!("[Message {i} of {n}]\n");
        rest = rest.strip_prefix(&marker)?.to_string();
        if i == n {
            parts.push(rest.clone());
            break;
        }
        let next = format!("\n\n[Message {} of {n}]\n", i + 1);
        let (part, after) = rest.split_once(&next)?;
        parts.push(part.to_string());
        rest = format!("[Message {} of {n}]\n{after}", i + 1);
    }
    let mut joined = format!(
        "{n} messages, queued while you worked. Each is its own message; take them in order."
    );
    for (i, p) in parts.iter().enumerate() {
        joined.push_str(&format!("\n\n[Message {} of {n}]\n{p}", i + 1));
    }
    (joined == text).then_some(parts)
}

/// Whether [Image #n] markers name every one of count images.
fn numbered_images_cover(text: &str, count: usize) -> bool {
    static NUM: LazyLock<Regex> = LazyLock::new(|| re(r"\[Image #(\d+)[^\]]*\]"));
    if count == 0 {
        return false;
    }
    let seen: HashSet<usize> = NUM
        .captures_iter(text)
        .filter_map(|m| m[1].parse().ok())
        .collect();
    (1..=count).all(|n| seen.contains(&n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_names() {
        assert_eq!(
            image_label("/Users/me/Desktop/Screenshot 2026-09-24 at 10.21.03.png"),
            "screenshot 10.21.03"
        );
        assert_eq!(image_label("/tmp/shot.png"), "shot.png");
    }

    #[test]
    fn paste_chips() {
        assert_eq!(
            paste_chip(1, "one two"),
            "[pasted text #1 · 1 line: one two]"
        );
    }
}
