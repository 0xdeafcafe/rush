// Package ui is the rush view: the native layout plus cost, time, CPU/RAM,
// preview, processes, accounts, groups and folder moves.
package ui

import (
	"cmp"
	"errors"
	"fmt"
	"hash/maphash"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/actions"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/hooks"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/menubar"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/room"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/0xdeafcafe/rush/internal/statusline"
	"github.com/0xdeafcafe/rush/internal/theme"
	"github.com/0xdeafcafe/rush/internal/update"
)

type mode int

const (
	modeList mode = iota
	modeHelp
	modeEff
	modeProjects
	modeWall
)

type inputKind int

const (
	inPrompt inputKind = iota
	inRename
	inGroup
	inReply
)

var groupModes = []string{"status", "agent", "group"}

// confirmation is a question asked in a box over the screen.
type confirmation struct {
	question string
	detail   string
	onYes    func() tea.Cmd
	onBang   func() tea.Cmd
	bangText string
	// A question between two choices rather than yes or cancel: y and n
	// each do something, labelled yesText and noText; esc still cancels.
	yesText, noText string
	onNo            func() tea.Cmd
	// again is a key that says yes too: the arrow pushed once more past
	// the edge that asked (edgePush).
	again string
	// more are choices beside yes, each on a key of its own.
	more []confirmChoice
}

// confirmChoice is a key a confirmation takes beside y and n.
type confirmChoice struct {
	key, text string
	do        func() tea.Cmd
}

type Model struct {
	reloadFields // #reload, and what it carries
	// upd is Settings › Updates, and the count in the key line.
	upd updatesState
	// sendModes are how enter sends to each session while it works, by key.
	sendModes map[string]sendMode
	// keys is the keymap in force: see keybind.go.
	keys keyState
	// hooks is this window's way to its plugins, which never waits; see
	// pluginui.go.
	hooks    *hooks.Client
	hookSeen hookSeen
	// bundledOn says which bundled plugins are on, as last read; nil
	// until then.
	bundledOn map[string]bool

	store     *state.Store
	loader    *fleet.Loader
	scanner   *fleet.Scanner
	snap      *fleet.Snapshot
	launchDir string
	version   string

	w, h     int
	mode     mode
	tick     int
	scanning bool
	loaded   bool

	// switching is set while ~/.claude is being signed in as another
	// login; switchedAt is when it last was.
	switching  bool
	switchedAt time.Time
	keepFailed bool // said once until it works again
	// quotas are other agents' account limits, by profile folder or
	// account key.
	quotas    map[string]usage.Quota
	accts     accountsState
	startOver *startOver                // what the next session starts as, picked on the start sheet
	listed    map[string][]agent.Choice // models read from an agent's home (Codex's cache), by kind
	resumedAt time.Time                 // when sessions a limit stopped were last told to carry on

	sel                string
	shown              string // the agent last picked, still shown while a folded section is
	order              []*fleet.Agent
	lines              []listLine
	scroll             int
	preview            bool
	full               bool
	peekFrom           string // the agent whose Session alone you left to peek at Agents
	previews           map[string]previewEntry
	btws               map[string]*btwThread // agents' side threads (/btw), by key
	quick              quickAsk              // the quick ask's float and its threads
	side               sidekick              // the sidekick column's notes, beside a wide Session
	live               *live
	liveOpening        string
	liveFailed         string
	liveFailedAt       time.Time
	headerPointerLines [2]string
	headerIconTail     string
	topHover           headerHover
	hover              string
	hoverAt            time.Time // when the pointer came onto hover
	tickerOn           bool      // a hovered feed post is scrolling
	tickerPending      bool
	rowKeys            []string
	footHits           []footHit    // the side list footer's recent setups, by column
	events             []agentEvent // the feed at the list's foot, oldest first
	listTop            int
	lastClick          time.Time
	// pressAt and pressXY are the last left press, and dbl whether the
	// one being handled is a second on the same cell: a double-click.
	pressAt time.Time
	pressXY [2]int
	dbl     bool

	input  []rune
	back   int // cursor distance from the input's end
	anchor int // selection start + 1; 0 when nothing is selected
	// promptTop is the Prompt box's first row on screen, kept between draws.
	promptTop int
	// pendingCopy is text to put on the clipboard with the next update.
	pendingCopy string
	// Where the Prompt's box was drawn, so a click can place the cursor.
	promptBox    box
	shelfHits    []shelfHit // the shelf's setups on promptBox's top border, by screen column
	promptBoxIdx int
	promptBoxY   int
	inKind       inputKind
	slashSel     int // the Prompt's command picker's selection
	// paneFocus sends keys to a rush-mode session's pane instead of the
	// list and its prompt.
	paneFocus bool
	dragging  bool      // resizing the list by its edge
	boxDrag   int       // selecting by dragging in an input box: 1 the Session's, 2 the prompt's
	imgs      imageRefs // images in the Prompt, each [Image #N] in its text
	// claudeView is a Claude Code agent's Session view: 0 its live screen,
	// 1 the summary.
	claudeView int
	zen        bool // the Zen view: only the agent that needs you
	peek       zenPeek
	frameLen   int // bytes in the last frame, to size the next
	lastKeyAt  time.Time
	paneTop    int // screen row of the pane's first line, for clicks
	// host is the connection to the rush-mode session the pane shows.
	host        *hostConn
	hostOpening string
	dirIdx      int
	pickedDir   string // a folder typed for new sessions, offered first
	pickedFor   string // the agent selected when a folder was picked; the pick holds while it stays selected
	startInTree bool   // alt+l: new sessions follow a selected worktree agent into its worktree, not its main checkout
	dirs        startDirsMemo
	// hold is where the agent being typed in sat at the last rebuild, its
	// section and place in it: it stays there while the draft is there.
	hold struct {
		key, section string
		rank, at     int
	}

	status    string
	statusErr bool
	statusAt  time.Time
	quitArmed time.Time
	// escAt is when esc last met an empty box in escKey's Session: esc
	// again soon after asks to close it (askClose).
	escAt  time.Time
	escKey string
	// closedAt is when ctrl+x last stopped closedKey: ctrl+x again soon
	// after moves it to Done without waiting for its process to go.
	closedAt  time.Time
	closedKey string
	confirm   *confirmation
	chipHot   chipHover
	dialog    *dialog
	picker    *picker
	community *communitySheet // shared local help board
	stream    streamState     // the board as a timeline at the list's foot
	broadcast map[string]bool // #broadcast picking: the agents the next message goes to
	roomFeed  *roomFeed       // the room the pane shows: see roomfeed.go
	rooms     roomRows        // rooms as list rows: see roomrows.go
	// lastPanel is the panel chosen last, chosen again in the next room set
	// up; verdicts the verdicts waiting as a card in their chat, by the
	// chat's agent key; verdictsPicked the verdicts already picked for you
	// in their chat. See discuss.go.
	lastPanel      []startOver
	verdicts       map[string]room.Summary
	verdictsPicked map[string]bool
	sheet          sheet // /fork, /rewind, /plugins, /statusline, /skills: see sheet.go
	// rewound holds the message /rewind put back, by agent, for the box
	// once the pane reconnects.
	rewound   map[string]string
	bars      statusline.Bars  // rush's own status lines: see bars.go
	barDrops  map[int][]string // segments each of their lines last left out for room
	embedded  bool
	promptFor string
	listW     int
	pastes    pastes // long pastes in the main box, shown as chips
	blurred   bool   // the terminal says rush isn't the focused window
	// undo is the Prompt's; a Session's box has its own.
	undo undoStack
	// The terminal's background and text, once it has said; rush's
	// colours are made from them.
	termBG, termFG *theme.RGB
	ground         theme.Ground // what the colours are made for now
	colored        bool         // whether they've been made yet
	sameFrame      bool         // the last message changed nothing on screen
	lastFrame      string       // what View drew last
	// openFailed is when a Session last failed to open, by agent key; zen
	// skips those for a while rather than sticking on one it can't show.
	openFailed   map[string]time.Time
	localQ       map[string]*localQueue // messages waiting for Claude Code sessions, by agent key
	subNotes     map[string][]subNote   // what you sent subagents that their conversations don't show, by subQKey
	online       onlineWatch            // sessions an API error stopped, told to continue once it can be reached
	subNudged    map[string]bool        // agent key + subagent run told it went quiet: see stucksubs.go
	moveWhenIdle map[string]bool        // agents to move to rush mode when their turn ends
	divHover     bool                   // the mouse is on the edge between Agents and the Session
	ptrX, ptrY   int                    // where the mouse was last seen
	ptrSeen      bool
	pointer      string     // the pointer's shape last asked of the terminal
	sheetAt      [2]int     // where the open sheet's body was drawn: x, y
	over         overlayHit // the box over the screen, as last drawn, for the mouse
	hibernated   map[string]bool
	offline      bool // never ask Anthropic for usage (--soak)
	// newer is the rush that's out when it's newer than this one; #update
	// installs it.
	newer        update.Info
	updating     bool
	attached     string
	view         int
	settingsPage int  // the Settings place's page: a tab of the dialog
	harnessPage  int  // and the Harnesses place's
	helpPage     int  // the guide's tab: helpPages
	onboard      bool // teaching: Getting started and tips
	cardShown    bool // Getting started was under the list last frame

	lastState   map[string]string
	lastErr     map[string]bool // agents last seen with an error, so one starting is noticed
	fx          clkFX           // what clanker is reacting to
	fxOn        bool            // his reaction is ticking
	fastPending bool            // a fastMsg is on its way
	edgeKey     string          // edgeKey, edgeAt and edgeN are arrow presses into a box's edge: edgePush
	edgeAt      time.Time
	edgeN       int
	fxKick      bool         // a reaction started this update; its ticking needs starting
	measuring   bool         // temp work is being measured in the background
	clean       cleanup      // the Cleanup view's worktrees, and the tidy-up
	work        workState    // the Projects place and the Agents place's Wall
	stackFor    int          // the list width a preview decides two-line rows for
	wall        wallState    // the Wall page
	grid        gridState    // agents pinned as tiles beside the Session
	barSort     string       // the command bar's agent order: see barSorts
	reaper      fleet.Reaper // ends what agents leave running when they stop
	squeezing   bool         // transcripts are being compressed in the background

	bar     *cmdBar  // the command bar, while it's open
	barBack *spot    // where the bar last jumped from
	jump    *barJump // a jump into a conversation that's still opening
	// intervening is #intervene @agent while that agent's chat opens.
	intervening *pendingIntervene
	// listFilter narrows the Agents view to what's typed after alt+f: nil
	// when it's closed. See listfilter.go.
	listFilter *listFilterState
	// groupOf is the list section each agent is in, folded or not.
	groupOf map[string]string
	folders folderCache // what git says of the folders in the list
	// projTitles are the list's project names by folderKey, for the tag
	// a row carries where no project heads it.
	projTitles map[string]string
	// runSince is when each working agent's run began, as the list saw it
	// start; zero for one already working when rush opened.
	runSince map[string]time.Time
	runInit  bool
	// hosted is the rush-mode session shown alone (NewHosted), and hostedKey
	// its agent's key once the snapshot has it.
	hosted, hostedKey string
	// hostedList is hosted with Agents shown beside the session (ctrl+6).
	hostedList bool
	// hostedAway is hosted with the keys off the message box, after esc:
	// esc again stops the turn, and anything else goes back to the box.
	hostedAway bool
	// snapWanted is a reading of the fleet asked for, snapLoading one
	// being made; selectOnLoad is an agent to select once one has it.
	snapWanted, snapLoading bool
	fleetRead               bool                            // a reading has landed: before it, the list says it's reading
	procWords               known[procKey, string]          // processes' words, as shortCmd draws them
	pickTrees               known[string, []string]         // repositories' worktrees, for the folder picker
	localCmds               known[cmdsKey, []agent.Command] // commands and skills on disk: see commandsOf
	paths                   known[string, pathFact]         // what's at paths typed, pasted or dropped: see lookPath
	selectOnLoad            string
	// keysDisambiguated is when the terminal said it tells ctrl+enter
	// from enter.
	keysDisambiguated bool
	// fleetAgents are every agent, which hosted's header still counts.
	fleetAgents []*fleet.Agent
	// sidebars are the plugins' arrangements of the list, read from
	// sidebarFiles; renamed holds the names an arrangement replaced.
	sidebars     []plugin.Sidebar
	sidebarFiles plugin.Sidebars
	renamed      map[*fleet.Agent]string
	// drawing is set while View draws a frame, when nothing changes:
	// kindMemo keeps what startKindIn worked out for it.
	drawing      bool
	kindMemo     kindMemo
	accountFrame accountFrame

	// relayPending is whether a relayoutMsg is on its way.
	relayPending bool
}

type previewEntry struct {
	p    agent.Preview
	size int64
}

type lineKind int

const (
	lineBlank lineKind = iota
	lineSection
	lineAgent
	lineProject // a project's heading inside a section, split by project
	lineTree    // a linked worktree's heading under its project
)

type listLine struct {
	kind   lineKind
	title  string
	meta   string
	folded bool
	peek   string
	agent  *fleet.Agent
	// root is a project line's folder, or a tree line's worktree; a tree
	// line's title is its project's folder.
	root string
	// inset is how far an agent row sits in, to line up under its
	// project's heading, or deeper under its worktree's.
	inset int
}

func sectionKey(title string) string { return "§" + title }

func New(store *state.Store, version string) *Model { return newModel(store, version, false) }

// newLoader is the fleet's reader, which reads which sessions ran which
// agents (a few dozen bytes each) as it's made.
//
//uiblock:nowait New is asked for before the program starts, so nothing waits on it
func newLoader(store *state.Store) *fleet.Loader { return fleet.NewLoader(store) }

// newModel is New, leaving past conversations out of its readings when
// skipPast (see fleet.Loader.SkipPast) until something needs every agent.
func newModel(store *state.Store, version string, skipPast bool) *Model {
	dir := os.Getenv("PWD") // the shell's word for now: Init asks the kernel
	convo.SetPixelPictures(store.Config.PixelPictures)
	m := &Model{
		store: store, loader: newLoader(store), scanner: fleet.NewScanner(),
		launchDir: dir, version: version, previews: map[string]previewEntry{},
		lastState:  map[string]string{},
		hibernated: map[string]bool{},
	}
	// Each second's refresh reads only what changed on disk; everything
	// is read afresh every few seconds all the same.
	m.loader.Watch(5 * time.Second)
	m.loader.SkipPast(skipPast)
	if store.Config.GroupBy == "" {
		store.Config.GroupBy = "status"
	}
	m.applyColors()
	convo.SetShowWhitespace(store.Config.ShowWhitespace)
	// Nothing is read here, where the first frame waits: the first reading
	// of the fleet and rush's own status lines land a moment after it.
	m.snap = &fleet.Snapshot{At: time.Now()}
	m.takeRestore()
	m.refresh()
	m.rebuild()
	m.onboard = true
	m.grid.keys, m.grid.share = slices.Clone(store.Config.Grid), store.Config.GridShare // the grid as it was left
	m.grid.under = map[string]bool{}
	for _, k := range store.Config.Grid {
		m.grid.under[k] = store.Config.GridBelow || slices.Contains(store.Config.GridBelowKeys, k)
	}
	return m
}

// launchDir is the folder rush was started in, as the kernel has it: the
// top of the checkout or worktree it's in, not a folder inside one.
func launchDir() tea.Msg {
	d, err := os.Getwd()
	if err != nil {
		return nil
	}
	if root := actions.RepoRoot(d); root != "" {
		d = root
	}
	return applyMsg(func(m *Model) tea.Cmd { m.launchDir = d; return nil })
}

// barsMsg is rush's own status lines, as read from disk.
type barsMsg statusline.Bars

// loadBars reads rush's own status lines off the UI goroutine.
func loadBars() tea.Msg { return barsMsg(statusline.LoadBars()) }

type tickMsg time.Time
type usageMsg struct {
	key string // where the reading is kept: see fleet.ReadingKey
	u   usage.Reading
}
type scanMsg map[string]fleet.Spend
type previewMsg struct {
	key string
	e   previewEntry
}
type doneMsg struct {
	text string
	err  error
}
type movedMsg struct {
	from *fleet.Agent
	to   string
}
type attachDoneMsg struct {
	agent *fleet.Agent
	err   error
}

// tick is every whole second of the clock, so every timer turns together.
func tick() tea.Cmd {
	return tea.Every(time.Second, func(t time.Time) tea.Msg { return tickMsg(t) })
}

// fastMsg redraws a timer still showing tenths of a second.
type fastMsg struct{}

// paneNow is the pane's clock; a benchmark moves it on a tenth a frame.
var paneNow = time.Now

func (m *Model) fastTick() tea.Cmd {
	if c := m.host; c == nil || !c.sess.Fast || !c.endShown || m.fastPending {
		return nil
	}
	m.fastPending = true
	return tea.Tick(100*time.Millisecond, func(time.Time) tea.Msg { return fastMsg{} })
}

func (m *Model) Init() tea.Cmd {
	watchUI()
	if m.hosted != "" {
		// Only the one session: nothing about the app as a whole.
		return tea.Batch(m.loadSnapCmd(), loadBars, launchDir, tick(), m.scan(), m.loadPreview(), startColours, m.loadKeys(), m.startHooks())
	}
	return tea.Batch(m.loadSnapCmd(), loadBars, launchDir, tick(), m.scan(), m.loadKeys(), m.startHooks(), m.watchNet(), m.fetchUsage(), m.findLogins(), m.fetchQuotas(), m.startMenuBar(), m.startView(), m.checkUpdate(), m.checkHarnesses(false), m.checkPluginApprovals(), startColours, m.loadStream())
}

// askColours asks the terminal for its background and text, which rush's
// colours are made from. A terminal that doesn't answer keeps rush's own.
var askColours = tea.Batch(tea.RequestBackgroundColor, tea.RequestForegroundColor)

// applyColors makes rush's colours for the theme in Settings, or for the
// terminal's own background and text.
func (m *Model) applyColors() {
	c := m.store.Config
	g := theme.Terminal(m.termBG, m.termFG)
	switch c.Theme {
	case "dark":
		g = theme.Dark
	case "light":
		g = theme.Light
	}
	if g != m.ground || !m.colored {
		m.ground, m.colored = g, true
		applyColors(g, c.ColorBlind)
	}
}

// startView opens on the layout you kept, or asks which, the first time.
// Agents alone kept before there was a choice counts as picking it.
func (m *Model) startView() tea.Cmd {
	c := &m.store.Config
	if c.View == "" && c.ListOnly {
		c.View = "list"
	}
	if c.View == "" {
		m.askView()
		return nil
	}
	cmd := m.openView()
	m.askMenuBar()
	return cmd
}

// startMenuBar opens the menu bar icon when it's on, building it first if
// rush changed since; one left running by an older rush is replaced.
func (m *Model) startMenuBar() tea.Cmd {
	if !m.store.Config.MenuBar || m.offline {
		return nil
	}
	return hostCmd(menubar.Start)
}

// fetchUsage refreshes every account's plan usage from Anthropic. Readings
// are shared with every other rush through a file, so an account is asked
// only when its last reading is older than usage.Every and the provider
// hasn't said to wait; offline (--soak) never asks.
func (m *Model) fetchUsage() tea.Cmd {
	path := filepath.Join(state.Dir(), "usage.json")
	offline := m.offline
	p := m.store.Config.ActiveAccount().Profile()
	cmds := m.fetchLoginUsage()
	if pr, ok := agent.As[agent.PlanReader](loginsKind); ok {
		cmds = append(cmds, func() tea.Msg {
			u := pr.RefreshPlan(path, p, offline)
			return usageMsg{key: fleet.ReadingKey(p, u), u: u}
		})
	}
	return tea.Batch(cmds...)
}

func (m *Model) scan() tea.Cmd {
	if m.scanning {
		return nil
	}
	m.scanning = true
	targets := m.targets()
	sc := m.scanner
	return func() tea.Msg { return scanMsg(sc.Run(targets)) }
}

// startDirs are the folders a new session can start in: where rush was
// opened, then folders with agents running, then recent ones.
func (m *Model) startDirs() []string {
	k := dirsKey{m.pickedDir, m.launchDir, dirsPrint(m.snap)}
	if !m.dirs.ok || m.dirs.key != k {
		m.dirs = startDirsMemo{k, m.readStartDirs(), true}
	}
	return slices.Clip(m.dirs.out) // what's appended to it is copied
}

// startDirsMemo is startDirs as last worked out: the header asks each frame.
type startDirsMemo struct {
	key dirsKey
	out []string
	ok  bool
}

type dirsKey struct {
	picked, launchDir string
	agents            uint64 // dirsPrint of the agents it was worked out from
}

// dirsPrint fingerprints what startDirs reads of the agents, without
// allocating: a frame asks it, and an agent can move in place.
func dirsPrint(snap *fleet.Snapshot) uint64 {
	h := uint64(14695981039346656037)
	mix := func(v uint64) { h = (h ^ v) * 1099511628211 }
	for _, a := range snap.Agents {
		mix(maphash.String(dirsSeed, a.Cwd))
		mix(uint64(a.UpdatedAt.UnixNano()))
		if a.Open() {
			mix(1)
		}
		if a.Interactive || a.Past {
			mix(2)
		}
	}
	return h
}

var dirsSeed = maphash.MakeSeed()

func (m *Model) readStartDirs() []string {
	seen := map[string]bool{}
	var out []string
	add := func(d string) {
		if d != "" && !seen[d] {
			seen[d] = true
			out = append(out, d)
		}
	}
	add(m.pickedDir)
	add(m.launchDir)
	agents := append([]*fleet.Agent(nil), m.snap.Agents...)
	sort.SliceStable(agents, func(i, j int) bool {
		if agents[i].Open() != agents[j].Open() {
			return agents[i].Open()
		}
		return agents[i].UpdatedAt.After(agents[j].UpdatedAt)
	})
	for _, a := range agents {
		if len(out) >= 12 {
			break
		}
		if (a.Interactive || a.Past) && host.IsTemp(a.Cwd) {
			continue
		}
		add(a.Cwd)
	}
	return out
}

// dockLines is how many lines of the latest message the dock shows.
func (m *Model) dockLines() int {
	n := m.store.Config.DockLines
	if n == 0 {
		n = 3
	}
	return min(max(n, 1), max(1, m.h/3))
}

// startDir is where a new session starts: the selected agent's folder,
// unless a folder was picked while it was selected.
func (m *Model) startDir() string {
	if a := m.focused(); a != nil && a.Key != m.pickedFor {
		if d := m.followDir(a); d != "" {
			return d
		}
	}
	return pickDir(m.startDirs(), m.dirIdx)
}

// followDir is the folder a new session takes from a: the top of its
// repository, not the subfolder it's in; for one in a linked worktree,
// the checkout it was made from, or with alt+l the worktree.
func (m *Model) followDir(a *fleet.Agent) string {
	if inTree(a) && !m.startInTree {
		return a.Root
	}
	for _, d := range []string{a.Repo, agentDir(a), a.Cwd} {
		if workDir(d) {
			return d
		}
	}
	return ""
}

// workDir is whether d is somewhere to start work: not a temp folder, and
// not rush's or an agent's own state (a session rush ran in a transcript's
// folder is its business, not a place for yours).
func workDir(d string) bool {
	if d == "" || host.IsTemp(d) {
		return false
	}
	if rel, err := filepath.Rel(state.Dir(), d); err == nil && !strings.HasPrefix(rel, "..") {
		return false
	}
	return !strings.Contains(d, "/projects/-") // an agent's transcripts, by folder
}

// inTree is an agent working in a linked worktree.
func inTree(a *fleet.Agent) bool { return a.Root != "" && a.Repo != "" && a.Repo != a.Root }

// pickStartDir is a folder chosen for new sessions: it holds while the
// agent selected now stays selected.
func (m *Model) pickStartDir(i int) {
	m.dirIdx = i
	m.pickedFor = ""
	if a := m.focused(); a != nil {
		m.pickedFor = a.Key
	}
}

func pickDir(dirs []string, i int) string {
	if len(dirs) == 0 {
		return ""
	}
	return dirs[((i%len(dirs))+len(dirs))%len(dirs)]
}

func (m *Model) targets() []fleet.Target {
	var targets []fleet.Target
	for _, a := range m.snap.Agents {
		if a.TranscriptPath != "" {
			targets = append(targets, fleet.Target{Key: a.Key, Path: a.TranscriptPath, Live: a.Live() || a.PID != 0 || a.Subs.Direct+a.Subs.Nested > 0, Past: a.Past})
		}
	}
	return targets
}

func (m *Model) rowAt(x, y int) string {
	i := y - m.listTop
	if m.mode != modeList || m.dialog != nil || m.picker != nil || m.sheet != nil || x >= m.listW || i < 0 || i >= len(m.rowKeys) {
		return ""
	}
	return m.rowKeys[i]
}

// mouseMove only lights the row under the mouse; it takes a click to
// select it.
func (m *Model) mouseMove(x, y int) tea.Cmd {
	if h := m.rowAt(x, y); h != m.hover {
		m.hover, m.hoverAt = h, time.Now()
	}
	return m.tickerTick()
}

func (m *Model) mouseClick(x, y int) tea.Cmd {
	if m.mode == modeWall {
		return m.wallClick(x, y)
	}
	k := m.rowAt(x, y)
	if k == "" {
		return nil
	}
	switch k {
	case footNextKey:
		return m.openComposer()
	case footRecentKey:
		m.footClick(x)
		return nil
	}
	if id, ok := strings.CutPrefix(k, streamKeyPrefix); ok {
		return m.openCommunity(id)
	}
	if m.broadcast != nil && !strings.HasPrefix(k, "§") {
		m.broadcastPick(k)
		return nil
	}
	double := k == m.sel && time.Since(m.lastClick) < 400*time.Millisecond
	m.sel, m.lastClick = k, time.Now()
	if strings.HasPrefix(k, "§") {
		m.toggleFold(strings.TrimPrefix(k, "§"))
		return nil
	}
	if cmd, ok := m.openRoomFor(m.selected()); ok {
		return cmd // a room, or one of its agents: the room opens
	}
	if double {
		return m.attach(m.selected())
	}
	return m.loadPreview()
}

func (m *Model) loadPreview() tea.Cmd {
	a := m.focused()
	if a == nil || a.TranscriptPath == "" {
		return nil
	}
	e, known := m.previews[a.Key]
	key, path, had, kind := a.Key, a.TranscriptPath, e.size, a.Acct.Kind
	// Looked at in the background: even a stat can wait on a slow disk.
	return func() tea.Msg {
		st, err := os.Stat(path)
		if err != nil || known && had == st.Size() {
			return nil
		}
		return previewMsg{key: key, e: previewEntry{p: agent.ReadPreview(kind, path, 384<<10), size: st.Size()}}
	}
}

// loadLivePreviews keeps the transcript tail of every working agent fresh, so
// rows can say what each one is doing right now. What has grown is found,
// and up to 8 read, in the background.
func (m *Model) loadLivePreviews() tea.Cmd {
	type live struct {
		key, path string
		kind      agent.Kind
		had       int64
		known     bool
	}
	var ls []live
	for _, a := range m.snap.Agents {
		if a.Live() && a.TranscriptPath != "" {
			e, ok := m.previews[a.Key]
			ls = append(ls, live{a.Key, a.TranscriptPath, a.Acct.Kind, e.size, ok})
		}
	}
	if len(ls) == 0 {
		return nil
	}
	return func() tea.Msg {
		var out previewsMsg
		for _, l := range ls {
			if len(out) == 8 {
				break
			}
			st, err := os.Stat(l.path)
			if err != nil || l.known && l.had == st.Size() {
				continue
			}
			out = append(out, previewMsg{key: l.key, e: previewEntry{p: agent.ReadPreview(l.kind, l.path, 128<<10), size: st.Size()}})
		}
		if len(out) == 0 {
			return nil
		}
		return out
	}
}

// previewsMsg brings several previews read at once.
type previewsMsg []previewMsg

// markSeen acknowledges an agent's question, finished turn or error until
// it has something new.
func (m *Model) markSeen(a *fleet.Agent) {
	if a == nil || a.State != "blocked" && !a.Halted() && !a.YourTurn(m.snap.At) {
		return
	}
	m.store.Overlay.Seen[a.Key] = time.Now()
	_ = m.store.SaveOverlay()
}

func (m *Model) flash(s string, err bool) {
	m.status, m.statusErr, m.statusAt = convo.KeyWord(s), err, time.Now()
	if err {
		m.react(fxError)
	}
}

func (m *Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	defer uiBusy(msg)()
	if _, ok := msg.(fastMsg); ok {
		m.fastPending = false
		return m, m.fastTick()
	}
	if _, ok := msg.(relayoutMsg); ok {
		m.relayPending = false
		return m, m.relayout()
	}
	if m.onCellSize(msg) {
		return m, nil
	}
	if c := m.host; c != nil {
		c.scrollOnly = false // set again by a message that only scrolls
	}
	_, cmd := m.update(msg)
	m.pinHosted()
	m.applyJump()
	cmd = tea.Batch(cmd, m.interveneReady())
	_, isTick := msg.(tickMsg)
	m.noteProgress(isTick)
	var copyCmd tea.Cmd
	if m.pendingCopy != "" {
		copyCmd, m.pendingCopy = setClipboard(m.pendingCopy), ""
	}
	var fxCmd tea.Cmd
	if m.fxKick {
		fxCmd, m.fxKick = fxTick(), false
	}
	var paneCmd tea.Cmd
	if c := m.host; c != nil && c.paneKick && !c.paneReading {
		c.paneKick, paneCmd = false, m.refreshSubs()
	}
	return m, tea.Batch(cmd, copyCmd, fxCmd, paneCmd, pictureCmds(msg), m.fastTick(), m.relayout(), m.syncLive(), m.syncHost(), m.syncWatch(), m.loadSnapCmd(), m.asks())
}

func (m *Model) update(msg tea.Msg) (tea.Model, tea.Cmd) {
	// Lines of text in one key press are a paste the terminal didn't
	// bracket: taken as a paste, they fold into a chip like any other.
	if k, ok := msg.(tea.KeyPressMsg); ok && len(k.Text) > 1 && strings.ContainsAny(k.Text, "\r\n") {
		msg = tea.PasteMsg{Content: k.Text}
	}
	if cmd, ok := m.barMsg(msg); ok {
		return m, cmd
	}
	if cmd, ok := m.listFilterMsg(msg); ok {
		return m, cmd
	}
	switch msg := msg.(type) {
	case liveOpenMsg:
		return m, m.onLiveOpen(msg)
	case liveMsg:
		return m, m.onLive(msg)
	case hostOpenMsg:
		return m, m.onHostOpen(msg)
	case hostLinesMsg:
		return m, m.onHostLines(msg)
	case growMsg:
		return m, m.onGrow(msg)
	case paneMsg:
		return m, m.onPane(msg)
	case wholeMsg:
		m.onWhole(msg)
		return m, nil
	case preMsg:
		return m, m.onPre(msg)
	case replayMsg:
		return m, m.onReplay(msg)
	case foldersMsg:
		m.onFolders(msg)
		return m, nil
	case subStatsMsg:
		return m, m.onSubStats(msg)
	case spawnFoundMsg:
		m.onSpawnFound(msg)
		return m, nil
	case tempMeasuredMsg:
		return m, m.onTempMeasured(msg)
	case tempMsg:
		m.onTemp(msg)
		return m, nil
	case worktreesMsg:
		m.onWorktrees(msg)
		return m, nil
	case tidiedMsg:
		m.onTidied(msg)
		return m, nil
	case removedMsg:
		m.onRemoved(msg)
		return m, nil
	case scratchClearedMsg:
		return m, m.onScratchCleared(msg)
	case squeezedMsg:
		m.onSqueezed(msg)
		return m, nil
	case cleanedMsg:
		m.onCleaned(msg)
		return m, nil
	case movedToRushMsg:
		// The old row is finished; the conversation carries on in rush mode.
		m.store.Overlay.Done[msg.from] = time.Now()
		// Messages still waiting for the old row go to the new session.
		var carry tea.Cmd
		if q := m.localQ[msg.from]; q != nil && len(q.items) > 0 {
			text, id := host.JoinQueue(q.items), msg.started.id
			delete(m.localQ, msg.from)
			carry = cmdErr("queued messages moved over", func() error {
				// The new host may still be coming up.
				var c *host.Client
				var err error
				for range 30 {
					if c, err = host.Dial(id); err == nil {
						break
					}
					time.Sleep(100 * time.Millisecond)
				}
				if err != nil {
					return err
				}
				defer c.Close()
				return c.Send(text)
			})
		}
		if n := m.store.Overlay.Names[msg.from]; n != "" {
			m.store.Overlay.Names[state.Key(msg.started.acct, "a:"+msg.started.id)] = n
		}
		_ = m.store.SaveOverlay()
		if a := m.agentByKey(msg.from); a != nil && a.Interactive {
			defer m.flash(a.DisplayName+" carries on in rush mode · its terminal copy is still open there, now under Done", false)
		}
		mm, cmd := m.update(msg.started)
		return mm, tea.Batch(cmd, carry)
	case screenDoneMsg:
		return m, m.onScreenDone(msg)
	case sheetMsg:
		return m, msg.apply(m)
	case rewoundMsg:
		return m, m.onRewound(msg)
	case hostStartedMsg:
		if m.hosted != "" {
			// It's in Agents; the hosted view stays on its own session.
			m.flash("started "+msg.name+" · it's in rush's Agents", false)
			return m, nil
		}
		if old := m.agentByKey(msg.retire); old != nil && !old.Done {
			m.toggleDone(old)
		}
		// Select the new session, once a reading has it, and give it the keys.
		m.refresh()
		m.selectOnLoad = state.Key(msg.acct, "a:"+msg.id)
		m.preview, m.paneFocus = true, true
		m.flash("started "+msg.name, false)
		return m, nil
	case tea.WindowSizeMsg:
		m.w, m.h = msg.Width, msg.Height
		return m, askCell()
	case snapMsg:
		return m, m.onSnap(msg)
	case shellsMsg:
		m.onShells(msg)
		return m, nil
	case barsMsg:
		m.bars = statusline.Bars(msg)
		return m, nil
	case netMsg:
		return m, m.onNet()
	case peekCheckMsg:
		return m, m.peekCheck()
	case tickMsg:
		m.tick++
		m.loader.Settle() // nothing new on disk: the last reading, processes sampled again
		m.refresh()
		m.zenPick()
		m.emitHooks()
		cmds := []tea.Cmd{tick(), m.watchBinary(), m.recheckAfter(), m.refreshSpawns(), m.refreshFolders(), m.refreshSubs(), m.flushLocalQueues(), m.flushSubQueues(), m.watchOnline(), m.nudgeStuckSubs()}
		if m.hosted == "" {
			// autoSwitch too: a session's usage reading arrives with the
			// snapshot, not with a fetch.
			cmds = append(cmds, m.movePending(), m.tidy(), m.squeezeTranscripts(), m.autoSwitch())
		}
		if m.hosted == "" || m.mode == modeProjects {
			cmds = append(cmds, m.measureTemp())
		}
		if c := m.workTick(); c != nil {
			cmds = append(cmds, c) // the overview reads on while it's open
		}
		if c := m.sideTick(); c != nil {
			cmds = append(cmds, c)
		}
		if c := m.wallTick(); c != nil {
			cmds = append(cmds, c) // and the Wall's tiles
		}
		if m.mode == modeProjects && time.Since(m.clean.checked) > 2*time.Minute {
			cmds = append(cmds, m.scanWorktrees()) // looked at when Projects opens, and every 2 minutes while it's open
		} else if m.hosted == "" && m.tick > 120 && time.Since(m.clean.checked) > 6*time.Hour {
			cmds = append(cmds, m.scanWorktrees()) // and every 6 hours anyway, to say when there's a lot to clean up
		}
		if m.tick%3 == 0 {
			cmds = append(cmds, m.scan(), reloadConfig)
		}
		if m.tick%60 == 0 && m.hosted == "" {
			cmds = append(cmds, m.fetchUsage(), m.findLogins(), m.fetchQuotas())
		}
		m.clkBeat(m.mood(m.tally()))
		cmds = append(cmds, m.loadPreview())
		if m.tick%2 == 0 {
			cmds = append(cmds, m.loadLivePreviews())
		}
		return m, tea.Batch(cmds...)
	case onlineMsg:
		return m, m.onOnline(msg)
	case configMsg:
		m.onConfig(msg)
		return m, nil
	case workTimelinesMsg:
		m.onWorkTimelines(msg)
		return m, nil
	case wallReadMsg:
		m.onWallRead(msg)
		return m, nil
	case scanMsg:
		m.scanning = false
		m.loader.SetSpend(msg)
		if len(msg) > 0 || !m.loaded {
			m.loaded = true
			m.refresh()
		}
		return m, nil
	case dialogReload:
		if m.dialog != nil {
			m.loadDialog()
		}
		if m.host != nil {
			m.host.mem = nil
		}
		if msg.err != nil {
			m.flash(msg.err.Error(), true)
		}
		return m, nil
	case usageMsg:
		m.loader.SetFetched(msg.key, msg.u)
		m.refresh()
		return m, m.autoSwitch()
	case acctMsg:
		return m, msg.applyTo(m)
	case loginsMsg:
		return m, m.onLogins(msg)
	case switchedMsg:
		return m, m.onSwitched(msg)
	case addedLoginMsg:
		return m, m.onAddedLogin(msg)
	case fxTickMsg:
		return m, m.onFXTick()
	case kbFrameMsg:
		return m, m.onKbFrame()
	case tickerMsg:
		m.tickerPending = false
		if !m.tickerOn {
			m.sameFrame = true
			return m, nil
		}
		return m, m.tickerTick()
	case subHoverMsg:
		// Redraw only if the run rested on is still the one under the pointer.
		if c := m.host; c == nil || !strings.HasPrefix(c.subHover, "sub:") || time.Since(c.subHoverAt) < subPeekAfter {
			m.sameFrame = true
		}
		return m, nil
	case previewMsg:
		m.previews[msg.key] = msg.e
		return m, nil
	case previewsMsg:
		for _, p := range msg {
			m.previews[p.key] = p.e
		}
		return m, nil
	case darkScheme, lightScheme:
		return m, askColours
	case tea.FocusMsg:
		m.blurred = false
		// The terminal's profile may have changed while away: light or
		// dark following the system, or another picked.
		return m, askColours
	case tea.BackgroundColorMsg:
		c := theme.Of(msg.Color)
		m.termBG = &c
		m.applyColors()
		return m, nil
	case tea.ForegroundColorMsg:
		c := theme.Of(msg.Color)
		m.termFG = &c
		m.applyColors()
		return m, nil
	case tea.BlurMsg:
		m.blurred, m.hover = true, "" // the pointer has gone elsewhere
		return m, nil
	case editedMsg:
		switch {
		case msg.err != nil:
			m.flash("editor: "+msg.err.Error(), true)
		case msg.pane && m.host != nil:
			c := m.host
			c.input, c.back = applyEdit(&c.pastes, c.input, msg.id, msg.text), 0
		case !msg.pane:
			m.input, m.back = applyEdit(&m.pastes, m.input, msg.id, msg.text), 0
		}
		return m, nil
	case localQueueFailed:
		// Back on the front of the queue, to try again when you say.
		// Back on the front of the queue; tried again in 30s, and held
		// only once it has failed three times running.
		if q := m.localQ[msg.key]; q != nil {
			q.items = append(msg.items, q.items...)
			q.fails++
			q.retry = time.Now().Add(30 * time.Second)
			if q.fails >= 3 {
				q.held, q.fails = true, 0
				m.flash("couldn't send the queue three times: "+msg.err.Error()+" · held; alt+h releases it", true)
				return m, nil
			}
		}
		m.flash("couldn't send the queue: "+msg.err.Error()+" · trying again in 30s", true)
		return m, nil
	case sendFailedMsg:
		m.sendFailed(msg)
		m.refresh()
		return m, nil
	case updateMsg:
		m.newer = msg.newer
		m.flash("rush "+msg.newer.Short()+" is out · #update installs it", false)
		return m, nil
	case pluginPendingMsg:
		m.openPluginApproval(msg.pending)
		return m, nil
	case updatedMsg:
		m.updating = false
		if msg.err != nil {
			m.flash("update: "+msg.err.Error(), true)
		} else {
			m.newer = update.Info{}
			m.rebuiltSaid = true
			m.flash("rush "+msg.to.Short()+" installed · #reload runs it, sessions carry on", false)
		}
		return m, m.watchHosts(false)
	case shotMsg:
		if m.picker != nil && m.picker.img == msg.path && m.picker.big == msg.big {
			m.picker.shot = msg.rows
		}
		return m, nil
	case subSentMsg:
		m.onSubSent(msg)
		return m, nil
	case doneMsg:
		if msg.err != nil {
			m.flash(msg.err.Error(), true)
		} else if msg.text != "" {
			m.flash(msg.text, false)
		}
		m.refresh()
		return m, nil
	case movedMsg:
		// The old row keeps its history; it moves to Done and points at the new one.
		m.store.Overlay.Moved[msg.from.Key] = msg.to
		m.store.Overlay.Done[msg.from.Key] = time.Now()
		if n := m.store.Overlay.Names[msg.from.Key]; n != "" {
			m.store.Overlay.Names[msg.to] = n
		}
		_ = m.store.SaveOverlay()
		m.sel = msg.to
		m.flash("moved — now "+strings.TrimPrefix(msg.to, msg.from.Acct.Name+"/"), false)
		m.refresh()
		return m, nil
	case jobGoneMsg:
		a := m.agentByKey(msg.key)
		if a == nil {
			return m, nil
		}
		return m, m.moveToRushWith(a, msg.text)
	case attachDoneMsg:
		m.attached = ""
		m.refresh()
		if msg.err != nil && msg.agent != nil {
			if errors.Is(msg.err, agent.ErrElsewhere) {
				m.flash("opened in another window", false)
				return m, nil
			}
			a := msg.agent
			if errors.Is(msg.err, agent.ErrGone) {
				// Claude Code has let the job go; its conversation carries on here.
				return m, m.moveToRush(a)
			}
			at, ok := agent.As[agent.Attacher](agent.Kind(a.Kind))
			if !ok {
				m.flash("couldn't open "+a.DisplayName+": "+msg.err.Error(), true)
				return m, nil
			}
			// The command is made (its program looked for) before the
			// terminal is handed over, off the UI goroutine.
			acct, id := a.Acct, a.ID
			return m, func() tea.Msg {
				return tea.ExecProcess(at.Attach(acct, id), func(err error) tea.Msg {
					return doneMsg{err: err}
				})()
			}
		}
		return m, nil
	case clipImageMsg:
		switch {
		case msg.err != nil:
			m.flash("couldn't read the clipboard: "+msg.err.Error(), true)
		case msg.path == "":
			m.flash("no image on the clipboard", false)
		default:
			m.attachImages([]string{msg.path})
		}
		return m, nil
	case tea.PasteMsg:
		if s, ok := m.sheet.(*roomSetup); ok {
			s.paste(msg.Content)
			return m, nil
		}
		if s, ok := m.sheet.(*communitySheet); ok {
			s.paste(msg.Content)
			return m, nil
		}
		if s, ok := m.sheet.(*signInSheet); ok {
			s.paste(msg.Content)
			return m, nil
		}
		if !m.embedded {
			msg.Content = cleanPaste(msg.Content)
		}
		// Whether it names files is asked of the disk first, off the UI
		// goroutine; the paste comes back once that's known.
		if !m.pathsKnown(msg.Content) {
			return m, m.statPaste(msg)
		}
		// Files dropped onto the terminal go to the box they were dropped
		// on, not the one that has the keys.
		if m.ptrSeen && isDrop(msg.Content, m.lookPath) {
			m.focusAt(m.ptrX, m.ptrY)
		}
		if m.embedded {
			m.embedPaste(msg.Content)
			return m, nil
		}
		if m.editingDoc() {
			m.host.memEd.insertText(msg.Content)
			return m, nil
		}
		if p := m.picker; p != nil && p.dirs != nil {
			p.query, p.cursor = append(p.query, []rune(oneLine(msg.Content))...), 0
			return m, nil
		}
		// A paste with no text is what some terminals send when the
		// clipboard holds only an image: read the image itself.
		if strings.TrimSpace(msg.Content) == "" {
			return m, pasteClipImage()
		}
		// Image files dropped onto the terminal arrive as a paste of their
		// paths. In either box each becomes [Image #N] where it was dropped.
		if c := m.host; c != nil && m.paneFocus && m.dialog == nil {
			if t, ok := c.imgs.inline(msg.Content, m.lookPath); ok {
				msg.Content = t
			}
		} else if m.dialog == nil {
			if t, ok := m.imgs.inline(msg.Content, m.lookPath); ok {
				msg.Content = t
			}
		}
		if t := m.quick.thread(); m.quick.focused() {
			r := []rune(oneLine(msg.Content))
			t.input, t.pos = insert(t.input, t.pos, r), t.pos+len(r)
			return m, nil
		}
		// A paste goes into whichever box has focus, at its cursor, newlines
		// kept so a pasted log or snippet arrives whole.
		// A long one shows as a chip and goes out whole.
		if c := m.host; c != nil && m.paneFocus {
			text := msg.Content
			if isLongPaste(text) {
				text = c.pastes.add(text)
			}
			pos := max(0, len(c.input)-c.back)
			c.undo.save(c.input, c.back, false)
			c.input = insert(c.input, pos, []rune(text))
			return m, nil
		}
		if m.acceptsText() {
			if m.dialog != nil {
				m.dialog.input = append(m.dialog.input, []rune(oneLine(msg.Content))...)
			} else {
				text := oneLine(msg.Content)
				if isLongPaste(msg.Content) {
					text = m.pastes.add(msg.Content)
				}
				m.input = insert(m.input, m.cursorPos(), []rune(text))
			}
		}
		return m, nil
	case tea.KeyboardEnhancementsMsg:
		m.keysDisambiguated = msg.SupportsKeyDisambiguation()
		return m, nil
	case tea.KeyPressMsg:
		if hot := m.chipHot; hot.box != 0 {
			m.chipHot = chipHover{} // lit again only once the pointer moves
			if msg.String() == "space" && m.boxesTakeKeys() && m.openChip(hot) {
				return m, nil
			}
		}
		cmd := m.key(macOption(msg))
		m.emitInput()
		return m, cmd
	case hooks.StateMsg, hooks.DoMsg:
		return m, m.onHooks(msg)
	case interceptedMsg:
		return m, m.onIntercepted(msg)
	case tea.MouseMotionMsg:
		wasOver := m.ptrSeen && m.ptrX > m.listW+1
		wasList := m.ptrSeen && m.ptrX < m.listW
		m.ptrX, m.ptrY, m.ptrSeen = msg.X, msg.Y, true
		// An open sheet has the mouse, as it has the keys.
		if m.sheet != nil {
			if msg.Button != tea.MouseLeft {
				m.sameFrame = true
				return m, m.pointerShape("default")
			}
			return m, tea.Batch(m.sheetMouse(mouseDrag, msg.X, msg.Y), m.pointerShape("grabbing"))
		}
		if m.grid.drag {
			m.gridDrag(msg.X, msg.Y)
			return m, m.pointerShape("grabbing")
		}
		if c := m.host; c != nil && c.minimap.dragging {
			if msg.Button == tea.MouseLeft {
				m.dragMinimap(c, msg.Y)
				return m, m.pointerShape("grabbing")
			}
			c.minimap.dragging = false
		}
		if t := m.btwDragging(); t != nil {
			if msg.Button == tea.MouseLeft {
				t.drag(msg.X, msg.Y)
				return m, nil
			}
			m.endBtwDrag(t)
		}
		if c := m.host; c != nil && c.pullY > 0 {
			if msg.Button == tea.MouseLeft {
				if c.pullY-1-msg.Y >= 3 {
					m.openStack(c, m.stackFromY(c, c.pullY-1))
				}
				return m, nil
			}
			c.pullY = 0
		}
		if c := m.host; c != nil && c.txt.drag {
			if msg.Button == tea.MouseLeft {
				m.dragTextSel(c, msg.X, msg.Y)
				return m, nil
			}
			if cmd := m.endTextSel(c); cmd != nil {
				return m, cmd
			}
		}
		if m.boxDrag != 0 {
			if msg.Button == tea.MouseLeft {
				m.dragBox(msg.X, msg.Y)
				return m, nil
			}
			m.endBoxDrag()
		}
		if m.dragging {
			if msg.Button == tea.MouseLeft {
				m.dragSplit(msg.X)
				return m, nil
			}
			m.dragging = false
			_ = m.store.SaveConfig()
		}
		on := m.listW > 0 && m.mode == modeList && (msg.X == m.listW || msg.X == m.listW+1)
		hover := m.hover
		changed := on != m.divHover
		changed = m.hoverChip(msg.X, msg.Y) || changed
		changed = m.hoverPane(msg.X, msg.Y) || changed
		changed = m.hoverHeader(msg.X, msg.Y) || changed
		if m.dialog != nil && m.dialog.page == pageKeys && m.sheet == nil {
			changed = m.keysHover(msg.Y) || changed
		}
		if c := m.host; c != nil {
			y := msg.Y
			if msg.X <= m.listW+1 {
				y = -1 // over the list, not the queue
			}
			changed = c.queueHover(y) || changed
		}
		// The pointer coming onto the Session or the list gives it the keys,
		// once as it crosses: tab to the other holds while the pointer stays put.
		if focused := m.paneFocus; msg.Button == tea.MouseNone && (!wasOver && msg.X > m.listW+1 || !wasList && msg.X < m.listW) {
			m.focusAt(msg.X, msg.Y)
			changed = changed || m.paneFocus != focused
		}
		m.divHover = on
		cmd := m.mouseMove(msg.X, msg.Y)
		subChanged, subCmd := m.subMouseMove(msg.X, msg.Y)
		if !changed && !subChanged && m.hover == hover {
			m.sameFrame = true // nothing moved that shows: keep the last frame
		}
		want := "default"
		switch {
		case m.host != nil && m.host.minimap.hit(msg.X, msg.Y):
			want = "pointer"
		case on:
			want = "ew-resize"
		case m.topHover.valid || m.hover != "" || m.host != nil && m.host.pointerHover != (paneHover{}):
			want = "pointer"
		case m.host != nil && m.host.subHover != "", m.chipHot.box != 0:
			want = "pointer" // a run or a paste to open, or the banner to go back
		}
		return m, tea.Batch(m.pointerShape(want), cmd, subCmd)
	case tea.MouseReleaseMsg:
		if m.sheet != nil {
			return m, tea.Batch(m.sheetMouse(mouseRelease, msg.X, msg.Y), m.pointerShape("default"))
		}
		var cmd tea.Cmd
		if c := m.host; c != nil {
			c.pullY = 0
		}
		if c := m.host; c != nil && c.minimap.dragging {
			m.dragMinimap(c, msg.Y)
			c.minimap.dragging = false
			return m, m.pointerShape("default")
		}
		if m.grid.drag {
			m.gridDrag(msg.X, msg.Y)
			m.grid.drag = false
			m.gridSave()
			return m, m.pointerShape("default")
		}
		if t := m.btwDragging(); t != nil {
			m.endBtwDrag(t)
		}
		if c := m.host; c != nil && c.txt.drag {
			cmd = m.endTextSel(c)
		}
		if m.boxDrag != 0 {
			m.endBoxDrag()
		}
		if m.dragging {
			m.dragging = false
			_ = m.store.SaveConfig()
		}
		return m, cmd
	case tea.MouseClickMsg:
		m.ptrX, m.ptrY, m.ptrSeen = msg.X, msg.Y, true
		if msg.Button == tea.MouseLeft {
			at := [2]int{msg.X, msg.Y}
			m.dbl = at == m.pressXY && time.Since(m.pressAt) < 400*time.Millisecond
			m.pressAt, m.pressXY = time.Now(), at
			if m.bar == nil && m.confirm == nil && m.clickQuick(msg.X, msg.Y) {
				return m, nil // the quick ask floats over all but those
			}
			if m.bar == nil && m.confirm == nil && m.clickSide(msg.X, msg.Y) {
				return m, nil
			}
			if cmd, ok := m.overlayClick(msg.X, msg.Y); ok {
				return m, cmd // a sheet, question or menu: see overlayclick.go
			}
		}
		if m.sheet != nil {
			if msg.Button == tea.MouseLeft {
				return m, m.sheetMouse(mousePress, msg.X, msg.Y)
			}
			return m, nil
		}
		if m.dialog != nil {
			if msg.Button == tea.MouseLeft {
				if cmd, ok := m.clickTab(msg.X, msg.Y); ok {
					return m, cmd
				}
				return m, m.settingsMouse(msg.X, msg.Y)
			}
			return m, nil
		}
		if msg.Button == tea.MouseLeft && m.mode == modeList {
			if cmd, ok := m.gridClick(msg.X, msg.Y); ok {
				return m, cmd
			}
		}
		if msg.Button == tea.MouseLeft {
			if cmd := m.draftImage(msg.X, msg.Y); m.sheet != nil {
				return m, cmd
			}
		}
		if msg.Button == tea.MouseLeft && m.clickBox(msg.X, msg.Y) {
			return m, nil
		}
		if msg.Button == tea.MouseLeft {
			if cmd, ok := m.clickTab(msg.X, msg.Y); ok {
				return m, cmd
			}
		}
		// Grabbing the edge between list and pane resizes the list.
		if msg.Button == tea.MouseLeft && m.listW > 0 && m.mode == modeList && (msg.X == m.listW || msg.X == m.listW+1) {
			m.dragging = true
			return m, nil
		}
		if msg.Button == tea.MouseLeft && m.host != nil && (m.listW == 0 || msg.X > m.listW+1) && m.mode == modeList && m.dialog == nil {
			m.paneFocus = true // clicking the Session gives it the keys
			if m.viewName(m.host) == "screen" && m.canEmbed() {
				m.embedded = true
			}
			m.host.txt.on = false // a click elsewhere drops what was dragged over
			if m.clickDockSide(m.host, msg.X, msg.Y) {
				return m, nil
			}
			if m.contextAt(m.host, msg.X, msg.Y) {
				return m, m.openInfo(m.host, infoContext)
			}
			if m.clickPaneTab(m.host, msg.X, msg.Y) {
				return m, nil
			}
			if m.clickLabel(m.host, msg.X, msg.Y) {
				return m, m.openSwitchSheet(m.host)
			}
			if cmd, ok := m.clickCard(m.host, msg.X, msg.Y); ok {
				return m, cmd
			}
			if m.clickBtw(m.host, msg.X, msg.Y) {
				return m, nil
			}
			if m.clickTasks(m.host, msg.X, msg.Y) {
				return m, nil
			}
			if m.stripClick(m.host, msg.X, msg.Y) {
				return m, nil
			}
			if m.pressMinimap(m.host, msg.X, msg.Y) {
				return m, m.pointerShape("grabbing")
			}
			if m.clickStack(m.host, msg.Y) {
				return m, nil
			}
			if m.onSubsTitle(m.host, msg.Y) {
				m.host.pullY = msg.Y + 1 // dragged up, it opens the stack
				return m, nil
			}
			if !m.embedded {
				if cmd, ok := m.clickMessage(m.host, msg.X, msg.Y); ok {
					return m, cmd
				}
			}
			if m.subPreviewAt(m.host, msg.X, msg.Y) {
				m.openSub(m.host, m.host.subPreview.id)
				m.host.scrollOnly = false
				return m, nil
			}
			if !m.embedded && m.startTextSel(m.host, msg.X, msg.Y) {
				return m, nil // a click on the text is one once it's released
			}
			m.clickRow(m.host, msg.X, msg.Y)
			return m, nil
		}
		// Right-clicking a link in the Session asks what to do with it:
		// open it, in Preview, Quick Look, reveal or copy it.
		if msg.Button == tea.MouseRight && m.host != nil && (m.listW == 0 || msg.X > m.listW+1) && m.mode == modeList && m.dialog == nil && m.picker == nil && !m.embedded {
			m.linkMenu(m.host, msg.X, msg.Y)
			return m, m.drawShot()
		}
		if msg.Button == tea.MouseLeft {
			m.paneFocus, m.embedded = false, false // clicking Agents takes the keys back
			return m, m.mouseClick(msg.X, msg.Y)
		}
	case tea.MouseWheelMsg:
		if cmd, ok := m.overlayWheel(msg.Button == tea.MouseWheelUp); ok {
			return m, cmd
		}
		if m.sheet != nil {
			ev := mouseWheelDown
			if msg.Button == tea.MouseWheelUp {
				ev = mouseWheelUp
			}
			return m, m.sheetMouse(ev, msg.X, msg.Y)
		}
		if m.dialog != nil {
			delta := 3
			if msg.Button == tea.MouseWheelUp {
				delta = -3
			}
			return m, m.settingsWheel(msg.X, msg.Y, delta)
		}
		// The wheel scrolls whatever is under the pointer: over the pane it
		// scrolls the conversation, and never moves the list behind it.
		if _, paneW, _ := m.layout(); m.mode == modeList && m.dialog == nil && paneW > 0 && (m.listW == 0 || msg.X > m.listW) {
			if c := m.host; c != nil {
				if m.subPreviewAt(c, msg.X, msg.Y) {
					delta := -3
					if msg.Button == tea.MouseWheelUp {
						delta = 3
					}
					p := &c.subPreview
					p.scroll = max(0, min(p.scroll+delta, max(0, p.total-p.rows)))
					c.scrollOnly = true
					return m, nil
				}
				if msg.Button == tea.MouseWheelUp && m.onSubsTitle(c, msg.Y) {
					m.openStack(c, m.stackFromY(c, msg.Y))
					return m, nil
				}
				switch msg.Button {
				case tea.MouseWheelUp:
					c.scroll += 3
				case tea.MouseWheelDown:
					c.scroll = max(0, c.scroll-3)
				}
				c.scrollOnly = true
			}
			return m, nil
		}
		if m.mode == modeList && m.dialog == nil {
			switch msg.Button {
			case tea.MouseWheelUp:
				m.move(-3)
			case tea.MouseWheelDown:
				m.move(3)
			}
			return m, m.loadPreview()
		}
	}
	return m, nil
}

// pointerShape asks the terminal for the pointer's shape, when it's
// changed. OSC 22 sets it in terminals that support it (kitty, ghostty,
// wezterm, foot); others ignore it.
func (m *Model) pointerShape(want string) tea.Cmd {
	if want == m.pointer || (want == "default" && m.pointer == "") {
		return nil
	}
	m.pointer = want
	return tea.Raw("\x1b]22;" + want + "\x1b\\")
}

// viewNames are the places at the top: ctrl+\ moves between them, and tab
// moves within one (the list and its Session, or a place's pages).
var viewNames = []string{"Agents", "Projects", "Harnesses", "Settings"}

// The places, in viewNames' order.
const (
	placeAgents = iota
	placeProjects
	placeHarnesses
	placeSettings
)

// setView switches the whole screen to a place, on the page it was last on.
func (m *Model) setView(v int) {
	m.view = (v + len(viewNames)) % len(viewNames)
	m.dialog, m.mode, m.picker, m.sheet = nil, modeList, nil, nil
	m.input, m.inKind = m.input[:0], inPrompt
	m.zen = false
	switch m.view {
	case placeAgents:
		// esc from the Wall lands here, on the plain list, never back on
		// the page it came from.
		m.work.page = agentsList
	case placeProjects:
		m.mode = modeProjects
	case placeHarnesses:
		m.openDialog(m.harnessPage)
	case placeSettings:
		m.openDialog(max(m.settingsPage, pageGeneral))
	}
}

// placePages are the dialog's pages a place shows: Harnesses the agents'
// set-up, Settings the rest.
func placePages(v int) (lo, hi int) {
	if v == placeHarnesses {
		return pageProviders, pageGeneral
	}
	return pageGeneral, pageUpdates + 1
}

// setSettingsPage shows one of the dialog's pages, in the place it's on.
func (m *Model) setSettingsPage(p int) {
	v := placeSettings
	if p < pageGeneral {
		v = placeHarnesses
	}
	if m.view != v {
		m.setView(v)
	}
	if v == placeHarnesses {
		m.harnessPage = p
	} else {
		m.settingsPage = p
	}
	m.openDialog(p)
}

// stepSettingsPage is [ and ]: the next page of the place's, round.
func (m *Model) stepSettingsPage(d int) {
	lo, hi := placePages(m.view)
	m.setSettingsPage(lo + (m.dialog.page-lo+d+hi-lo)%(hi-lo))
}

// setZen turns Zen on or off. Zen is Agents with only the agent that needs
// you on screen, the oldest first; the list comes back when it's off.
func (m *Model) setZen(on bool) {
	if on && m.view != placeAgents {
		m.setView(placeAgents)
	}
	m.zen, m.peek = on, zenPeek{}
	defer m.rebuild()
	if !on {
		return
	}
	m.embedded, m.full, m.paneFocus = false, false, true
	if q := m.zenQueue(); len(q) > 0 {
		m.sel = q[0].Key
	}
}

// zenFull is Zen showing only the agent, the whole screen.
func (m *Model) zenFull() bool { return m.zen }

// wide is when the preview gets its own half of the screen.
func (m *Model) wide() bool { return m.w >= 170 }

// autoSplit is when the Session shows beside the list without being asked:
// a wide screen you haven't pushed to the list alone.
func (m *Model) autoSplit() bool { return m.wide() && !m.store.Config.ListOnly }

// focusAt gives the keys to the box at (x, y), as a click there would: the
// Session's or the Agents'. Anywhere else leaves them where they are.
func (m *Model) focusAt(x, y int) {
	if m.host == nil || m.mode != modeList || m.dialog != nil || m.sheet != nil || m.listW == 0 {
		return
	}
	switch {
	case x > m.listW+1 && !m.paneFocus:
		m.paneFocus = true
	case x < m.listW && (m.paneFocus || m.embedded):
		m.paneFocus, m.embedded = false, false
	}
}

func (m *Model) acceptsText() bool {
	return m.confirm == nil && m.sheet == nil && (m.dialog == nil || m.dialog.asking != "") && m.mode == modeList
}

// notify posts a notification when an agent starts waiting on the user,
// and has clanker react to that and to agents answered, finished or failing.
func (m *Model) notify() {
	first := len(m.lastState) == 0
	if m.lastErr == nil {
		m.lastErr = map[string]bool{}
	}
	for _, a := range m.snap.Agents {
		if a.Checking {
			continue
		}
		prev := m.lastState[a.Key]
		m.lastState[a.Key] = a.State
		failing := strings.HasPrefix(a.Detail, "API error") || strings.HasPrefix(a.Detail, "usage limit") || a.Halted()
		wasFailing := m.lastErr[a.Key]
		m.lastErr[a.Key] = failing
		if !first && prev != "" {
			switch {
			case failing && !wasFailing:
				m.react(fxError)
				m.noteEvent(a, cRed, "✗", oneLine(a.HaltReason()))
			case prev != "blocked" && a.NeedsYou():
				m.react(fxAsk)
				m.noteEvent(a, cOrange, "?", oneLine(a.Needs))
			case prev == "blocked" && (a.State == "working" || a.State == "running"):
				m.react(fxAnswered)
			case (prev == "working" || prev == "running") && a.State == "done":
				m.react(fxDone)
				m.noteEvent(a, cGreen, "✓", "finished")
			}
		}
		// Not for the agent you're looking at while rush has focus.
		watching := !m.blurred && m.paneFocus && m.host != nil && m.host.key == a.Key
		// The menu bar icon, when it runs, notifies instead, with buttons.
		if !first && !m.store.Config.Quiet && prev != "" && prev != "blocked" && a.NeedsYou() && !watching {
			body := a.Needs
			if body == "" {
				body = oneLine(a.Detail)
			}
			notifyLater(a.DisplayName+" needs you", body, true)
		}
		// A turn that died on an error goes nowhere until someone says so;
		// the menu bar only knows about questions, so this is said here.
		if !first && !m.store.Config.Quiet && prev != "" && a.Halted() && !wasFailing && !watching {
			notifyLater(a.DisplayName+" stopped", a.HaltReason(), false)
		}
	}
}

// notifyLater posts a notification from a goroutine of its own: posting
// runs a program, and whether the menu bar runs is a look at the disk.
// With unlessMenuBar, the menu bar posts it instead when it runs.
func notifyLater(title, body string, unlessMenuBar bool) {
	go func() {
		if unlessMenuBar && menubar.Running() {
			return
		}
		actions.Notify(title, body)
	}()
}

// hibernate stops finished agents whose process is still resident.
func (m *Model) hibernate() {
	after := m.store.Config.Hibernate.AfterMinutes
	if after <= 0 {
		return
	}
	for _, a := range m.snap.Agents {
		if a.Worker != nil && a.State == "done" && a.Age(m.snap.At) > time.Duration(after)*time.Minute && !m.hibernated[a.Key] {
			m.hibernated[a.Key] = true // one try each; a failed stop is not retried every second
			go func() { _ = stopOutside(a) }()
		}
	}
}

func (m *Model) agentByKey(key string) *fleet.Agent {
	for _, a := range m.snap.Agents {
		if a.Key == key {
			return a
		}
	}
	return nil
}

func (m *Model) selected() *fleet.Agent {
	for _, a := range m.order {
		if a.Key == m.sel {
			return a
		}
	}
	return nil
}

func (m *Model) move(d int) {
	items := m.items()
	if len(items) == 0 {
		return
	}
	i := 0
	for j, k := range items {
		if k == m.sel {
			i = j
		}
	}
	i = min(max(i+d, 0), len(items)-1)
	if m.sel != "" && !strings.HasPrefix(m.sel, "§") {
		m.shown = m.sel
	}
	m.sel = items[i]
	m.hover = ""
}

// items are the rows ↑↓ stop on, in display order: the agents and every
// section heading, where enter folds or opens the section.
func (m *Model) items() []string {
	var out []string
	for _, l := range m.lines {
		switch l.kind {
		case lineSection:
			out = append(out, sectionKey(l.title))
		case lineAgent:
			out = append(out, l.agent.Key)
		}
	}
	return out
}

func (m *Model) folded(title string) bool {
	if v, ok := m.store.Config.Folds[title]; ok {
		return v
	}
	return title == "Earlier" || title == "Scratch" || title == otherSection && m.activeSidebar() != nil
}

func (m *Model) toggleFold(title string) {
	if m.store.Config.Folds == nil {
		m.store.Config.Folds = map[string]bool{}
	}
	m.store.Config.Folds[title] = !m.folded(title)
	_ = m.store.SaveConfig()
	m.rebuild()
}

// typingIn is the selected agent's key while a draft to it is in a box, its
// Session's or the Prompt replying to it; rebuild keeps its row in place
// till the draft's sent or cleared.
func (m *Model) typingIn() string {
	if c := m.host; c != nil && c.key == m.sel && len(c.input) > 0 || m.inKind == inReply && len(m.input) > 0 {
		return m.sel
	}
	return ""
}

// focused is the agent whose card is open: the selection, or on a section
// heading the agent picked before it, so the Session beside the list
// doesn't come and go as ↑↓ pass a heading.
func (m *Model) focused() *fleet.Agent {
	key := m.sel
	if strings.HasPrefix(key, "§") {
		key = m.shown
	}
	for _, a := range m.order {
		if a.Key == key {
			m.shown = a.Key
			return a
		}
	}
	return nil
}

// What's in play splits by what it wants from you: Needs you (a question,
// a finished turn, a halt), Stuck? (at work but silent for
// fleet.StuckAfter), Working, then Idle (open, or stopped within
// activeFor).
const (
	needsSection   = "Needs you"
	stuckSection   = "Stuck?"
	workingSection = "Working"
	idleSection    = "Idle"
	// justLeftSection is the way out: what left Idle, most recent first,
	// for justLeftFor and up to justLeftCount, so one you missed is still
	// where you'd look.
	justLeftSection = "Just left"
)

// inPlay is whether section title holds agents in play, not history.
func inPlay(title string) bool {
	switch title {
	case needsSection, stuckSection, workingSection, idleSection:
		return true
	}
	return false
}

// byTime is whether section title lists what finished purely by when, the
// most recent first, its project a tag rather than a heading.
func byTime(title string) bool {
	switch title {
	case justLeftSection, "Today", "Done", "Earlier":
		return true
	}
	return false
}

// activeFor is how long a stopped agent stays in Active before Today has
// it: Settings › General, 5 minutes unless set.
func (m *Model) activeFor() time.Duration {
	switch n := m.store.Config.ActiveMinutes; {
	case n < 0:
		return 0
	case n == 0:
		return 5 * time.Minute
	default:
		return time.Duration(n) * time.Minute
	}
}

// justLeftFor is how long an agent stays in Just left after its last
// activity: Settings › General, 2 hours unless set; off, none do.
func (m *Model) justLeftFor() time.Duration {
	switch n := m.store.Config.JustLeftMinutes; {
	case n < 0:
		return 0
	case n == 0:
		return 2 * time.Hour
	default:
		return time.Duration(n) * time.Minute
	}
}

// justLeftCount is how many Just left holds at most: 10 unless set.
func (m *Model) justLeftCount() int {
	return cmp.Or(m.store.Config.JustLeftCount, 10)
}

// rebuild groups the agents for the current group-by mode. What needs the
// user comes first; anything finished more than a day ago goes to Earlier.
func (m *Model) rebuild() {
	by := m.store.Config.GroupBy
	now := m.snap.At
	sb := m.activeSidebar()
	m.nameAgents(sb)
	typing, hold := m.typingIn(), m.hold
	if hold.key != typing {
		hold.key = "" // a new draft, or none: nothing held yet
	}
	m.hold.key = ""
	type group struct {
		name   string
		agents []*fleet.Agent
		rank   int
		recent time.Time
	}
	order := map[*fleet.Agent]int{} // places under a plugin's arrangement
	var left []*fleet.Agent         // Just left, before its count is applied
	groups := map[string]*group{}
	add := func(name string, rank int, a *fleet.Agent) {
		g := groups[name]
		if g == nil {
			g = &group{name: name, rank: rank}
			groups[name] = g
		}
		g.agents = append(g.agents, a)
		if a.UpdatedAt.After(g.recent) {
			g.recent = a.UpdatedAt
		}
	}
	for _, a := range m.roomize(m.snap.Agents) { // a room is one row, its members under it
		if m.zen && !a.NeedsYou() && !a.Waiting() {
			continue // Zen's list is only the agents waiting on you
		}
		if f := m.listFilter; f != nil && len(f.query) > 0 && !m.listFilterMatch(a) {
			continue // alt+f: only what's typed matches, by name or what was said
		}
		if a.Key == hold.key {
			add(hold.section, hold.rank, a) // a row doesn't leave from under a draft
			continue
		}
		if sb != nil {
			// The plugin's sections replace rush's; each row still shows
			// its agent's state.
			name, rank, at := sidebarPlace(sb, a)
			order[a] = at
			add(name, rank, a)
			continue
		}
		fresh := a.Open() || a.Busy() || a.Pinned || a.Age(now) < 24*time.Hour
		switch {
		case a.NeedsYou(), a.Halted(), a.Waiting():
			add(needsSection, 1, a) // a question, or an error it won't go on from by itself
		case folderKey(a) == scratchSection:
			add("Scratch", 10, a) // temp-folder runs finish on their own, not on your turn; folded
		case a.YourTurn(now), a.JustFinished(now) && !a.Seen && !a.Checking:
			add(needsSection, 1, a) // finished without asking; often wants "keep going"
		case a.Stuck(now):
			add(stuckSection, 2, a)
		case a.Checking, a.Live(), a.Busy():
			add(workingSection, 3, a)
		case a.Pinned:
			add("Pinned", 5, a)
		case a.PID != 0, !a.Done && a.Age(now) < m.activeFor():
			add(idleSection, 4, a) // open, or stopped a moment ago: in play a while before Today has it
		case !a.Done && fresh && a.Age(now) < m.justLeftFor():
			left = append(left, a) // left Idle a while ago: kept in sight, not gone
		case !fresh:
			add("Earlier", 9, a)
		case a.Done:
			add("Done", 8, a)
		case by == "agent":
			add(agentName(a.Kind), 5, a)
		case by == "group" && a.Group != "":
			add(a.Group, 5, a)
		default:
			add("Today", 7, a)
		}
	}
	// The most recent stay in Just left, up to its count; the rest go on to Today.
	slices.SortStableFunc(left, func(a, b *fleet.Agent) int { return cmp.Compare(a.Age(now), b.Age(now)) })
	for i, a := range left {
		if i < m.justLeftCount() {
			add(justLeftSection, 5, a)
		} else {
			add("Today", 7, a)
		}
	}
	list := make([]*group, 0, len(groups))
	for _, g := range groups {
		list = append(list, g)
	}
	// Split by project, each section's rows sit together by the
	// repository they work in, its worktrees' after its own.
	split := sb == nil && m.splitProjects()
	var titles map[string]string
	if split {
		keys := map[string]bool{}
		for _, g := range list {
			for _, a := range g.agents {
				keys[folderKey(a)] = true
			}
		}
		titles = folderTitles(keys)
	}
	var atWork []*fleet.Agent
	for _, n := range []string{workingSection, stuckSection} {
		if g := groups[n]; g != nil {
			atWork = append(atWork, g.agents...)
		}
	}
	m.noteRuns(now, atWork)
	sort.Slice(list, func(i, j int) bool {
		if list[i].rank != list[j].rank {
			return list[i].rank < list[j].rank
		}
		return list[i].recent.After(list[j].recent)
	})
	for _, g := range list {
		less := m.sortLess
		switch {
		case sb != nil && g.rank < len(sb.Sections):
			less = func(a, b *fleet.Agent) bool {
				if order[a] != order[b] {
					return order[a] < order[b]
				}
				return m.sortLess(a, b)
			}
		case sb == nil && g.name == "Done":
			less = m.doneLess
		case sb == nil && (g.name == needsSection || g.name == stuckSection):
			less = waitedLess(now) // what you'd look at first: a question, then the longest waiting or silent
		case sb == nil && g.name == workingSection:
			less = m.runLess(now) // the longest at it this run first
		case sb == nil:
			less = stoppedLess(now) // the one that stopped last first
		}
		if split && !byTime(g.name) { // what finished goes by time, its project a tag
			within := less
			less = func(a, b *fleet.Agent) bool {
				if c := cmpLower(titles[folderKey(a)], titles[folderKey(b)]); c != 0 {
					return c < 0
				}
				if c := cmp.Compare(treeOf(a), treeOf(b)); c != 0 {
					return c < 0
				}
				return within(a, b)
			}
		}
		sort.SliceStable(g.agents, func(i, j int) bool { return less(g.agents[i], g.agents[j]) })
		if i := slices.IndexFunc(g.agents, func(a *fleet.Agent) bool { return a.Key == hold.key }); i >= 0 {
			a := g.agents[i]
			rest := slices.Delete(g.agents, i, i+1)
			lo, hi := 0, len(rest)
			if split && !byTime(g.name) { // within its project's rows, or it'd sit under another's heading
				peer := func(b *fleet.Agent) bool { return folderKey(b) == folderKey(a) && treeOf(b) == treeOf(a) }
				if lo = slices.IndexFunc(rest, peer); lo < 0 {
					lo, hi = i, i
				} else {
					for hi = lo; hi < len(rest) && peer(rest[hi]); hi++ {
					}
				}
			}
			g.agents = slices.Insert(rest, min(max(hold.at, lo), hi), a)
		}
	}
	m.order = m.order[:0]
	m.lines = m.lines[:0]
	clear(m.groupOf)
	if m.groupOf == nil {
		m.groupOf = map[string]string{}
	}
	for _, g := range list {
		var cost float64
		var names []string
		for _, a := range g.agents {
			cost += a.Spend.Cost
			if len(names) < 4 {
				names = append(names, oneLine(a.DisplayName))
			}
		}
		fold := m.folded(g.name)
		meta := sectionMeta(len(g.agents), cost)
		// One extra figure at most, and only one you can act on: temp work
		// where /clean all reaches it, memory where agents rest.
		switch name := g.name; {
		case sb != nil:
		case name == "Done", name == "Earlier":
			var temp int64
			for _, a := range g.agents {
				if a.PID == 0 {
					temp += a.Temp
				}
			}
			if temp >= tempShown {
				meta += " · " + disk(temp) + " tmp"
			}
		case name == idleSection:
			var held uint64
			for _, a := range g.agents {
				if !a.Live() && !a.Busy() {
					held += a.Mem
				}
			}
			if held > 0 {
				meta += " · " + mem(held) + " idle ram" //nolint:rush // once per section, meta is fresh each time
			}
		}
		m.lines = append(m.lines, listLine{kind: lineSection, title: g.name, meta: meta,
			folded: fold, peek: strings.Join(names, ", ")})
		project, tree := "\x00", ""
		split := split && !byTime(g.name)
		for i, a := range g.agents {
			if a.Key == typing {
				m.hold.key, m.hold.section, m.hold.rank, m.hold.at = a.Key, g.name, g.rank, i
			}
			m.order = append(m.order, a)
			m.groupOf[a.Key] = g.name
			if fold {
				continue
			}
			if split {
				if p := folderKey(a); p != project {
					project, tree = p, ""
					m.lines = append(m.lines, listLine{kind: lineProject, title: titles[p], root: p})
				}
				if t := treeOf(a); t != tree {
					tree = t
					m.lines = append(m.lines, listLine{kind: lineTree, title: project, root: t})
				}
			}
			inset := 0
			if split {
				inset = rowInset
				if tree != "" {
					inset = 2 * rowInset
				}
			}
			m.lines = append(m.lines, listLine{kind: lineAgent, agent: a, inset: inset})
			under := inset
			if !isRoomRow(a) {
				under += rowInset // a room opened from a chat, joined under it
			}
			for _, mb := range m.roomMembers(a) {
				m.order = append(m.order, mb)
				m.groupOf[mb.Key] = g.name
				m.lines = append(m.lines, listLine{kind: lineAgent, agent: mb, inset: under})
			}
		}
		if !fold { // folded history stacks tight
			m.lines = append(m.lines, listLine{kind: lineBlank})
		}
	}
	m.projTitles = titles
	m.applyRestore()
	valid := false
	for _, l := range m.lines {
		valid = valid || l.kind == lineSection && sectionKey(l.title) == m.sel || l.kind == lineAgent && l.agent.Key == m.sel
	}
	if !valid {
		if m.inKind == inReply && m.sel != "" {
			m.inKind = inPrompt
			m.flash("the agent you were replying to has gone", true)
		}
		m.sel = ""
		for _, k := range m.items() {
			if !strings.HasPrefix(k, "§") {
				m.sel = k
				break
			}
		}
		if items := m.items(); m.sel == "" && len(items) > 0 {
			m.sel = items[0]
		}
	}
}

var sortModes = []string{"name", "recent", "cost", "cpu", "ram", "tokens", "time"}

// sortLess orders rows inside a section. By name a row keeps its place while
// its agent works; the number columns sort biggest first.
func (m *Model) sortLess(a, b *fleet.Agent) bool {
	now := m.snap.At
	byName := func() bool {
		if c := cmpLower(a.DisplayName, b.DisplayName); c != 0 {
			return c < 0
		}
		return a.Key < b.Key
	}
	var x, y float64
	switch m.store.Config.SortBy {
	case "recent":
		x, y = float64(-a.Age(now)), float64(-b.Age(now))
	case "cost":
		x, y = a.Spend.Cost, b.Spend.Cost
	case "cpu":
		x, y = a.CPU, b.CPU
	case "ram":
		x, y = float64(a.Mem), float64(b.Mem)
	case "tokens":
		x, y = float64(a.Spend.Context), float64(b.Spend.Context)
	case "time":
		x, y = float64(a.Elapsed(now)), float64(b.Elapsed(now))
	default:
		return byName()
	}
	if x != y {
		return x > y
	}
	return byName()
}

// cmpLower compares strings as their strings.ToLower forms would compare,
// without making them: the list sorts by name every second.
func cmpLower(x, y string) int {
	for x != "" && y != "" {
		r, n := utf8.DecodeRuneInString(x)
		q, k := utf8.DecodeRuneInString(y)
		if r, q = unicode.ToLower(r), unicode.ToLower(q); r != q {
			return cmp.Compare(r, q)
		}
		x, y = x[n:], y[k:]
	}
	return cmp.Compare(len(x), len(y))
}

// waitedLess orders a question or halt first, then by how long each has
// been silent, longest first.
func waitedLess(now time.Time) func(a, b *fleet.Agent) bool {
	return func(a, b *fleet.Agent) bool {
		if x, y := asks(a), asks(b); x != y {
			return x
		}
		if x, y := a.Quiet(now), b.Quiet(now); x != y {
			return x > y
		}
		return a.Key < b.Key
	}
}

// stoppedLess orders agents that aren't at work: one asking you something
// first, then by when each stopped, the latest first.
func stoppedLess(now time.Time) func(a, b *fleet.Agent) bool {
	return func(a, b *fleet.Agent) bool {
		if x, y := asks(a), asks(b); x != y {
			return x
		}
		if x, y := a.Quiet(now), b.Quiet(now); x != y {
			return x < y
		}
		return a.Key < b.Key
	}
}

// noteRuns marks when each agent at work began this run; one no longer
// at work is forgotten, so a restart counts from zero.
func (m *Model) noteRuns(now time.Time, at []*fleet.Agent) {
	next := make(map[string]time.Time, len(at))
	for _, a := range at {
		t, ok := m.runSince[a.Key]
		switch {
		case ok:
		case !m.runInit:
			t = time.Time{} // already at it: since before rush looked
		default:
			t = now
		}
		next[a.Key] = t
	}
	m.runSince = next
	m.runInit = m.runInit || m.snap != nil && len(m.snap.Agents) > 0
}

// runFor is how long a has been at its current run: the session's whole
// life when the run began before rush opened.
func (m *Model) runFor(a *fleet.Agent, now time.Time) time.Duration {
	if t := m.runSince[a.Key]; !t.IsZero() {
		return now.Sub(t)
	}
	return a.Elapsed(now)
}

// runLess orders the working by how long each has been at this run,
// longest first.
func (m *Model) runLess(now time.Time) func(a, b *fleet.Agent) bool {
	return func(a, b *fleet.Agent) bool {
		if x, y := m.runFor(a, now), m.runFor(b, now); x != y {
			return x > y
		}
		return a.Key < b.Key
	}
}

// asks is an agent waiting on an answer from you, not just a next task.
func asks(a *fleet.Agent) bool { return a.NeedsYou() || a.Waiting() || a.Halted() }

// doneLess orders Done by when each was last touched, newest first: put
// away, or used since.
func (m *Model) doneLess(a, b *fleet.Agent) bool {
	last := func(a *fleet.Agent) time.Time {
		if t := m.store.Overlay.Done[a.Key]; t.After(a.UpdatedAt) {
			return t
		}
		return a.UpdatedAt
	}
	if x, y := last(a), last(b); !x.Equal(y) {
		return x.After(y)
	}
	return a.Key < b.Key
}

func (m *Model) setSort(mode string) {
	m.store.Config.SortBy = mode
	_ = m.store.SaveConfig()
	m.rebuild()
	m.flash("sorted by "+mode, false)
}

func sectionMeta(n int, cost float64) string {
	s := fmt.Sprintf("%d", n)
	if cost > 0 {
		s += " · " + money(cost)
	}
	return s
}

func tildify(p string) string {
	home, _ := os.UserHomeDir()
	if home != "" && strings.HasPrefix(p, home) {
		return "~" + p[len(home):]
	}
	return p
}

func cmdErr(text string, f func() error) tea.Cmd {
	return func() tea.Msg {
		if err := f(); err != nil {
			return doneMsg{err: err}
		}
		return doneMsg{text: text}
	}
}

func (m *Model) attach(a *fleet.Agent) tea.Cmd {
	if a == nil {
		return nil
	}
	m.attached = a.Key
	m.markSeen(a)
	if a.Interactive {
		m.flash(fmt.Sprintf("%s is open in another terminal (pid %d)", a.DisplayName, a.PID), false)
		return nil
	}
	if a.Past {
		m.flash(a.DisplayName+" is a past conversation · a message resumes it in rush mode", false)
		return nil
	}
	// One attach at a time from here: the preview's would fight the full
	// screen over the session's size.
	m.closeLive()
	j, ok := agent.As[agent.Joiner](agent.Kind(a.Kind))
	if !ok {
		m.attached = ""
		m.flash("couldn't open "+a.DisplayName+": its agent has no screen of its own to open", true)
		return nil
	}
	return tea.Exec(j.Join(a.Acct, a.ID), func(err error) tea.Msg { return attachDoneMsg{agent: a, err: err} })
}

func (m *Model) togglePin(a *fleet.Agent) tea.Cmd {
	p, id := a.Acct, a.ID
	pn, ok := agent.As[agent.Pinner](p.Kind)
	if !ok {
		m.flash(a.DisplayName+"'s agent keeps no pins", true)
		return nil
	}
	return cmdErr("", func() error { return pn.TogglePin(p, id) })
}

func (m *Model) toggleDone(a *fleet.Agent) {
	// The selection stays in the group the agent leaves: the row below it,
	// or above if it was the last. One whose process markDone is stopping
	// leaves when that exits, so it's counted as gone already, else the
	// selection would follow it down into Done.
	next, from := "", m.sectionOf(a.Key)
	if m.sel == a.Key {
		next = m.neighbour(a.Key)
	}
	stopping := !a.Done && a.PID != 0 && !a.Interactive && !a.Pinned
	defer func() {
		if next != "" && (stopping || m.sectionOf(a.Key) != from) && m.sectionOf(next) == from {
			m.sel = next
		}
	}()
	if _, ok := m.store.Overlay.Done[a.Key]; ok {
		delete(m.store.Overlay.Done, a.Key)
		m.flash("moved back: "+a.DisplayName, false)
	} else {
		m.store.Overlay.Done[a.Key] = time.Now()
		m.flash("done: "+a.DisplayName, false)
	}
	_ = m.store.SaveOverlay()
	m.refresh()
}

// neighbour is the agent row after key in its section, or the one before
// when key is the last; "" when it's alone there.
func (m *Model) neighbour(key string) string {
	var rows []string
	i := -1
	for _, l := range m.lines {
		if l.kind == lineSection {
			if i >= 0 {
				break
			}
			rows = rows[:0]
		}
		if l.kind == lineAgent {
			if l.agent.Key == key {
				i = len(rows)
			}
			rows = append(rows, l.agent.Key)
		}
	}
	switch {
	case i < 0:
		return ""
	case i+1 < len(rows):
		return rows[i+1]
	case i > 0:
		return rows[i-1]
	}
	return ""
}

func (m *Model) nativeView() tea.Cmd {
	v, ok := agent.As[agent.SessionsViewer](loginsKind)
	if !ok {
		return nil
	}
	p := m.store.Config.ActiveAccount().Profile()
	return func() tea.Msg { // exec.Command looks the program up on PATH
		return tea.ExecProcess(v.SessionsView(p), func(err error) tea.Msg { return doneMsg{err: err} })()
	}
}
