package ui

import (
	"fmt"
	"os"
	"path/filepath"
	"cmp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/photon/fuzzy"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// The command bar (ctrl+k, or cmd+k where the terminal passes it on) goes
// anywhere from anywhere: a place or a page, an agent, a Session's view, a
// turn of the conversation open, or a match in any agent's transcript.
// What's typed is matched loosely against names; words also search the
// open conversation and, after a pause, every transcript in the background.
//
// ctrl+f is the same bar found where you are: in a Session it finds in that
// chat, in the list in the selected agent's group, in Machine or Settings
// among that place's pages. Each ctrl+f widens it (the chat, its group, all
// agents, everywhere, then round again), as does backspace with nothing
// typed; ctrl+k goes straight to everywhere.

// barScope is how far the bar looks.
type barScope struct {
	kind  string // "chat", "group", "agents", "place"; "" is everywhere
	key   string // the chat's agent
	group string // the list section
	place int    // the place, for "place"
	label string
}

// transcripts reports whether the scope searches agents' transcripts.
func (sc barScope) transcripts() bool {
	return sc.kind == "" || sc.kind == "group" || sc.kind == "agents"
}

// barScopes are the scopes ctrl+f steps through from here, narrowest first;
// the last is always everywhere.
func (m *Model) barScopes() []barScope {
	var out []barScope
	if m.view != placeAgents {
		out = append(out, barScope{kind: "place", place: m.view, label: viewNames[m.view%len(viewNames)]})
	} else {
		if c := m.host; c != nil && (m.paneFocus || m.zen) {
			out = append(out, barScope{kind: "chat", key: c.key, label: "this chat"})
		}
		group, ok := strings.CutPrefix(m.sel, "§")
		if !ok {
			group = m.groupOf[m.sel]
		}
		if group != "" {
			out = append(out, barScope{kind: "group", group: group, label: group})
		}
		out = append(out, barScope{kind: "agents", label: "all agents"})
	}
	return append(out, barScope{label: "everywhere"})
}

type barItem struct {
	section string
	glyph   string // painted
	title   string // plain; lit where the query matched
	lit     []int  // rune indexes of title to light
	meta    string // plain, dim, in a column after the title
	tail    string // plain, dim, short, at the right edge: state, age, turn
	score   int
	run     func(m *Model) tea.Cmd
}

type cmdBar struct {
	scopes []barScope
	at     int // the scope in use
	query  []rune
	pos    int
	cursor int
	top    int // first row of the results on screen
	items  []barItem

	// Transcripts, searched in the background for the query's words.
	gen      int
	search   *barSearch
	found    []barFound
	searched int
	toSearch int
	done     bool
	glint    int // where the frame's glint is while searching
}

// spot is somewhere the bar jumped from, for "Back".
type spot struct {
	view, settingsPage int
	projSel                     string
	agentsPage                  int
	mode                        mode // which of Agents' pages
	zen                         bool
	key, name                   string
	settingsName                string // the Settings page's
	paneView                    int
	ref                         string
}

// barJump is a jump into an agent's conversation waiting for it to open:
// the hit is found again in the session as the pane reads it, by its words
// and snippet, since a rush session's turns count from its host's replay.
type barJump struct {
	key, name   string
	ref         string
	query, snip string
	at          time.Time
}

func (b *cmdBar) scope() barScope { return b.scopes[b.at] }

// barToggle opens the bar (ctrl+k everywhere, ctrl+f where you are) and,
// while it's open, changes its scope: ctrl+f widens it, ctrl+k goes to
// everywhere and, there already, closes it. ctrl+k is also "cut to the end
// of the line", which it stays while there's something after the cursor to
// cut; on a Claude Code agent's own screen both keys are Claude Code's.
func (m *Model) barToggle(s string) (tea.Cmd, bool) {
	if s != "super+k" && s != "ctrl+k" && s != "ctrl+f" {
		return nil, false
	}
	if b := m.bar; b != nil {
		last := len(b.scopes) - 1
		switch {
		case s == "ctrl+f":
			b.at = (b.at + 1) % len(b.scopes)
		case b.at != last:
			b.at = last
		default:
			m.closeBar()
			return nil, true
		}
		return m.barChanged(), true
	}
	if m.confirm != nil || m.dialog != nil && m.dialog.asking != "" || m.embedded && s != "super+k" {
		return nil, false
	}
	if s == "ctrl+k" && m.canCut() {
		return nil, false
	}
	m.everyAgent()
	m.openBar(s == "ctrl+f")
	return nil, true
}

// openBar opens the bar where you are (here) or everywhere.
func (m *Model) openBar(here bool) {
	m.bar = &cmdBar{scopes: m.barScopes()}
	if !here {
		m.bar.at = len(m.bar.scopes) - 1
	}
	m.embedded = false
	m.barRebuild()
}

// canCut reports whether ctrl+k would cut text in the box that has the keys.
func (m *Model) canCut() bool {
	if c := m.host; m.paneFocus && c != nil && m.mode == modeList && m.dialog == nil {
		pos := len(c.input) - c.back
		return lineEnd(c.input, pos) > pos
	}
	if m.acceptsText() {
		pos := len(m.input) - m.back
		return lineEnd(m.input, pos) > pos
	}
	return false
}

func (m *Model) closeBar() {
	if m.bar != nil && m.bar.search != nil {
		m.bar.search.cancel()
	}
	m.bar = nil
}

func (m *Model) barKey(k tea.KeyPressMsg, s string) tea.Cmd {
	b := m.bar
	switch s {
	case "esc", "ctrl+c":
		m.closeBar()
		return nil
	case "up", "ctrl+p":
		b.cursor = roundMove(b.cursor, -1, len(b.items))
		return nil
	case "down", "ctrl+n":
		b.cursor = roundMove(b.cursor, 1, len(b.items))
		return nil
	case "pgup":
		b.cursor = max(0, b.cursor-10)
		return nil
	case "pgdown":
		b.cursor = max(0, min(len(b.items)-1, b.cursor+10))
		return nil
	case "tab", "shift+tab":
		b.cursor = b.nextSection(map[string]int{"tab": 1, "shift+tab": -1}[s])
		return nil
	case "ctrl+s":
		// The agents' order: by what they need of you, the latest, or in
		// sections by project, worktree, model or harness.
		m.barSort = barSorts[(slices.Index(barSorts, m.barSort)+1)%len(barSorts)]
		b.cursor = 0
		return m.barChanged()
	case "ctrl+enter", "ctrl+j":
		// ctrl+j is the same key where the terminal can't tell ctrl+enter
		// from enter.
		return m.searchNow()
	case "enter":
		if b.cursor >= len(b.items) {
			return nil
		}
		it := b.items[b.cursor]
		from := m.here()
		m.closeBar()
		cmd := it.run(m)
		if it.section != "" && from != nil && it.title != "Back" {
			m.barBack = from
		}
		return tea.Batch(cmd, m.loadPreview())
	}
	// Backspace with nothing typed widens the scope, as ctrl+f does.
	if (s == "backspace" || s == "ctrl+h") && len(b.query) == 0 && b.at < len(b.scopes)-1 {
		b.at++
		return m.barChanged()
	}
	buf, pos, ok := edit(b.query, b.pos, k, s)
	if !ok || strings.Contains(string(buf), "\n") || string(buf) == string(b.query) {
		if ok {
			b.pos = pos
		}
		return nil
	}
	b.query, b.pos = buf, pos
	return m.barChanged()
}

// nextSection is the first row of the next (or previous) section, round.
func (b *cmdBar) nextSection(d int) int {
	if len(b.items) == 0 {
		return 0
	}
	var starts []int
	for i, it := range b.items {
		if i == 0 || it.section != b.items[i-1].section {
			starts = append(starts, i)
		}
	}
	cur := 0
	for j, st := range starts {
		if st <= b.cursor {
			cur = j
		}
	}
	return starts[(cur+d+len(starts))%len(starts)]
}

// barChanged rebuilds the rows for a new query and, once typing pauses,
// searches the transcripts for its words.
func (m *Model) barChanged() tea.Cmd {
	b := m.bar
	b.cursor, b.top = 0, 0
	if b.search != nil {
		b.search.cancel()
		b.search = nil
	}
	b.gen++
	b.found, b.searched, b.toSearch, b.done = nil, 0, 0, false
	m.barRebuild()
	if !searchable(string(b.query)) || !b.scope().transcripts() || m.store.Config.SearchTranscriptsOnKey {
		return nil
	}
	gen := b.gen
	return tea.Tick(200*time.Millisecond, func(time.Time) tea.Msg { return barPauseMsg(gen) })
}

// searchNow searches the transcripts for what's typed, at once: how they
// are searched at all when SearchTranscriptsOnKey keeps typing to names.
func (m *Model) searchNow() tea.Cmd {
	b := m.bar
	if b.search != nil || !b.scope().transcripts() {
		return nil
	}
	if !searchable(string(b.query)) {
		m.flash("type at least 3 letters to search the transcripts", false)
		return nil
	}
	return tea.Batch(m.searchTranscripts(), glintTick(b.gen))
}

// searchKey is the key that searches the transcripts on demand, as this
// terminal can send it: ctrl+j where ctrl+enter arrives as enter.
func (m *Model) searchKey() string {
	if m.keysDisambiguated {
		return "ctrl+enter"
	}
	return "ctrl+j"
}

// searchable is a query with enough to it to be worth reading every
// transcript for.
func searchable(q string) bool {
	n := 0
	for _, w := range convo.Words(stripIn(q)) {
		n += len(w)
	}
	return n >= 3
}

type barPauseMsg int

type barFoundMsg struct {
	gen   int
	found []barFound
	done  bool
}

// barFound is one agent's matches.
type barFound struct {
	key, name string
	rank      int // the agent's place in the search order: most recent first
	hits      []convo.Hit
	total     int
}

// barMsg handles the bar's own messages; ok is false for anything else.
func (m *Model) barMsg(msg tea.Msg) (tea.Cmd, bool) {
	switch msg := msg.(type) {
	case tea.MouseClickMsg:
		if m.bar == nil {
			return nil, false
		}
		if cmd, ok := m.overlayClick(msg.X, msg.Y); ok && msg.Button == tea.MouseLeft {
			return cmd, true // a row picked, or outside it: put away
		}
		m.closeBar()
		return nil, true
	case tea.MouseWheelMsg:
		if b := m.bar; b != nil && len(b.items) > 0 {
			d := map[tea.MouseButton]int{tea.MouseWheelUp: -3, tea.MouseWheelDown: 3}[msg.Button]
			b.cursor = max(0, min(len(b.items)-1, b.cursor+d))
		}
		return nil, m.bar != nil
	case tea.MouseMotionMsg, tea.MouseReleaseMsg:
		return nil, m.bar != nil
	case barPauseMsg:
		if b := m.bar; b != nil && int(msg) == b.gen && b.search == nil {
			return tea.Batch(m.searchTranscripts(), glintTick(b.gen)), true
		}
		return nil, true
	case barGlintMsg:
		if b := m.bar; b != nil && int(msg) == b.gen && b.search != nil && !b.done {
			b.glint += 3
			return glintTick(b.gen), true
		}
		return nil, true
	case barFoundMsg:
		b := m.bar
		if b == nil || msg.gen != b.gen || b.search == nil {
			return nil, true
		}
		for _, f := range msg.found {
			b.searched++
			if f.total > 0 {
				b.found = append(b.found, f)
			}
		}
		sort.Slice(b.found, func(i, j int) bool { return b.found[i].rank < b.found[j].rank })
		b.done = msg.done
		// Keep the cursor on the row it was on as rows arrive above it.
		var keep *barItem
		if b.cursor < len(b.items) {
			keep = &b.items[b.cursor]
		}
		title, section := "", ""
		if keep != nil {
			title, section = keep.title+keep.meta, keep.section
		}
		m.barRebuild()
		for i, it := range b.items {
			if it.section == section && it.title+it.meta == title {
				b.cursor = i
				break
			}
		}
		if msg.done {
			return nil, true
		}
		return b.search.wait(), true
	}
	return nil, false
}

// barSearch reads transcripts on a few goroutines and hands back what they
// find in batches, so a thousand files cost a handful of redraws.
type barSearch struct {
	gen  int
	ch   chan barFound
	stop chan struct{}
	once sync.Once
}

func (s *barSearch) cancel() { s.once.Do(func() { close(s.stop) }) }

// wait reads the next batch of matches, or the search's end, as a
// barFoundMsg for the command bar.
func (s *barSearch) wait() tea.Cmd {
	return s.waitAs(func(found []barFound, done bool) tea.Msg {
		return barFoundMsg{gen: s.gen, found: found, done: done}
	})
}

// waitAs is wait, but for a caller other than the command bar: mk wraps the
// batch (or the search's end) in that caller's own message type, so its
// results don't cross with the bar's.
func (s *barSearch) waitAs(mk func(found []barFound, done bool) tea.Msg) tea.Cmd {
	return func() tea.Msg {
		f, ok := <-s.ch
		if !ok {
			return mk(nil, true)
		}
		batch := []barFound{f}
		soon := time.After(60 * time.Millisecond)
		for {
			select {
			case f, ok := <-s.ch:
				if !ok {
					return mk(batch, true)
				}
				batch = append(batch, f)
			case <-soon:
				return mk(batch, false)
			}
		}
	}
}

// searchAgentTranscripts reads agents' transcripts for q on a few
// goroutines, in the order given, handing back what they find in batches
// through the barSearch returned, so a thousand files cost a handful of
// redraws.
func searchAgentTranscripts(gen int, agents []*fleet.Agent, q string) *barSearch {
	type job struct {
		rank      int
		key, name string
		path      string
	}
	jobs := make([]job, len(agents))
	for i, a := range agents {
		jobs[i] = job{i, a.Key, oneLine(a.DisplayName), a.TranscriptPath}
	}
	s := &barSearch{gen: gen, ch: make(chan barFound, 16), stop: make(chan struct{})}
	if len(jobs) == 0 {
		close(s.ch)
		return s
	}
	feed := make(chan job)
	go func() {
		var wg sync.WaitGroup
		for range min(4, len(jobs)) {
			wg.Add(1)
			go func() {
				defer wg.Done()
				for j := range feed {
					hits, total, _ := convo.SearchFile(j.path, q, 3)
					select {
					case s.ch <- barFound{key: j.key, name: j.name, rank: j.rank, hits: hits, total: total}:
					case <-s.stop:
						return
					}
				}
			}()
		}
	send:
		for _, j := range jobs {
			select {
			case feed <- j:
			case <-s.stop:
				break send
			}
		}
		close(feed)
		wg.Wait()
		close(s.ch)
	}()
	return s
}

// searchTranscripts starts reading every agent's transcript for the query,
// the most recently active first, leaving out the one open in the pane (its
// matches are already under This session). in:<name> narrows the agents.
func (m *Model) searchTranscripts() tea.Cmd {
	b := m.bar
	q, in := splitIn(string(b.query))
	sc := b.scope()
	var agents []*fleet.Agent
	for _, a := range m.snap.Agents {
		// Everywhere, the open chat's matches are under This session.
		if a.TranscriptPath == "" || sc.kind == "" && m.host != nil && a.Key == m.host.key {
			continue
		}
		if sc.kind == "group" && m.groupOf[a.Key] != sc.group {
			continue
		}
		if in != "" && !strings.Contains(strings.ToLower(a.DisplayName), in) {
			continue
		}
		agents = append(agents, a)
	}
	now := m.snap.At
	sort.SliceStable(agents, func(i, j int) bool { return agents[i].Age(now) < agents[j].Age(now) })
	b.toSearch = len(agents)
	if len(agents) == 0 {
		b.done = true
		return nil
	}
	b.search = searchAgentTranscripts(b.gen, agents, q)
	return b.search.wait()
}

// splitIn takes in:<name> out of a query.
func splitIn(q string) (rest, in string) {
	var keep []string
	for _, f := range strings.Fields(q) {
		if v, ok := strings.CutPrefix(strings.ToLower(f), "in:"); ok {
			in = v
			continue
		}
		keep = append(keep, f)
	}
	return strings.Join(keep, " "), in
}

func stripIn(q string) string { r, _ := splitIn(q); return r }

// barRebuild lists what the query matches, section by section.
func (m *Model) barRebuild() {
	b := m.bar
	raw := strings.TrimSpace(string(b.query))
	q, in := splitIn(raw)
	words := convo.Words(q)
	// Names are matched by the words alone: is:, file: and turn: only
	// narrow conversations.
	name := strings.Join(words, " ")
	filtered := convo.Filtered(q) || in != ""
	var out []barItem
	add := func(section string, items []barItem, limit int) {
		if name != "" {
			sort.SliceStable(items, func(i, j int) bool { return items[i].score > items[j].score })
		}
		for i := range items {
			items[i].section = section
		}
		out = append(out, items[:min(limit, len(items))]...)
	}
	switch sc := b.scope(); sc.kind {
	case "chat":
		if c := m.host; c != nil && c.key == sc.key {
			add("This chat · "+oneLine(m.hostName(c)), m.barTurns(c, q), 300)
		}
	case "group", "agents":
		if !filtered {
			title := "Agents"
			if sc.kind == "group" {
				title = sc.group
			}
			add(title, m.barAgents(name, sc.group), 40)
		}
		add("Transcripts", m.barTranscripts(words), 60)
	case "place":
		var pages []barItem
		for _, it := range m.barPlaces(name) {
			if strings.HasPrefix(it.title, sc.label+" › ") {
				pages = append(pages, it)
			}
		}
		add(sc.label, pages, 20)
	default:
		_, turnErr := strconv.Atoi(strings.TrimPrefix(raw, "#"))
		switch {
		case filtered:
		case strings.HasPrefix(raw, "#") && turnErr != nil:
			// # is for commands: none of them clutter the bar until asked for.
			title := "Commands"
			if a := m.focused(); a != nil {
				title += " · on " + oneLine(a.DisplayName)
			}
			add(title, m.barCommands(name), 40)
		default:
			// Agents first, by what they need of you; places once typed for,
			// but for starting one and going back.
			places := m.barPlaces(name)
			if name == "" || m.barSort != "status" && m.barSort != "" {
				groups := m.barAgentGroups(name)
				for _, g := range groups {
					add(g.title, g.items, max(3, 12/max(1, len(groups))))
				}
				places = slices.DeleteFunc(places, func(it barItem) bool { return it.title != "Back" && it.title != "Start an agent" })
				add("Go to", places, 2)
				break
			}
			// Typed: whichever matches best comes first.
			agents, goTo := m.barAgents(name, ""), places
			best := func(its []barItem) int {
				top := -1 << 30
				for _, it := range its {
					top = max(top, it.score)
				}
				return top
			}
			if best(goTo) > best(agents) {
				add("Go to", goTo, 8)
				add("Agents", agents, 10)
			} else {
				add("Agents", agents, 10)
				add("Go to", goTo, 8)
			}
		}
		if c := m.host; c != nil && in == "" && (!strings.HasPrefix(raw, "#") || turnErr == nil) {
			add("This session · "+oneLine(m.hostName(c)), m.barTurns(c, q), map[bool]int{true: 4, false: 50}[raw == ""])
		}
		// Anything else typed can be a new agent's task (#12 is a turn).
		if raw != "" && !filtered && !strings.HasPrefix(raw, "#") {
			add("Start", []barItem{m.barStart(raw)}, 1)
		}
		add("Transcripts", m.barTranscripts(words), 60)
	}
	b.items = out
	b.cursor = min(b.cursor, max(0, len(out)-1))
}

func (m *Model) hostName(c *hostConn) string {
	if a := m.agentByKey(c.key); a != nil {
		return a.DisplayName
	}
	return "this agent"
}

// matchItem scores an item against the words typed; one that doesn't match
// is left out. Matches in the title are lit; keywords only count.
func matchItem(it barItem, q, keywords string) (barItem, bool) {
	if q == "" {
		return it, true
	}
	if s, at, ok := fuzzy.Match(q, it.title); ok {
		it.score, it.lit = s, at
		return it, true
	}
	if s, _, ok := fuzzy.Match(q, it.title+" "+keywords); ok {
		it.score = s - 20
		return it, true
	}
	return it, false
}

// barPlaces are the places, their pages, the open Session's views and a
// few things to do.
func (m *Model) barPlaces(q string) []barItem {
	var items []barItem
	add := func(glyph, title, meta, keywords string, run func(m *Model) tea.Cmd) {
		if it, ok := matchItem(barItem{glyph: glyph, title: title, meta: meta, run: run}, q, keywords); ok {
			items = append(items, it)
		}
	}
	if bk := m.barBack; bk != nil {
		where := bk.where()
		add(paint(cOrange, "↩"), "Back", where, "previous return "+where, func(m *Model) tea.Cmd { return m.goSpot(bk) })
	}
	add(paint(cGreen, "+"), "Start an agent", "in "+tildify(m.startDir())+" · "+m.startWith(m.startDir(), true), "new session spawn prompt task model effort profile", func(m *Model) tea.Cmd {
		m.toPrompt()
		return nil
	})
	add(paint(cSub, "◇"), "Agents", "the list", "home list", func(m *Model) tea.Cmd {
		m.goView(placeAgents)
		m.setZen(false)
		return nil
	})
	zen := "Zen"
	if m.zen {
		zen = "Leave Zen"
	}
	add(paint(cSub, "◇"), zen, "only the agent that needs you", "focus zen", func(m *Model) tea.Cmd {
		m.setZen(!m.zen)
		return nil
	})
	add(paint(cSub, "◇"), "Agents › Wall", "every agent at once, streaming", "agents wall grid tiles all agents live stream dashboard", func(m *Model) tea.Cmd {
		m.goView(placeAgents)
		m.setAgentsPage(agentsWall)
		return m.refreshFolders()
	})
	for _, p := range []struct{ name, id, what string }{
		{"Projects", "", "each repository whole: branch, changes, worktrees and their sizes, agents' scratch, commits, PRs and its agents"},
		{"Projects › Temporary", paneTemp, "/tmp and finished agents' temp work"},
		{"Projects › System", paneSystem, "processes no project owns"},
	} {
		add(paint(cSub, "◇"), p.name, p.what, "projects repositories repos folders git branches worktrees commits prs cleanup clean disk temporary scratch processes orphans machine system "+p.name, func(m *Model) tea.Cmd {
			m.goView(placeProjects)
			m.pickInProjects(p.id)
			return m.projectsOpen()
		})
	}
	for i, p := range m.settingsPages() {
		place := "Settings › "
		if i < pageGeneral {
			place = "Harnesses › "
		}
		add(paint(cSub, "◇"), place+p.name, "", "settings preferences harnesses agents "+p.name, func(m *Model) tea.Cmd {
			m.setSettingsPage(i)
			return nil
		})
	}
	if c := m.host; c != nil {
		for i, v := range m.views(c) {
			title := "Session › " + strings.ToUpper(v[:1]) + v[1:]
			add(paint(cBlue, "▤"), title, oneLine(m.hostName(c)), "session view "+v, func(m *Model) tea.Cmd {
				m.goView(placeAgents)
				if c := m.host; c != nil {
					c.view = i
				}
				m.preview, m.paneFocus = true, true
				return nil
			})
		}
		add(paint(cBlue, "⌕"), "Find in this chat", "ctrl+f", "search find session", func(m *Model) tea.Cmd {
			m.goView(placeAgents)
			m.preview, m.paneFocus = true, true
			m.openBar(true)
			return nil
		})
	}
	if a := m.focused(); a != nil && !a.Rush && !a.Interactive && a.PID != 0 {
		add(paint(cSub, "◇"), "Open full screen", oneLine(a.DisplayName)+" in Claude Code", "attach claude code full screen native", func(m *Model) tea.Cmd {
			return m.attach(a)
		})
	}
	w, _ := m.stash()
	add(paint(cSub, "◇"), w.Title, strings.ToLower(w.Tab)+", sent, cleared and replaced · "+w.Command(), "stash drafts sent cleared replaced history messages typed before", func(m *Model) tea.Cmd {
		return m.stashCommand("history")
	})
	add(paint(cSub, "◇"), "Folder for new sessions", tildify(m.startDir()), "start dir cwd", func(m *Model) tea.Cmd {
		m.goView(placeAgents)
		return m.openDirPicker()
	})
	add(paint(cSub, "◇"), "All keys", "?", "help shortcuts keys", func(m *Model) tea.Cmd {
		m.goView(placeAgents)
		m.mode = modeHelp
		return nil
	})
	cur := m.startDir()
	for i, d := range m.startDirs() {
		if d == cur {
			continue // "Start an agent" above already starts here
		}
		i, base := i, filepath.Base(d)
		add(paint(cGreen, "+"), "Start an agent › "+tildify(d), "", "start new agent session spawn "+base, func(m *Model) tea.Cmd {
			m.pickStartDir(i)
			m.toPrompt()
			return nil
		})
	}
	return items
}

// barCommands are rush's # commands and plugins' commands, typeable and
// runnable from the bar: picking one drops it (with its argument's space,
// if it needs one) into the Prompt, the way typing it there would, and
// runs it at once when it doesn't. Those that act on an agent are offered
// only once one is focused; the rest are rush-wide.
func (m *Model) barCommands(q string) []barItem {
	var items []barItem
	a := m.focused()
	for _, cmd := range append(m.availableFleetCommands(), m.pluginHashCommands()...) {
		if fleetNeedsAgent[cmd.Name] && a == nil {
			continue
		}
		if it, ok := matchItem(barItem{glyph: paint(cSub, "›"), title: "#" + cmd.Name, meta: convo.KeyWord(cmd.Description), run: func(m *Model) tea.Cmd {
			m.toPrompt()
			m.input, m.back = completed("#", cmd, false), 0
			if !needsArg(cmd) {
				return m.submit()
			}
			return nil
		}}, q, cmd.Name+" "+cmd.Description); ok {
			items = append(items, it)
		}
	}
	return items
}

// barStart starts a new agent with what's typed as its task, the way the
// Prompt does.
func (m *Model) barStart(task string) barItem {
	return barItem{glyph: paint(cGreen, "+"), title: task, meta: "new agent in " + tildify(m.startDir()), run: func(m *Model) tea.Cmd {
		m.toPrompt()
		m.input, m.back, m.pastes = []rune(task), 0, pastes{}
		return m.submit()
	}}
}

// toPrompt puts the keys in Agents' Prompt, where new sessions start.
func (m *Model) toPrompt() {
	m.goView(placeAgents)
	if m.zen {
		m.setZen(false)
	}
	m.paneFocus, m.embedded = false, false
	if m.inKind != inPrompt {
		m.input, m.back, m.inKind = m.input[:0], 0, inPrompt
	}
}

// goView moves to a place unless it's already there: setView starts the
// place afresh, dropping what's typed in the Prompt.
func (m *Model) goView(v int) {
	if m.view != v || m.mode == modeHelp {
		m.setView(v)
	}
}

// barAgents are the agents, those needing you first, then the latest.
func (m *Model) barAgents(q, group string) []barItem {
	now := m.snap.At
	agents := append([]*fleet.Agent{}, m.allAgents()...)
	sort.SliceStable(agents, func(i, j int) bool {
		a, b := agents[i], agents[j]
		if a.NeedsYou() != b.NeedsYou() {
			return a.NeedsYou()
		}
		return a.Age(now) < b.Age(now)
	})
	var items []barItem
	for _, a := range agents {
		if group != "" && m.groupOf[a.Key] != group {
			continue
		}
		if it, ok := m.barAgentItem(a, q, now); ok {
			items = append(items, it)
		}
	}
	return items
}

// barAgentItem is a's row in the bar, if it matches q.
func (m *Model) barAgentItem(a *fleet.Agent, q string, now time.Time) (barItem, bool) {
	repo := a.Repo
	if repo != "" {
		repo = filepath.Base(repo)
	}
	where := strings.Trim(repo+" · "+a.Branch, " ·")
	it := barItem{glyph: agentGlyph(a), title: oneLine(a.DisplayName), meta: where, tail: agentState(a, now), run: func(m *Model) tea.Cmd { return m.goAgent(a) }}
	return matchItem(it, q, a.Repo+" "+a.Branch+" "+a.Group+" "+a.State+" "+agentName(a.Kind))
}

// barSorts are the orders ctrl+s steps the bar's agents through.
var barSorts = []string{"status", "latest", "project", "worktree", "model", "harness"}

// barSortedGroups are the agents matching q, latest first, in one section
// or a section each by the bar's sort.
func (m *Model) barSortedGroups(q string) []barGroup {
	now := m.snap.At
	agents := slices.Clone(m.allAgents())
	sort.SliceStable(agents, func(i, j int) bool { return agents[i].Age(now) < agents[j].Age(now) })
	var out []barGroup
	at := map[string]int{}
	for _, a := range agents {
		it, ok := m.barAgentItem(a, q, now)
		if !ok {
			continue
		}
		title := "Latest"
		switch m.barSort {
		case "project":
			title = filepath.Base(cmp.Or(a.Repo, a.Cwd))
		case "worktree":
			title = strings.Trim(tildify(a.Cwd)+" · "+a.Branch, " ·")
		case "model":
			title = cmp.Or(modelWord(a.Kind, a.Spend.Model), "model unknown")
		case "harness":
			title = agentName(a.Kind)
		}
		i, seen := at[title]
		if !seen {
			i, at[title] = len(out), len(out)
			out = append(out, barGroup{title: title})
		}
		out[i].items = append(out[i].items, it)
	}
	return out
}

type barGroup struct {
	title string
	items []barItem
}

// barAgentGroups are the agents in sections by what they need of you:
// needing you, working, idle, then earlier ones.
func (m *Model) barAgentGroups(q string) []barGroup {
	if m.barSort != "status" && m.barSort != "" {
		return m.barSortedGroups(q)
	}
	order := []string{"Needs you", "Working", "Idle", "Earlier"}
	by := map[string][]barItem{}
	for _, it := range m.barAgents("", "") {
		g := "Earlier"
		switch it.tail {
		case "needs you", "waiting":
			g = "Needs you"
		case "working":
			g = "Working"
		case "idle":
			g = "Idle"
		}
		by[g] = append(by[g], it)
	}
	var out []barGroup
	for _, g := range order {
		if len(by[g]) > 0 {
			out = append(out, barGroup{g, by[g]})
		}
	}
	return out
}

func agentGlyph(a *fleet.Agent) string {
	switch {
	case a.NeedsYou() || a.Waiting():
		return paint(cYellow, "●")
	case a.Live():
		return paint(cOrange, "●")
	case a.PID != 0:
		return paint(cSub, "○")
	}
	return faint("·")
}

func agentState(a *fleet.Agent, now time.Time) string {
	switch {
	case a.NeedsYou():
		return "needs you"
	case a.Waiting():
		return "waiting"
	case a.Live():
		return "working"
	case a.PID != 0:
		return "idle"
	}
	return age(a.Age(now)) + " ago"
}

// goAgent selects an agent and opens its Session.
func (m *Model) goAgent(a *fleet.Agent) tea.Cmd {
	m.goView(placeAgents)
	if m.zen {
		m.setZen(false)
	}
	m.hostedShow(a.Key)
	m.sel = a.Key
	return m.focusPane(a)
}

// barTurns are the open conversation's turns, the latest first, or its
// matches for the query.
func (m *Model) barTurns(c *hostConn, q string) []barItem {
	var items []barItem
	// #12 is turn 12.
	if n, err := strconv.Atoi(strings.TrimPrefix(strings.TrimSpace(q), "#")); err == nil && strings.HasPrefix(strings.TrimSpace(q), "#") {
		for _, t := range c.sess.Turns {
			if t.N == n {
				ref := fmt.Sprintf("t%d", n)
				items = append(items, barItem{glyph: paint(cSub, "›"), title: oneLine(t.Prompt), tail: fmt.Sprintf("#%d", n), score: 1000, run: func(m *Model) tea.Cmd {
					m.jumpInPane(ref)
					return nil
				}})
			}
		}
		return items
	}
	if len(convo.Words(q)) == 0 && !convo.Filtered(q) {
		for i := len(c.sess.Turns) - 1; i >= 0; i-- {
			t := c.sess.Turns[i]
			text := oneLine(t.Prompt)
			if text == "" {
				continue
			}
			meta := fmt.Sprintf("#%d", t.N)
			if c.sess.Partial {
				meta = "" // numbered from the end read so far: not its own yet
			}
			if !t.Start.IsZero() {
				meta = strings.TrimSpace(age(time.Since(t.Start)) + " ago  " + meta)
			}
			ref := fmt.Sprintf("t%d", t.N)
			items = append(items, barItem{glyph: paint(cSub, "›"), title: text, tail: meta, run: func(m *Model) tea.Cmd {
				m.jumpInPane(ref)
				return nil
			}})
		}
		return items
	}
	words := convo.Words(q)
	for _, h := range c.sess.Search(q) {
		ref := h.Ref
		items = append(items, barItem{glyph: whoGlyph(h.Who), title: homeless(h.Snippet), lit: litWords(homeless(h.Snippet), words), tail: fmt.Sprintf("#%d %s", h.Turn, h.Who), run: func(m *Model) tea.Cmd {
			m.jumpInPane(ref)
			return nil
		}})
	}
	return items
}

// homeless is text with the home folder in it written ~, as paths are
// everywhere else.
func homeless(s string) string {
	if home, _ := os.UserHomeDir(); home != "" {
		return strings.ReplaceAll(s, home, "~")
	}
	return s
}

func whoGlyph(who string) string {
	switch who {
	case "you":
		return paint(cText, "›")
	case convo.WhoAgent:
		return paint(cOrange, "✻")
	}
	return paint(cSub, "·")
}

// barTranscripts are the matches found in other agents' transcripts so far.
func (m *Model) barTranscripts(words []string) []barItem {
	q := stripIn(string(m.bar.query))
	var items []barItem
	for _, f := range m.bar.found {
		for _, h := range f.hits {
			f, h := f, h
			items = append(items, barItem{glyph: whoGlyph(h.Who), title: homeless(h.Snippet), lit: litWords(homeless(h.Snippet), words), meta: oneLine(f.name), tail: fmt.Sprintf("#%d", h.Turn), run: func(m *Model) tea.Cmd {
				return m.goFound(f, h, q)
			}})
		}
	}
	return items
}

// goFound opens the agent a transcript match is in and, once its pane has
// the conversation, opens the match there.
func (m *Model) goFound(f barFound, h convo.Hit, q string) tea.Cmd {
	a := m.anyAgent(f.key)
	if a == nil {
		m.flash(f.name+" isn't in the list any more", true)
		return nil
	}
	m.jump = &barJump{key: f.key, name: f.name, ref: h.Ref, query: q, snip: h.Snippet, at: time.Now()}
	cmd := m.goAgent(a)
	m.applyJump()
	return cmd
}

// jumpInPane opens a turn or step of the pane's conversation and selects it.
func (m *Model) jumpInPane(ref string) {
	c := m.host
	if c == nil {
		return
	}
	m.goView(placeAgents)
	turn, _, isStep := strings.Cut(ref, ":")
	c.open[turn] = true
	if isStep {
		c.open[ref] = true
		if p := c.sess.ParentRef(ref); p != "" {
			c.open[p] = true // a subagent's step shows under its opened parent
		}
	}
	c.sel = ref
	c.view, c.selMoved = 0, true
	m.preview, m.paneFocus = true, true
}

// applyJump finishes a jump into a conversation once it's open: the match
// is looked for again as the pane has it, until it's found or a few seconds
// have gone by.
func (m *Model) applyJump() {
	j := m.jump
	if j == nil {
		return
	}
	if time.Since(j.at) > 6*time.Second {
		m.jump = nil
		m.flash("couldn't find that in "+j.name+"'s conversation", true)
		return
	}
	c := m.host
	if c == nil || c.key != j.key || !c.ready {
		return
	}
	ref := ""
	if j.query == "" {
		turn, _, _ := strings.Cut(j.ref, ":")
		for _, t := range c.sess.Turns {
			if fmt.Sprintf("t%d", t.N) == turn {
				ref = j.ref
			}
		}
	} else {
		for _, h := range c.sess.Search(j.query) {
			if h.Snippet != j.snip {
				continue
			}
			if ref == "" || h.Ref == j.ref {
				ref = h.Ref
			}
		}
	}
	if ref == "" {
		return // the host may still be replaying
	}
	m.jump = nil
	m.jumpInPane(ref)
}

// here is where the screen is now, to come back to.
func (m *Model) here() *spot {
	s := &spot{view: m.view, agentsPage: m.work.page, mode: m.mode, projSel: m.work.projSel, settingsPage: m.settingsPage, zen: m.zen, key: m.sel}
	if a := m.agentByKey(m.sel); a != nil {
		s.name = oneLine(a.DisplayName)
	}
	if d := m.dialog; d != nil && (m.view == placeSettings || m.view == placeHarnesses) {
		s.settingsPage, s.settingsName = d.page, m.settingsPages()[d.page].name
	}
	if c := m.host; c != nil && c.key == m.sel && m.paneFocus {
		s.paneView, s.ref = c.view, c.sel
	}
	return s
}

func (s *spot) where() string {
	switch s.view {
	case placeAgents:
		if s.mode == modeWall {
			return "Agents › Wall"
		}
	case placeProjects:
		switch s.projSel {
		case paneTemp:
			return "Projects › Temporary"
		case paneSystem:
			return "Projects › System"
		}
		return "Projects"
	case placeHarnesses:
		return "Harnesses › " + s.settingsName
	case placeSettings:
		return "Settings › " + s.settingsName
	}
	if s.zen {
		return "Zen"
	}
	if s.name == "" {
		return "Agents"
	}
	return s.name
}

// goSpot goes back to where the last jump came from, and makes where it
// went the new way back.
func (m *Model) goSpot(s *spot) tea.Cmd {
	m.barBack = m.here()
	switch s.view {
	case placeSettings, placeHarnesses:
		m.setSettingsPage(s.settingsPage)
		return nil
	case placeProjects:
		m.goView(placeProjects)
		m.pickInProjects(s.projSel)
		return m.projectsOpen()
	}
	m.goView(placeAgents)
	switch s.mode {
	case modeWall:
		m.setAgentsPage(agentsWall)
		return m.refreshFolders()
	default:
		m.setAgentsPage(agentsList)
	}
	if s.zen != m.zen {
		m.setZen(s.zen)
	}
	a := m.anyAgent(s.key)
	if a == nil || s.zen {
		return nil
	}
	m.hostedShow(a.Key)
	m.sel = a.Key
	if s.ref == "" {
		if c := m.host; c != nil && c.key == a.Key {
			c.view = s.paneView
		}
		return nil
	}
	m.jump = &barJump{key: a.Key, name: s.name, ref: s.ref, at: time.Now()}
	cmd := m.focusPane(a)
	m.applyJump()
	return cmd
}

// litWords are the runes of s where the query's words are found.
func litWords(s string, words []string) []int {
	var at []int
	for _, w := range words {
		i, j := convo.FindFold(s, w)
		if i < 0 {
			continue
		}
		from := len([]rune(s[:i]))
		for k := range []rune(s[i:j]) {
			at = append(at, from+k)
		}
	}
	return at
}

// barBox draws the bar: the query, then the rows that fit, by section.
func (m *Model) barBox(w, h int) []string {
	b := m.bar
	inner := w - 4
	q := string(b.query)
	cur := paint(cOrange, "▏")
	field := paint(cText, string(b.query[:b.pos])) + cur + paint(cText, string(b.query[b.pos:]))
	sc := b.scope()
	if q == "" {
		field = cur + dim(barHolder(sc))
	}
	head := paint(cOrange+bold, "❯ ") + field
	if sc.kind != "" {
		head = barChip + paint(cOrange, " ⌕ ") + barChip + paint(cText+bold, sc.label+" ") + reset + " " + head
	}
	if m.store.Config.SearchTranscriptsOnKey && sc.transcripts() && b.search == nil {
		head = spread(head, keys(m.searchKey(), "search transcripts"), inner)
	} else if with := dim("new agents: ") + m.startWith(m.startDir(), false); cellw.String(head+with)+4 <= inner {
		head = spread(head, with, inner) // what "Start an agent" starts as
	}
	out := []string{head, faint(strings.Repeat("─", inner))}

	// Rows, with a heading where each section starts.
	type row struct {
		text string
		item int // -1 for a heading
	}
	// Metas line up in one column, just past the longest title that has one.
	col := 0
	for _, it := range b.items {
		if it.meta != "" {
			col = max(col, cellw.String(oneLine(it.title)))
		}
	}
	var rows []row
	for i, it := range b.items {
		if i == 0 || it.section != b.items[i-1].section {
			if i > 0 {
				rows = append(rows, row{"", -1})
			}
			rows = append(rows, row{m.barHeading(it.section, inner), -1})
		}
		rows = append(rows, row{barRow(it, inner, col, i == b.cursor), i})
	}
	if len(b.items) == 0 {
		rows = append(rows, row{"", -1}, row{dim("  nothing matches"), -1})
		if b.search != nil && !b.done {
			rows[len(rows)-1] = row{dim("  searching transcripts…"), -1}
		}
	}
	avail := max(3, h-len(out)-2)
	sel := 0
	for i, r := range rows {
		if r.item == b.cursor {
			sel = i
		}
	}
	// Keep the selection on screen, with its heading when it's near the top.
	if sel < b.top+1 {
		b.top = max(0, sel-2)
	}
	if sel >= b.top+avail {
		b.top = sel - avail + 1
	}
	b.top = max(0, min(b.top, len(rows)-avail))
	for i := b.top; i < min(len(rows), b.top+avail); i++ {
		out = append(out, rows[i].text)
	}
	out = append(out, "", m.barHint(inner))
	return out
}

var barChip string

// barHolder is what the empty box says it takes, in each scope.
func barHolder(sc barScope) string {
	switch sc.kind {
	case "chat":
		return "words in this chat · #12 for turn 12 · is:failed is:edit file:x turn:3-5"
	case "group":
		return "an agent in " + sc.group + ", or words to search their transcripts"
	case "agents":
		return "an agent, or words to search every agent's transcript"
	case "place":
		return "a page of " + sc.label
	}
	return "go to an agent, a place or a turn · # for commands"
}

// barHint is the row of keys under the results: it says what ctrl+f and
// ctrl+k will do from here, so the scopes teach themselves.
func (m *Model) barHint(w int) string {
	b := m.bar
	last := len(b.scopes) - 1
	pairs := []string{"↑↓", "choose", "enter", "go"}
	switch {
	case b.at < last-1:
		pairs = append(pairs, "ctrl+f", "wider: "+b.scopes[b.at+1].label, "ctrl+k", "everywhere")
	case b.at == last-1:
		pairs = append(pairs, "ctrl+f · ctrl+k", "everywhere")
	case last > 0:
		pairs = append(pairs, "ctrl+f", "just "+b.scopes[0].label)
	}
	if b.scope().kind == "" {
		sort := m.barSort
		if sort == "" {
			sort = "status"
		}
		pairs = append(pairs, "#", "commands", "tab", "next section", "ctrl+s", "by "+sort)
	}
	if b.scope().transcripts() {
		pairs = append(pairs, "in:name is:failed", "narrow")
	}
	return keysFit(w, append(pairs, "esc", "close")...)
}

// barStatus says how the transcript search is going.
func (m *Model) barStatus() string {
	b := m.bar
	switch {
	case b.search == nil && searchable(string(b.query)) && !m.store.Config.SearchTranscriptsOnKey:
		return dim(" … ")
	case b.search == nil:
		return ""
	case !b.done:
		return paint(cSub, fmt.Sprintf(" searching %d/%d transcripts ", b.searched, b.toSearch))
	}
	n := 0
	for _, f := range b.found {
		n += f.total
	}
	return dim(fmt.Sprintf(" %d in %d of %d transcripts ", n, len(b.found), b.toSearch))
}

func (m *Model) barHeading(section string, w int) string {
	meta := ""
	if section == "Transcripts" {
		shown, total := 0, 0
		for _, f := range m.bar.found {
			shown += len(f.hits)
			total += f.total
		}
		if total > shown {
			meta = fmt.Sprintf("%d more; open one to see all", total-shown)
		}
	}
	return rule(section, meta, w)
}

// barRow is one result in columns: its glyph, the title lit where it
// matched, its meta in a column of its own, and a short tail at the right
// edge, so the eye runs down each rather than across the box.
func barRow(it barItem, w, col int, sel bool) string {
	tail := ""
	if it.tail != "" {
		tail = "  " + dim(it.tail)
	}
	tw := w - 3 - cellw.String(tail)
	if it.meta != "" {
		tw = min(tw, max(12, min(col, w*9/20)))
	}
	title := litTitle(oneLine(it.title), it.lit, tw)
	line := " " + it.glyph + " " + title
	if it.meta != "" {
		line += strings.Repeat(" ", max(2, 3+tw-cellw.String(line)+2))
		if mw := w - cellw.String(line) - cellw.String(tail); mw > 3 {
			line += ansi.Truncate(dim(it.meta), mw, "…")
		}
	}
	line += strings.Repeat(" ", max(0, w-cellw.String(line)-cellw.String(tail))) + tail
	if sel {
		return highlight(paint(cOrange, "▍")+line[1:], w)
	}
	return line
}

// litTitle paints title, its matched runes bright, cut to w cells.
func litTitle(title string, lit []int, w int) string {
	if cellw.String(title) > w {
		title = ansi.Truncate(title, w, "…")
	}
	if len(lit) == 0 {
		return paint(cText, title)
	}
	on := map[int]bool{}
	for _, i := range lit {
		on[i] = true
	}
	var sb strings.Builder
	for i, r := range []rune(title) {
		if on[i] {
			sb.WriteString(paint(cOrange+bold, string(r)))
		} else {
			sb.WriteString(paint(cText, string(r)))
		}
	}
	return sb.String()
}

// overlayBar draws the bar over the screen, a little below the top, the way
// a command bar drops down rather than sitting in the middle: a frame that
// glows from orange to ember, its name set into the top edge and the
// transcript search's progress into the bottom one, a light running round it
// while the search is under way, and a shadow on the screen behind.
func (m *Model) overlayBar(base string) string {
	lines := strings.Split(base, "\n")
	bw := min(m.w-6, 110)
	bh := max(8, m.h*2/3)
	body := m.barBox(bw, bh-2)
	inner := bw - 4
	b := m.bar
	e := barEdge{w: bw, h: len(body) + 2, glint: -1}
	if b.search != nil && !b.done {
		e.glint = b.glint
	}
	title, key := paint(cText+bold, " ✻ Go anywhere "), dim(" ctrl+k ")
	if sc := b.scope(); sc.kind != "" {
		title, key = paint(cText+bold, " ⌕ Find in "+sc.label+" "), dim(" ctrl+f ")
	}
	box := []string{e.line(0, "╭", "╮", title, key)}
	for i, l := range body {
		box = append(box, e.cell(0, i+1, "│")+panel(" "+fit(l, inner)+" ")+e.cell(bw-1, i+1, "│"))
	}
	box = append(box, e.line(e.h-1, "╰", "╯", "", m.barStatus()))
	top := max(1, min(m.h/8, len(lines)-len(box)-1))
	left := (m.w - bw - 2) / 2
	m.keepOverlay(left, top, bw, len(box), left+2, top+1)
	plain := make([]string, len(lines))
	for y := range lines {
		plain[y] = unplace(ansi.Strip(fit(lines[y], m.w)))
		lines[y] = faint(plain[y])
	}
	// The shadow: two cells right of the frame and one row under it, the
	// screen there darker still.
	shade := func(y, from, to int) string {
		return barShadow + strings.ReplaceAll(ansi.Cut(plain[y], from, to), reset, reset+barShadow) + reset
	}
	for i, l := range box {
		y := top + i
		if y >= len(lines) {
			break
		}
		right := faint(ansi.Cut(plain[y], left+bw, m.w))
		if i > 0 {
			right = shade(y, left+bw, left+bw+2) + faint(ansi.Cut(plain[y], left+bw+2, m.w))
		}
		lines[y] = faint(ansi.Cut(plain[y], 0, left)) + l + right
	}
	if y := top + len(box); y < len(lines) {
		lines[y] = faint(ansi.Cut(plain[y], 0, left+2)) + shade(y, left+2, left+bw+2) + faint(ansi.Cut(plain[y], left+bw+2, m.w))
	}
	return strings.Join(lines, "\n")
}

var barShadow string

// barEdge colours the bar's frame cell by cell: orange at the top left
// cooling to ember at the bottom right, and, while transcripts are being
// searched, a glint with a fading tail running clockwise round it.
type barEdge struct {
	w, h  int
	glint int // where the glint is along the edge, or -1
}

// around is how far along the edge, clockwise from the top left, a cell is.
func (e barEdge) around(x, y int) int {
	switch {
	case y == 0:
		return x
	case x == e.w-1:
		return e.w - 1 + y
	case y == e.h-1:
		return 2*(e.w-1) + e.h - 1 - x
	}
	return 2*(e.w-1) + 2*(e.h-1) - y
}

func (e barEdge) color(x, y int) string {
	t := 0.65*float64(x)/float64(max(1, e.w-1)) + 0.35*float64(y)/float64(max(1, e.h-1))
	r, g, b := mix([3]float64{232, 128, 92}, [3]float64{110, 52, 40}, t)
	if e.glint >= 0 {
		n := 2*(e.w-1) + 2*(e.h-1)
		d := ((e.glint-e.around(x, y))%n + n) % n
		if d < 14 {
			k := 1 - float64(d)/14
			r, g, b = mix([3]float64{r, g, b}, [3]float64{255, 232, 205}, k*k)
		}
	}
	return rgb(int(r), int(g), int(b))
}

func mix(a, b [3]float64, t float64) (float64, float64, float64) {
	t = max(0, min(1, t))
	return a[0] + (b[0]-a[0])*t, a[1] + (b[1]-a[1])*t, a[2] + (b[2]-a[2])*t
}

func (e barEdge) cell(x, y int, s string) string { return e.color(x, y) + s + reset }

// line is the top or bottom edge, with a label set into it at each end.
func (e barEdge) line(y int, l, r, left, right string) string {
	var sb strings.Builder
	sb.WriteString(e.cell(0, y, l))
	x := 1
	dash := func(n int) {
		for ; n > 0; n-- {
			sb.WriteString(e.cell(x, y, "─"))
			x++
		}
	}
	lw, rw := cellw.String(left), cellw.String(right)
	if lw+rw+6 > e.w {
		left, right, lw, rw = "", "", 0, 0
	}
	if lw > 0 {
		dash(1)
		sb.WriteString(left)
		x += lw
	}
	dash(e.w - 1 - x - rw - map[bool]int{true: 1, false: 0}[rw > 0])
	if rw > 0 {
		sb.WriteString(right)
		x += rw
		dash(1)
	}
	sb.WriteString(e.cell(e.w-1, y, r))
	return sb.String()
}

type barGlintMsg int

// glintTick moves the glint along while a search runs.
func glintTick(gen int) tea.Cmd {
	return tea.Tick(40*time.Millisecond, func(time.Time) tea.Msg { return barGlintMsg(gen) })
}

// sendNowKey is the key a Session's hints name for sending now: yours if
// you moved it, else ctrl+enter. A terminal that can't tell ctrl+enter
// apart (macOS's Terminal) has none: ctrl+s, which it had, is the stash's.
func (m *Model) sendNowKey() string {
	if ks := m.keyMap().Keys("session.send"); len(ks) > 0 {
		return ks[0].String()
	}
	return "ctrl+enter"
}
