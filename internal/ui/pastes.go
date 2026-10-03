package ui

import (
	"fmt"
	"math/rand/v2"
	"os"
	"slices"
	"strconv"
	"strings"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/convo"
)

// pastes keeps the long pastes a box shows as chips; the text goes out in
// full when the message is sent.
type pastes struct {
	n    int
	text map[int]string
	last int // the chip the last paste went in as, 0 once opened
}

var pasteRe = convo.PasteChipRe

// isLongPaste is a paste worth folding into a chip: any with more than one
// line, or a very long one.
func isLongPaste(s string) bool {
	return strings.Contains(strings.TrimRight(s, "\n"), "\n") || len(s) > 800
}

func chipFor(id int, text string) string { return convo.PasteChip(id, text) }

// add keeps text and returns the chip that stands for it.
func (p *pastes) add(text string) string {
	if p.text == nil {
		p.text = map[int]string{}
	}
	p.n++
	p.text[p.n] = text
	return chipFor(p.n, text)
}

// paste is a long paste going into buf at pos: its chip, or, as in Claude
// Code, the same text pasted again with its chip right before the cursor
// opens that chip in place to edit. Either way the cursor's offset from
// the end of the text stays as it was.
func (p *pastes) paste(buf []rune, pos int, text string) []rune {
	if id := p.last; id != 0 && p.text[id] == text {
		chip := []rune(chipFor(id, text))
		if from := pos - len(chip); from >= 0 && slices.Equal(buf[from:pos], chip) {
			p.last = 0
			return slices.Concat(buf[:from], []rune(text), buf[pos:])
		}
	}
	chip := p.add(text)
	p.last = p.n
	return insert(buf, pos, []rune(chip))
}

// expand puts the pasted text back in place of each chip. Tagged, each
// goes between <pasted_content> tags as Claude Code sends a paste: Claude
// knows the words were pasted, and the conversation shows it as its chip.
func (p *pastes) expand(s string, tagged bool) string {
	if len(p.text) == 0 {
		return s
	}
	return pasteRe.ReplaceAllStringFunc(s, func(chip string) string {
		id, _ := strconv.Atoi(pasteRe.FindStringSubmatch(chip)[1])
		t, ok := p.text[id]
		if !ok {
			return chip
		}
		if tagged {
			tag := fmt.Sprintf(`id="%04x"`, rand.IntN(0x10000))
			return "\n\n<pasted_content " + tag + ">\n" + strings.TrimRight(t, "\n") + "\n</pasted_content " + tag + ">\n"
		}
		return t
	})
}

// out is a box's text as it goes out: each chip the text it stands for,
// tagged or not as expand does, trimmed. Every send reads a box through it.
func (p *pastes) out(buf []rune, tagged bool) string {
	return strings.TrimSpace(p.expand(string(buf), tagged))
}

// unfold is a sent message back in a box: each tagged paste a chip again.
func (p *pastes) unfold(s string) []rune {
	return []rune(strings.TrimSpace(convo.EachPaste(s, p.add)))
}

// lastIn is the id of the last chip in the draft, 0 if there is none.
func (p *pastes) lastIn(buf []rune) int {
	ms := pasteRe.FindAllStringSubmatch(string(buf), -1)
	for i := len(ms) - 1; i >= 0; i-- {
		if id, _ := strconv.Atoi(ms[i][1]); p.text[id] != "" {
			return id
		}
	}
	return 0
}

// chipAt is the span of the chip with the character at pos in it, and
// whether it's a paste kept here, to open.
func (p *pastes) chipAt(buf []rune, pos int) (seg, bool) {
	s := string(buf)
	if !convo.HasPasteChip(s) {
		return seg{}, false
	}
	for _, loc := range pasteRe.FindAllStringSubmatchIndex(s, -1) {
		from := utf8.RuneCountInString(s[:loc[0]])
		to := from + utf8.RuneCountInString(s[loc[0]:loc[1]])
		if pos >= from && pos < to {
			id, _ := strconv.Atoi(s[loc[2]:loc[3]])
			_, ok := p.text[id]
			return seg{from, to}, ok
		}
	}
	return seg{}, false
}

// open puts the text of the chip at sp back in buf in its place, and
// returns where the cursor goes: after it.
func (p *pastes) open(buf []rune, sp seg) ([]rune, int, bool) {
	if got, ok := p.chipAt(buf, sp.from); !ok || got != sp {
		return buf, 0, false
	}
	id, _ := strconv.Atoi(pasteRe.FindStringSubmatch(string(buf[sp.from:sp.to]))[1])
	t := []rune(p.text[id])
	return slices.Concat(buf[:sp.from], t, buf[sp.to:]), sp.from + len(t), true
}

// chipHover is the paste chip under the pointer: in box 1, the Session's,
// or 2, the Prompt; 0 for none. Space or a click opens it.
type chipHover struct {
	box int
	at  seg
}

// boxAt is which input box has its text at screen x, y (1 the Session's,
// 2 the Prompt, 0 neither), and the position in the text there.
func (m *Model) boxAt(x, y int) (int, int) {
	if c := m.host; c != nil && c.box.w > 0 {
		x0 := 2
		if m.listW > 0 {
			x0 = m.listW + 3
		}
		if y > c.boxY && y <= c.boxY+c.box.rows() && x >= x0 && x < x0+c.box.w {
			return 1, c.box.at(y-c.boxY-1, x-x0)
		}
	}
	b := m.promptBox
	if m.zenFull() || b.w == 0 {
		return 0, 0
	}
	if y > m.promptBoxY && y <= m.promptBoxY+b.rows() && x < b.w {
		return 2, b.at(y-m.promptBoxY-1, x)
	}
	return 0, 0
}

// chipUnder is the paste chip at screen x, y, if there's one to open.
func (m *Model) chipUnder(x, y int) chipHover {
	if !m.boxesTakeKeys() {
		return chipHover{}
	}
	which, pos := m.boxAt(x, y)
	var sp seg
	ok := false
	switch c := m.host; {
	case which == 1 && c != nil:
		sp, ok = c.pastes.chipAt(c.input, pos)
	case which == 2:
		sp, ok = m.pastes.chipAt(m.promptBox.text, pos)
	}
	if !ok {
		return chipHover{}
	}
	return chipHover{which, sp}
}

// boxesTakeKeys is whether a key could go to an input box now: they're on
// screen and nothing over them (a menu, the command bar, a question, a
// dialog, an agent's own screen, the /btw panel) has the keys instead.
func (m *Model) boxesTakeKeys() bool {
	if m.mode != modeList || m.picker != nil || m.bar != nil || m.dialog != nil || m.confirm != nil || m.embedded {
		return false
	}
	if c := m.host; c != nil {
		if t := m.btwFor(c.key); t != nil && t.focused {
			return false
		}
	}
	return true
}

// hoverChip lights the paste chip under the pointer, and reports whether
// that changed what's lit.
func (m *Model) hoverChip(x, y int) bool {
	was := m.chipHot
	m.chipHot = m.chipUnder(x, y)
	return m.chipHot != was
}

// openChip puts a chip's pasted text back in its box, to read and edit in
// place; undo folds it again.
func (m *Model) openChip(h chipHover) bool {
	m.chipHot = chipHover{}
	switch c := m.host; {
	case h.box == 1 && c != nil:
		buf, pos, ok := c.pastes.open(c.input, h.at)
		if !ok {
			return false
		}
		c.undo.save(c.input, c.back, false)
		c.input, c.back, c.anchor = buf, len(buf)-pos, 0
		m.paneFocus = true
	case h.box == 2:
		buf, pos, ok := m.pastes.open(m.input, h.at)
		if !ok {
			return false
		}
		m.paneFocus, m.embedded = false, false
		m.input, m.anchor = buf, 0
		m.setCursor(pos)
	default:
		return false
	}
	return true
}

// editedMsg carries text back from $EDITOR: into paste id, or (id 0) the
// whole draft, of the Session's box or the main one.
type editedMsg struct {
	pane bool
	id   int
	text string
	err  error
}

// editInEditor opens text in $VISUAL or $EDITOR and sends back the result.
// The temp file is written before, and read after, off the UI goroutine.
func editInEditor(text string, pane bool, id int) tea.Cmd {
	var tmp string
	prepare := func() (string, error) {
		f, err := os.CreateTemp("", "rush-*.md")
		if err != nil {
			return "", err
		}
		tmp = f.Name()
		_, _ = f.WriteString(text)
		_ = f.Close()
		return tmp, nil
	}
	return editorCmd(prepare, func(err error) tea.Msg {
		if tmp == "" {
			return editedMsg{err: err}
		}
		return sheetMsg{apply: func(*Model) tea.Cmd {
			return func() tea.Msg {
				defer os.Remove(tmp)
				b, rerr := os.ReadFile(tmp)
				if err == nil {
					err = rerr
				}
				return editedMsg{pane: pane, id: id, text: strings.TrimRight(string(b), "\n"), err: err}
			}
		}}
	})
}

// editDraft is ctrl+g: the last paste in the draft, or the whole draft.
func editDraft(p *pastes, buf []rune, pane bool) tea.Cmd {
	if id := p.lastIn(buf); id != 0 {
		return editInEditor(p.text[id], pane, id)
	}
	return editInEditor(string(buf), pane, 0)
}

// applyEdit puts what came back from the editor into a box.
func applyEdit(p *pastes, buf []rune, id int, text string) []rune {
	if id == 0 {
		return []rune(text)
	}
	old := p.text[id]
	p.text[id] = text
	return []rune(strings.Replace(string(buf), chipFor(id, old), chipFor(id, text), 1))
}
