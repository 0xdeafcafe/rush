package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// An agent asking to write or edit one of its memory notes gets a card of
// its own: the note as it will read (its description, then its text with
// no frontmatter, or what changes in it), its type and scope, and buttons
// to remember it, edit it first, skip it or always allow it.

// cardBtn is a button drawn on a card, where a click presses it: x and y
// from the card's top left, three rows high unless h says.
type cardBtn struct {
	x, y, w, h int
	key        string
}

// memPeek is what a memory card needs from disk, read in the background:
// the note as it is now, whether it's there, and the notes beside it.
type memPeek struct {
	old   string
	had   bool
	notes int // -1 when it isn't in a memory folder
	// What the folder holds: every note's bytes (this one's as it is now),
	// the index's lines and bytes, and the notes there by name, for links.
	bytes      int64
	indexLines int
	indexBytes int64
	names      map[string]bool
}

// indexCap is how many of MEMORY.md's lines a new session reads: Claude
// Code's auto memory loads the index up to here.
const indexCap = 200

// memPeeks are the memory cards' reads out, by approval and path.
var memPeeks = offReads[string, memPeek]{}

// inMemoryDir is whether path is a note in a Claude config dir's
// projects/<project>/memory folder, which is where auto memory lives
// whichever config dir it is.
func inMemoryDir(path string) bool {
	dir := filepath.Dir(path)
	return filepath.Ext(path) == ".md" && filepath.Base(dir) == "memory" &&
		filepath.Base(filepath.Dir(filepath.Dir(dir))) == "projects"
}

// memoryWrite is whether a step waiting on you writes or edits a memory
// note: one in a memory folder, or a file with a note's frontmatter. The
// index, MEMORY.md, keeps the plain card.
func memoryWrite(st *convo.Step) bool {
	_, ok := memNoteOf(st)
	return ok
}

// noteWrite is the write a memory card asks about: a Write or Edit as it
// is, or a shell heredoc into a note read as a Write (appends, for >>).
type noteWrite struct {
	call    tool.Call
	appends bool
	shell   bool
}

// memNoteOf is the memory note st writes, when it writes one.
func memNoteOf(st *convo.Step) (noteWrite, bool) {
	n := noteWrite{call: st.Call()}
	if n.call.Kind == tool.Shell {
		path, text, appends, ok := heredocWrite(n.call.Input.Command)
		if !ok || !inMemoryDir(path) {
			return n, false
		}
		n.call.Kind, n.call.Input = tool.Write, tool.Input{Path: path, Content: text}
		n.appends, n.shell = appends, true
	}
	c := n.call
	if c.Kind != tool.Write && c.Kind != tool.Edit || filepath.Base(c.Input.Path) == "MEMORY.md" {
		return n, false
	}
	if inMemoryDir(c.Input.Path) {
		return n, true
	}
	fm := agent.FrontMatter(strings.NewReader(c.Input.Content))
	return n, fm["name"] != "" && fm["description"] != "" && fm["type"] != ""
}

// heredoc is a command that only writes a heredoc to a file: cat > f
// <<'EOF', cat <<EOF >> f, or tee [-a] f <<EOF, and nothing after.
var heredoc = regexp.MustCompile(`(?s)^\s*(?:cat\s+(>>?)\s*("[^"]+"|'[^']+'|[^\s<>|;&]+)\s*<<-?\s*(['"]?)(\w+)['"]?` +
	`|cat\s*<<-?\s*(['"]?)(\w+)['"]?\s*(>>?)\s*("[^"]+"|'[^']+'|[^\s<>|;&]+)` +
	`|tee\s+(-a\s+)?("[^"]+"|'[^']+'|[^\s<>|;&-][^\s<>|;&]*)\s*<<-?\s*(['"]?)(\w+)['"]?(?:\s*>\s*/dev/null)?)[ \t]*\n`)

// heredocWrite is the file a heredoc-only command writes, what it writes,
// and whether it appends.
func heredocWrite(cmd string) (path, text string, appends, ok bool) {
	m := heredoc.FindStringSubmatchIndex(cmd)
	if m == nil {
		return "", "", false, false
	}
	g := func(i int) string {
		if m[2*i] < 0 {
			return ""
		}
		return cmd[m[2*i]:m[2*i+1]]
	}
	var op, word string
	switch {
	case g(2) != "":
		op, path, word = g(1), g(2), g(4)
	case g(8) != "":
		op, path, word = g(7), g(8), g(6)
	default:
		op, path, word = map[bool]string{true: ">>", false: ">"}[g(9) != ""], g(10), g(12)
	}
	rest := cmd[m[1]:]
	body, after, found := strings.Cut(rest, "\n"+word)
	if strings.HasPrefix(rest, word+"\n") || strings.TrimSpace(rest) == word {
		body, after, found = "", strings.TrimPrefix(rest, word), true
	}
	if !found || strings.TrimSpace(after) != "" {
		return "", "", false, false // more runs after it
	}
	path = strings.Trim(path, `"'`)
	if home, err := os.UserHomeDir(); err == nil {
		for _, h := range []string{"~", "$HOME", "${HOME}"} {
			if rest, ok := strings.CutPrefix(path, h+"/"); ok {
				path = filepath.Join(home, rest)
			}
		}
	}
	if !filepath.IsAbs(path) {
		return "", "", false, false
	}
	return path, body + "\n", op == ">>", true
}

// peekMem is what's on disk for the note at path, once the read's in.
// It's read once for each time it's asked about (key).
func (c *hostConn) peekMem(key, path string) (memPeek, bool) {
	if p, ok := memPeeks.take(key); ok {
		c.peekOf, c.peek = key, p
	}
	if c.peekOf == key {
		return c.peek, true
	}
	shaped := inMemoryDir(path)
	memPeeks.start(key, func() memPeek {
		p := memPeek{notes: -1}
		if b, err := os.ReadFile(path); err == nil {
			p.old, p.had = string(b), true
		}
		if shaped {
			p.notes, p.names = 0, map[string]bool{}
			ms, _ := filepath.Glob(filepath.Join(filepath.Dir(path), "*.md"))
			for _, f := range ms {
				b, _ := os.ReadFile(f)
				if filepath.Base(f) == "MEMORY.md" {
					p.indexBytes, p.indexLines = int64(len(b)), strings.Count(strings.TrimRight(string(b), "\n"), "\n")+1
					continue
				}
				p.notes++
				p.bytes += int64(len(b))
				p.names[strings.TrimSuffix(filepath.Base(f), ".md")] = true
				if n := agent.FrontMatter(strings.NewReader(string(b)))["name"]; n != "" {
					p.names[n] = true
				}
			}
		}
		return p
	})
	if p, ok := memPeeks.take(key); ok { // tests read at once
		c.peekOf, c.peek = key, p
		return p, true
	}
	return memPeek{}, false
}

// splitFront is a note's frontmatter, fences and all, and the text after.
func splitFront(s string) (front, body string) {
	if !strings.HasPrefix(s, "---\n") && !strings.HasPrefix(s, "---\r\n") {
		return "", s
	}
	_, rest, _ := strings.Cut(s, "\n")
	for i := 0; i < len(rest); {
		j := strings.IndexByte(rest[i:], '\n')
		line := rest[i:]
		if j >= 0 {
			line = rest[i : i+j]
		}
		if strings.TrimSpace(line) == "---" {
			if j < 0 {
				return s, ""
			}
			return s[:len(s)-len(rest)+i+j+1], rest[i+j+1:]
		}
		if j < 0 {
			break
		}
		i += j + 1
	}
	return "", s
}

var memLink = regexp.MustCompile(`\[\[[^\]\n]+\]\]`)

// memTypeColour is each type of note in its own colour.
func memTypeColour(t string) string {
	switch t {
	case "user":
		return cBlue
	case "feedback":
		return cYellow
	case "project":
		return cGreen
	case "reference":
		return cQueue
	}
	return cSub
}

// memTexts is the note before and after st: the Write's text, or the
// Edit's on top of the note as it is (just the edit itself until that's
// read), or the note with what's appended.
func memTexts(in *tool.Input, edit, appends bool, peek memPeek, known bool) (before, after string) {
	switch {
	case appends && known && peek.had:
		return peek.old, peek.old + in.Content
	case appends:
		return "", in.Content
	case edit && known && peek.had:
		after = peek.old
		for _, e := range in.Edits {
			n := 1
			if e.All {
				n = -1
			}
			after = strings.Replace(after, e.Old, e.New, n)
		}
		return peek.old, after
	case edit:
		var o, n []string
		for _, e := range in.Edits {
			o, n = append(o, e.Old), append(n, e.New)
		}
		return strings.Join(o, "\n"), strings.Join(n, "\n")
	case known && peek.had:
		return peek.old, in.Content
	}
	return "", in.Content
}

// memoryCard draws the card for a memory note st asks to write, w wide
// and in at most maxH rows, and the buttons on it.
func memoryCard(c *hostConn, st *convo.Step, w, maxH int) ([]string, []cardBtn) {
	note, _ := memNoteOf(st)
	call := note.call
	in := call.Input
	peek, known := c.peekMem(st.Approval.ID+" "+in.Path, in.Path)
	oldText, newText := memTexts(&in, call.Kind == tool.Edit, note.appends, peek, known)
	update := call.Kind == tool.Edit || note.appends || known && peek.had
	fm := agent.FrontMatter(strings.NewReader(newText))
	_, body := splitFront(newText)
	_, oldBody := splitFront(oldText)

	edge := cYellow
	if c.cardFocus {
		edge = cOrange
	}
	e := func(s string) string { return paint(edge, s) }
	var out []string
	row := func(s string) { out = append(out, onBg(qCard, e("│")+fit("  "+s, w-2)+e("│"), w)) }
	tw := max(20, w-6)

	// The top edge: what it asks, and the note's type and scope.
	title := "Remember this?"
	if update {
		title = "Update this memory?"
	}
	left := e("╭─ ") + paint(memTypeColour(fm["type"]), "◆") + " " + paint(cText+bold, title) + " "
	var meta []string
	if t := fm["type"]; t != "" {
		meta = append(meta, paint(memTypeColour(t), t))
	}
	if inMemoryDir(in.Path) {
		meta = append(meta, paint(cSub, "this project"))
	}
	right := ""
	if len(meta) > 0 {
		right = " " + strings.Join(meta, dim(" · ")) + " "
	}
	fill := w - cellw.String(left) - cellw.String(right) - 2
	out = append(out, onBg(qCard, left+e(strings.Repeat("─", max(1, fill)))+right+e("─╮"), w))
	row("")

	// What it says: its description in bold, then its text, or what
	// changes in it.
	about := firstNonEmpty(fm["description"], fm["name"], strings.TrimSuffix(filepath.Base(in.Path), ".md"))
	for _, l := range wrap(oneLine(about), tw) {
		row(paint(cText+bold, l))
	}
	row("")
	text := memDiff(oldBody, body, tw)
	if !update {
		text = memText(body, tw)
	}
	// It keeps to maxH: the text gives way, the rest (15 rows) doesn't.
	if room := max(3, maxH-15); maxH > 0 && len(text) > room {
		text = append(text[:room-1:room-1], dim(fmt.Sprintf("… %d more lines", len(text)-room+1)))
	}
	for _, l := range text {
		row(l)
	}
	row("")

	// The foot: how much memory there is, what this adds to every session,
	// where it goes, the notes it links to, and whether it's new.
	if inMemoryDir(in.Path) && known && peek.notes >= 0 {
		for _, l := range memImpact(peek, update, in.Path, fm["description"], newText, tw) {
			row(l)
		}
		row("")
	}
	var ls []string
	for _, l := range memLink.FindAllString(body, 6) {
		if peek.names == nil || peek.names[strings.Trim(l, "[]")] {
			ls = append(ls, paint(cBlue, l))
		} else {
			ls = append(ls, faint(l+" (none yet)"))
		}
	}
	links := ""
	if len(ls) > 0 {
		links = paint(cSub, "links") + "  " + strings.Join(ls, " ")
	}
	status := "new"
	if update {
		status = "update"
	}
	row(spread(links, dim(status)+"  ", w-4))
	row("")

	rows, btns := memButtons(memBtnsFor(st), c.cardFocus, c.btnPick, c.subHover, len(out))
	for _, r := range rows {
		row(r)
	}

	// The bottom edge says how to get to it, or back from it.
	hint := " ↑ to answer "
	if c.cardFocus {
		hint = " ←→ pick · enter · esc back to typing "
	}
	out = append(out, onBg(qCard, e("╰"+strings.Repeat("─", max(1, w-3-cellw.String(hint))))+dim(hint)+e("─╯"), w))
	return out, btns
}

// memImpact is what saving a note means, in a few rows: how much memory
// the project has, what every new session reads more of, and where it's
// kept. Tokens are estimated, a token to four bytes.
func memImpact(p memPeek, update bool, path, about, text string, w int) []string {
	tok := func(n int64) string { return "~" + convo.Tokens(max(1, int(n/4))) + " tokens" }
	label := func(s string) string { return paint(cSub, fmt.Sprintf("%-8s", s)) + "  " }
	notes := p.notes
	if !update && !p.had {
		notes++
	}
	index := fmt.Sprintf("index %d of %d lines", p.indexLines, indexCap)
	if p.indexLines >= indexCap {
		index = paint(cYellow, fmt.Sprintf("index %d lines: past %d, new sessions miss the rest", p.indexLines, indexCap))
	}
	have := fmt.Sprintf("%d note%s · %s", notes, plural(notes), tok(p.bytes+int64(len(text))-int64(len(p.old))))
	out := []string{label("memory") + dim(have+" · ") + dim(index)}
	var impact string
	if update {
		d := int64(len(text)) - int64(len(p.old))
		change := "the same size"
		if d != 0 {
			change = fmt.Sprintf("%+d bytes", d)
		}
		impact = "a session reads it when it's relevant: " + change + " (" + tok(int64(len(text))) + " in all); the index stays as it is"
	} else {
		// The index gets a line naming it: its description and a link.
		impact = "every new session here starts with the index, a line (" + tok(int64(len(about))+40) + ") longer; the note (" +
			tok(int64(len(text))) + ") is read only when it's relevant"
	}
	for i, l := range wrap(impact, w-10) {
		if i == 0 {
			out = append(out, label("impact")+dim(l))
		} else {
			out = append(out, strings.Repeat(" ", 10)+dim(l))
		}
	}
	return append(out, label("file")+faint(shortPath(tildify(path), w-10)))
}

// memText is a note's text as it reads, wrapped to w.
func memText(body string, w int) []string {
	var out []string
	for l := range strings.SplitSeq(strings.Trim(body, "\n"), "\n") {
		for _, wl := range wrap(convo.Inline(strings.ReplaceAll(l, "\t", "  "), cText), w) {
			out = append(out, paint(cText, wl))
		}
	}
	return out
}

// memBtns are the memory card's buttons: the key each presses, what
// shows for it, and its name.
var memBtns = [][3]string{{"enter", "↵", "Remember"}, {"e", "e", "Edit it first"}, {"n", "n", "Skip"}, {"a", "a", "Always ok"}}

// memBtnsFor are the buttons st's card has: one written by a shell
// command has no always: that would allow the command, cat >> and all,
// not the note.
func memBtnsFor(st *convo.Step) [][3]string {
	if n, _ := memNoteOf(st); n.shell {
		return memBtns[:3]
	}
	return memBtns
}

// memButtons draws the memory card's buttons, bs.
func memButtons(bs [][3]string, focus bool, pick int, hover string, y int) ([3]string, []cardBtn) {
	return cardButtons(bs, focus, pick, hover, y)
}

// btnTone is a button's colour, by what its key does: yes green, no red,
// for good blue, don't-ask yellow, edit first purple.
func btnTone(key string) string {
	switch key {
	case "y", "enter":
		return cGreen
	case "n":
		return cRed
	case "a":
		return cBlue
	case "o":
		return cYellow
	case "e":
		return cQueue
	}
	return cOrange
}

// cardButtons draws a card's buttons, three rows starting y rows into the
// card, and where each is: each outlined in the colour of what it does,
// its key bold in it. The picked one is enter's: while the card has the
// keys it's filled solid, as is one under the pointer.
func cardButtons(bs [][3]string, focus bool, pick int, hover string, y int) ([3]string, []cardBtn) {
	var rows [3]strings.Builder
	btns := make([]cardBtn, 0, len(bs))
	x := 3 // past the edge and its two spaces
	for i, b := range bs {
		iw := cellw.String(" " + b[1] + " " + b[2] + "  ")
		tone := btnTone(b[0])
		if i > 0 {
			for j := range rows {
				rows[j].WriteString("  ")
			}
			x += 2
		}
		if focus && i == pick || hover == "btn:"+b[0] {
			// Solid: half blocks above and below, the middle filled.
			rows[0].WriteString(paint(tone, strings.Repeat("▄", iw+2)))
			rows[1].WriteString(paint(btnFill[tone], fit("  "+b[1]+" "+b[2], iw+2)))
			rows[2].WriteString(paint(tone, strings.Repeat("▀", iw+2)))
		} else {
			label := paint(tone+bold, b[1]) + " " + paint(cText, b[2])
			if i == pick {
				label = paint(tone+bold, b[1]) + " " + paint(cBright+bold, b[2])
			}
			rows[0].WriteString(paint(tone, "╭"+strings.Repeat("─", iw)+"╮"))
			rows[1].WriteString(paint(tone, "│") + " " + label + "  " + paint(tone, "│"))
			rows[2].WriteString(paint(tone, "╰"+strings.Repeat("─", iw)+"╯"))
		}
		btns = append(btns, cardBtn{x: x, y: y, w: iw + 2, key: b[0]})
		x += iw + 2
	}
	return [3]string{rows[0].String(), rows[1].String(), rows[2].String()}, btns
}

// memDiff is what changes between a note's old text and its new: the
// lines between what they share at the start and at the end, the old red
// and the new green, with a line of what's around them.
func memDiff(old, cur string, w int) []string {
	a := strings.Split(strings.Trim(old, "\n"), "\n")
	b := strings.Split(strings.Trim(cur, "\n"), "\n")
	pre := 0
	for pre < len(a) && pre < len(b) && a[pre] == b[pre] {
		pre++
	}
	suf := 0
	for suf < len(a)-pre && suf < len(b)-pre && a[len(a)-1-suf] == b[len(b)-1-suf] {
		suf++
	}
	var out []string
	line := func(sign, col, l string) {
		for i, wl := range wrap(convo.Inline(strings.ReplaceAll(l, "\t", "  "), cText), w-2) {
			if i > 0 {
				sign = " "
			}
			out = append(out, paint(col, sign+" ")+paint(cText, wl))
		}
	}
	if pre > 0 {
		out = append(out, "  "+ansi.Truncate(convo.Inline(a[pre-1], cDim), w-2, "…"))
	}
	for _, l := range a[pre : len(a)-suf] {
		line("−", cRed, l)
	}
	for _, l := range b[pre : len(b)-suf] {
		line("+", cGreen, l)
	}
	if suf > 0 {
		out = append(out, "  "+ansi.Truncate(convo.Inline(a[len(a)-suf], cDim), w-2, "…"))
	}
	return out
}

// clickCard presses the card's button under a click.
func (m *Model) clickCard(c *hostConn, x, y int) (tea.Cmd, bool) {
	if key := m.cardBtnAt(c, x, y); key != "" {
		c.cardFocus = true
		return m.cardKey(c, key, true)
	}
	return m.clickQuestion(c, x, y)
}

// cardBtnAt is the key of the card's button at x, y on screen; "" off them.
func (m *Model) cardBtnAt(c *hostConn, x, y int) string {
	if c.panelClip[1] > 0 && (y < c.panelClip[0] || y >= c.panelClip[1]) {
		return ""
	}
	for _, b := range c.btns {
		bx, by := m.paneX()+b.x, c.dockY+c.cardTop+b.y
		h := b.h
		if h == 0 {
			h = 3
		}
		if x >= bx && x < bx+b.w && y >= by && y < by+h {
			return b.key
		}
	}
	return ""
}

// openWritten opens the note "e" allowed in the memory view's editor, once
// the write is done.
func (m *Model) openWritten(c *hostConn) {
	st := c.sess.Step(c.editAfter)
	if st == nil || st.Status == convo.Running || st.Status == convo.Waiting {
		return
	}
	c.editAfter = ""
	if st.Status != convo.OK || !m.showView(c, "memory") {
		return
	}
	n, _ := memNoteOf(st)
	path := n.call.Input.Path
	c.sel, c.mem, c.memEd, c.memEdit = "mem:"+path, nil, openDoc(path), true
}
