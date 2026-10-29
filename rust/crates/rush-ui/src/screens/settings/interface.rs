//! Settings › General's second half: how rush looks and what its keys do,
//! with a preview of the Agents list (and an example Session beside it)
//! as each look setting has it now. After Go's
//! internal/ui/settings_interface.go (interfaceSections :12), view.go
//! (listPreview :1518, subLineFade :2113), folders.go:27 (splitProjects),
//! model.go:698 (dockLines) and sidebar.go:25 (groupModes).

use std::rc::Rc;

use rush_core::text::Line;

use super::form::{PreviewFn, Section, choice, set};
use super::paint::{first_non_empty, pad_lines, pbox, side_by_side};
use super::{State, config, config_mut, sample};
use crate::model::Model;
use crate::style::*;

/// The ways the list can be grouped (groupModes).
pub const GROUP_MODES: [&str; 3] = ["status", "agent", "group"];

/// How see-through the list's second line is, from its setting
/// (subLineFade).
pub fn sub_line_fade(v: &str) -> f64 {
    match v {
        "dim" => 0.5,
        "faint" => 0.75,
        v => v
            .trim_end_matches('%')
            .parse::<i64>()
            .map(|n| n.clamp(0, 90) as f64 / 100.0)
            .unwrap_or(0.0),
    }
}

/// Whether each section is split by project (splitProjects): in the rail
/// look only when asked, in the classic look unless turned off.
pub fn split_projects(c: &rush_state::Config) -> bool {
    if c.list_look != "classic" {
        return c.split_by == "project";
    }
    c.split_by != "none"
}

/// The card's lines under the list (dockLines).
pub fn dock_lines(c: &rush_state::Config, h: usize) -> i64 {
    let n = if c.dock_lines == 0 { 3 } else { c.dock_lines };
    n.max(1).min((h as i64 / 3).max(1))
}

/// The list as it's drawn now, for the preview, kept with each key on
/// General: the preview is drawn where the model can't be changed
/// (listPreview).
#[derive(Default)]
pub struct ListPreview {
    pub lines: Vec<Line>,
}

/// Takes the list's rows again for the preview, the width the inspector
/// under the form gives it.
pub fn refresh_preview(m: &mut Model) {
    let w = m.w.saturating_sub(6).saturating_sub(4).max(10);
    let rows = 15;
    let (scroll, keys, top) = (m.scroll, m.row_keys.clone(), m.list_top);
    let mut out = vec![m.column_header(w)];
    out.extend(m.list_lines(w, rows));
    m.scroll = scroll;
    m.row_keys = keys;
    m.list_top = top;
    while out.len() < rows + 1 {
        out.push(Line::new());
    }
    m.kept::<State>().preview.lines = out;
}

/// What a look setting changes, as it will look: the Agents list and,
/// where there's room, a Session beside it.
fn preview(m: &Model) -> PreviewFn {
    let list = m
        .kept
        .peek::<State>()
        .map(|s| s.preview.lines.clone())
        .unwrap_or_default();
    Rc::new(move |w: usize, h: usize| {
        let rows = h.saturating_sub(3).max(1); // its header and box take three
        let lines: Vec<Line> = list
            .iter()
            .take(rows + 1)
            .map(|l| fit(l.clone(), w.saturating_sub(4).min(l.width().max(1))))
            .collect();
        let bw = lines.iter().map(Line::width).max().unwrap_or(0);
        let lbox = pbox(
            line([spb("Agents", TEXT)]),
            line([sp("your own, as the list shows them", DIM)]),
            &lines,
            bw + 4,
            false,
        );
        let sw = w as isize - bw as isize - 5;
        if sw >= 40 {
            let sw = sw as usize;
            let sess = pbox(
                line([spb("Session", TEXT)]),
                line([sp("an example", DIM)]),
                &pad_lines(sample::session_preview(sw - 4, lines.len()), lines.len()),
                sw,
                false,
            );
            return side_by_side(&lbox, &sess, bw + 4);
        }
        lbox
    })
}

/// General's sections on the look of rush and its keys.
pub fn sections(m: &Model) -> Vec<Section> {
    let c = config(m);

    let mut view = choice(
        "Layout",
        &c.view,
        "How rush lays out Agents and the Session, now and next time. #view and shift+← → change it too.",
        &[
            (
                "split",
                "Agents on the left, the picked agent's Session beside them, when the screen is wide enough.",
            ),
            (
                "agent",
                "one Session with the whole screen; esc shows Agents, and the next one you open has the whole screen too.",
            ),
            (
                "list",
                "Agents alone; a Session you open takes the screen until esc.",
            ),
            ("", "rush asks which the next time it opens."),
        ],
        set(|m, v| {
            if v.is_empty() {
                config_mut(m).view.clear();
            } else {
                config_mut(m).set_view(v);
            }
        }),
    );
    view.unset = "ask".into();
    let mut enter = choice(
        "Enter on an agent",
        &c.enter_on,
        "What enter does on an agent in the list.",
        &[
            ("rename", "renames it, as in the Finder; ctrl+o opens it."),
            ("open", "opens its Session; ctrl+r renames it."),
            ("", "rush asks the first time you press it."),
        ],
        set(|m, v| config_mut(m).enter_on = v.into()),
    );
    enter.unset = "ask".into();
    let mut group = choice(
        "Group by",
        first_non_empty(&[&c.group_by, "status"]),
        "How agents are sorted into sections.",
        &[
            (
                "status",
                "Needs you first (questions and halts), then Finished (turns that ended without asking), Stuck?, Working, and Idle (open, or stopped recently: Settings › General); the rest of the last day under Today, older ones under Earlier.",
            ),
            (
                "agent",
                "one section per harness (Claude Code, Codex, Copilot…), handy when you run several.",
            ),
            (
                "group",
                "your own sections; put an agent in one with /group <name>. Ungrouped agents fall back to status.",
            ),
        ],
        set(|m, v| config_mut(m).group_by = v.into()),
    );
    // Plugins arrange the list too, once they're in this rush.
    group.choices = GROUP_MODES.iter().map(|s| s.to_string()).collect();
    let split = choice(
        "Split by project",
        if split_projects(c) { "on" } else { "off" },
        "Whether each section's agents sit together by the repository they work in, whatever the grouping. ctrl+p, or the toggle at the top of the list, changes it too.",
        &[
            (
                "on",
                "under each section, a line per project with its branch, commits ahead and behind and uncommitted changes; agents in a linked worktree under the repository it came from. In the rail look, only the project's name heads its rows, and it's off until turned on.",
            ),
            ("off", "one run of rows per section, as the sort puts them."),
        ],
        set(|m, v| {
            config_mut(m).split_by = if v == "on" { "project" } else { "none" }.into();
        }),
    );
    let look = choice(
        "Look of the list",
        first_non_empty(&[&c.list_look, "rail"]),
        "How the Agents list is drawn. What's in it, and its keys, are the same either way.",
        &[
            (
                "rail",
                "an edge down each section, coloured by what it wants of you, each agent on a branch off it with a line under it for its project (in its own colour, as on the Wall), branch, uncommitted changes and PR; every other agent on a faint band.",
            ),
            (
                "classic",
                "a line per agent, under a heading line per project when split by project.",
            ),
        ],
        set(|m, v| config_mut(m).list_look = v.into()),
    );
    let sort_by = choice(
        "Sort rows by",
        first_non_empty(&[&c.sort_by, "name"]),
        "The order of rows inside each section. You can also click a column header on the Agents view.",
        &[
            (
                "name",
                "alphabetical, so a row stays put while its agent works.",
            ),
            (
                "recent",
                "most recently active first; rows move as agents update.",
            ),
            ("cost", "most expensive first."),
            (
                "cpu",
                "busiest first, by CPU across everything the agent started.",
            ),
            (
                "ram",
                "heaviest first, by memory across everything the agent started.",
            ),
            (
                "tokens",
                "biggest context first, by its newest message's tokens.",
            ),
            ("time", "longest-running first."),
        ],
        set(|m, v| config_mut(m).sort_by = v.into()),
    );
    let width = if c.side_width > 0.0 {
        format!("{:.0}%", c.side_width * 100.0)
    } else {
        String::new()
    };
    let mut list_width = choice(
        "List width",
        &width,
        "The list's share of the screen beside a Session. Dragging its edge, or shift+← →, changes it too.",
        &[
            ("", "rush's choice, by the screen's width."),
            ("25%", "a quarter of the screen."),
            ("33%", "a third of the screen."),
            ("50%", "half the screen."),
        ],
        set(|m, v| {
            let pct: f64 = v.trim_end_matches('%').parse().unwrap_or(0.0);
            config_mut(m).side_width = pct / 100.0;
        }),
    );
    list_width.unset = "rush's".into();
    let dock = choice(
        "Card lines",
        &dock_lines(c, m.h).to_string(),
        "How many lines of the agent's latest words its card under the list shows.",
        &[
            ("3", "three lines."),
            ("6", "six lines."),
            ("10", "ten lines."),
            ("15", "fifteen lines."),
        ],
        set(|m, v| {
            if let Some(n) = super::general::scan_int(v) {
                config_mut(m).dock_lines = n;
            }
        }),
    );
    let stack_at = match c.stack_at {
        n if n < 0 => "narrow only".to_string(),
        n if n >= 100 => "always".to_string(),
        n if n > 0 => format!("{n}%"),
        _ => String::new(),
    };
    let mut stack = choice(
        "Two-line rows",
        &stack_at,
        "When each agent's row takes two lines, its name and figures above and its latest words under them.",
        &[
            (
                "",
                "when the list has 42% of the screen or less, or is too narrow for words beside the name.",
            ),
            (
                "30%",
                "when the list has 30% of the screen or less, or is too narrow for words beside the name.",
            ),
            (
                "50%",
                "when the list has half the screen or less, or is too narrow for words beside the name.",
            ),
            (
                "60%",
                "when the list has 60% of the screen or less, or is too narrow for words beside the name.",
            ),
            (
                "narrow only",
                "only when the list is too narrow for words beside the name: one line wherever it fits.",
            ),
            (
                "always",
                "every row takes two lines, however wide the list is.",
            ),
        ],
        set(|m, v| {
            let c = config_mut(m);
            match v {
                "" => c.stack_at = 0,
                "narrow only" => c.stack_at = -1,
                "always" => c.stack_at = 100,
                v => {
                    if let Some(n) = super::general::scan_int(v) {
                        c.stack_at = n;
                    }
                }
            }
        }),
    );
    stack.unset = "42%".into();

    let mut theme = choice(
        "Theme",
        &c.theme,
        "What rush's colours are made for. Its text and panels are shades between your terminal's background and text colour, so they follow its theme; its orange, green and red stay, made as easy to read on your background as on rush's own.",
        &[
            (
                "",
                "rush asks the terminal for its background and text, again whenever you come back to it; a terminal that doesn't say gets rush's dark.",
            ),
            (
                "dark",
                "rush's own dark colours, whatever the terminal says.",
            ),
            (
                "light",
                "rush's own light colours, for a light terminal that doesn't say it's light.",
            ),
        ],
        set(|m, v| config_mut(m).theme = v.into()),
    );
    theme.unset = "match terminal".into();
    let colours = choice(
        "Colours",
        if c.color_blind {
            "colour-blind"
        } else {
            "standard"
        },
        "How rush tells good from bad: added and removed lines in a diff, done and failed steps and agents.",
        &[
            ("standard", "green and red."),
            (
                "colour-blind",
                "sky blue and amber, which stay apart for red-green and blue-yellow colour blindness alike; + and − still mark every diff line.",
            ),
        ],
        set(|m, v| config_mut(m).color_blind = v == "colour-blind"),
    );
    let logo = choice(
        "Logo",
        if c.hide_logo { "hidden" } else { "shown" },
        "The rush bottle at the top left.",
        &[
            ("shown", "the bottle beside the counts, five rows tall."),
            (
                "hidden",
                "text only: the header takes three rows, and the rest goes to the list.",
            ),
        ],
        set(|m, v| config_mut(m).hide_logo = v == "hidden"),
    );
    let spaces = choice(
        "Spaces and tabs in diffs",
        if c.show_whitespace { "shown" } else { "hidden" },
        "Whether a diff marks its spaces and tabs, as · and →, so a change in indentation shows.",
        &[
            ("hidden", "diffs show text only."),
            ("shown", "every space is a faint · and every tab a →."),
        ],
        set(|m, v| config_mut(m).show_whitespace = v == "shown"),
    );

    let sub_now = format!("{}%", (sub_line_fade(&c.sub_line) * 100.0 + 0.5) as i64);
    let sub_line = choice(
        "Second line transparency",
        &sub_now,
        "How see-through the line under each agent's name is: what it's doing, or what it last said. It keeps its state's colour, only quieter.",
        &[
            ("0%", "solid: as bright as the name."),
            ("25%", "a step back from the name."),
            ("50%", "half way to the background."),
            (
                "75%",
                "a shade above the ground, there when you look for it.",
            ),
        ],
        set(|m, v| {
            config_mut(m).sub_line = if v == "0%" { String::new() } else { v.into() };
        }),
    );
    let search = choice(
        "ctrl+k searches transcripts",
        if c.search_transcripts_on_key {
            "on ctrl+enter"
        } else {
            "as you type"
        },
        "What the command bar (ctrl+k) looks through as you type.",
        &[
            (
                "as you type",
                "agent names, commands and every transcript, as you type.",
            ),
            (
                "on ctrl+enter",
                "names and commands as you type; ctrl+enter (or ctrl+j) then searches the transcripts.",
            ),
        ],
        set(|m, v| config_mut(m).search_transcripts_on_key = v == "on ctrl+enter"),
    );
    let command = choice(
        "Enter after a /command",
        if c.enter_sends_command {
            "sends"
        } else {
            "completes"
        },
        "What enter does when a message ends in a /command the picker shows, typed in full.",
        &[
            (
                "completes",
                "puts the command in with a space after it; enter again sends the message.",
            ),
            (
                "sends",
                "sends the message as it is; a command typed in part is still completed.",
            ),
        ],
        set(|m, v| config_mut(m).enter_sends_command = v == "sends"),
    );
    let row_key = choice(
        "Open a picked row with",
        first_non_empty(&[&c.row_key, "enter and space"]),
        "The key that opens or closes the conversation row you've picked. Whichever it is, nothing you've typed is sent while a row is picked.",
        &[
            ("enter and space", "either opens or closes it."),
            ("enter", "enter does; space says so."),
            ("space", "space does; enter says so."),
        ],
        set(|m, v| {
            config_mut(m).row_key = if v == "enter and space" {
                String::new()
            } else {
                v.into()
            };
        }),
    );
    let copying = choice(
        "Copy on select",
        if c.copies_on_select() { "on" } else { "off" },
        "Whether text you drag over, in a conversation or a message box, goes to the clipboard as you let go.",
        &[
            ("on", "it's copied as the drag ends, as terminals do."),
            (
                "off",
                "it stays selected until cmd+c (where the terminal passes it on) or ctrl+c copies it; for a terminal that copies with cmd+c itself.",
            ),
        ],
        set(|m, v| config_mut(m).copy_on_select = Some(v == "on")),
    );

    let mut cols = Vec::new();
    for (key, name, what) in [
        (
            "running",
            "RUNNING",
            "the subagents and shells each agent has going.",
        ),
        ("cpu", "CPU", "how busy everything each agent started is."),
        (
            "ram",
            "RAM",
            "the memory everything each agent started holds.",
        ),
        ("tokens", "TOKENS", "how full each agent's context is."),
        ("cost", "COST", "what each agent has spent."),
        (
            "time",
            "TIME",
            "how long each has run; for one that needs you, how long it has waited; for a stuck one, how long it has been silent.",
        ),
    ] {
        let shown = if c.shows(key) { "shown" } else { "hidden" };
        cols.push(choice(
            name,
            shown,
            &format!("The {name} column: {what} A narrow list drops some by itself."),
            &[
                ("shown", "when the list is wide enough for it."),
                ("hidden", "never: its room goes to names and words."),
            ],
            set(move |m, v| {
                let c = config_mut(m);
                c.hide_columns.retain(|h| h != key);
                if v == "hidden" {
                    c.hide_columns.push(key.into());
                }
            }),
        ));
    }
    let p = preview(m);
    let mut look_rows = vec![view, theme, colours, logo, spaces];
    let mut list_rows = vec![
        group, look, split, sort_by, list_width, dock, stack, sub_line, enter, search,
    ];
    for st in look_rows.iter_mut().chain(cols.iter_mut()) {
        st.preview = Some(p.clone());
    }
    for (i, st) in list_rows.iter_mut().enumerate() {
        // group, look, split, sort, two-line rows, second line.
        if matches!(i, 0..=3 | 6 | 7) {
            st.preview = Some(p.clone());
        }
    }

    let minimap = choice(
        "Conversation minimap",
        if c.hide_minimap { "hidden" } else { "shown" },
        "A miniature of the conversation along its right edge. Click or drag to scroll. Hidden automatically in narrow panes.",
        &[
            (
                "shown",
                "coloured text shapes and a shaded viewport; click or drag to navigate.",
            ),
            ("hidden", "all available width goes to the conversation."),
        ],
        set(|m, v| config_mut(m).hide_minimap = v == "hidden"),
    );
    let pictures = choice(
        "Pictures",
        if c.pixel_pictures {
            "pixelated"
        } else {
            "sharp"
        },
        "How images in the conversation are drawn. Sharp needs a terminal with Kitty graphics (Ghostty, Kitty); elsewhere they're always pixelated.",
        &[
            ("sharp", "full detail where the terminal can show it."),
            ("pixelated", "coloured blocks, softer and quieter."),
        ],
        set(|m, v| config_mut(m).pixel_pictures = v == "pixelated"),
    );
    look_rows.push(minimap);
    look_rows.push(pictures);
    vec![
        Section::new("Look", look_rows),
        Section::new("Agents list", list_rows),
        Section {
            title: "Agents list columns".into(),
            note: "the figures beside each agent".into(),
            rows: cols,
            advanced: false,
        },
        Section::new("Message box", vec![command, row_key, copying]),
    ]
}
