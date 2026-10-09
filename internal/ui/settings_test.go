package ui

import (
	"cmp"
	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
	"os"
	"runtime"
	"slices"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/state"
)

// [ and ] go through Settings' pages, round the ends, as tab and
// shift+tab do.
func TestSettingsPagesBrackets(t *testing.T) {
	m, _ := benchModel(140, 50)
	m.setSettingsPage(pageProviders)
	m.Update(tea.KeyPressMsg{Code: ']', Text: "]"})
	if m.dialog.page != pageHarnesses {
		t.Fatalf("] went to page %d, not Harnesses", m.dialog.page)
	}
	m.Update(tea.KeyPressMsg{Code: '[', Text: "["})
	m.Update(tea.KeyPressMsg{Code: '[', Text: "["})
	if m.dialog.page != pageProfiles {
		t.Fatalf("[ from Providers went to page %d, not Harnesses' last (Profiles)", m.dialog.page)
	}
	m.setSettingsPage(pageGeneral)
	m.Update(tea.KeyPressMsg{Code: tea.KeyTab})
	if m.dialog == nil || m.dialog.page != pageGeneral+1 {
		t.Fatal("tab didn't go to Settings' next page")
	}
	m.Update(tea.KeyPressMsg{Code: tea.KeyTab, Mod: tea.ModShift})
	if m.dialog == nil || m.dialog.page != pageGeneral {
		t.Fatal("shift+tab didn't go back a page")
	}
	if !strings.Contains(ansi.Strip(m.pages()), "[ ]") {
		t.Fatalf("the page strip doesn't say [ ]: %q", ansi.Strip(m.pages()))
	}
}

// Every page draws, and every form row explains itself.
func TestSettingsPagesDraw(t *testing.T) {
	m, _ := benchModel(140, 50)
	m.setView(placeSettings)
	for i, p := range m.settingsPages() {
		m.setSettingsPage(i)
		if body := m.dialogBody(130); len(body) < 3 {
			t.Fatalf("%s drew %d lines", p.name, len(body))
		}
		if p.form == nil {
			continue
		}
		for _, st := range flat(p.form(m)) {
			if st.line == nil && st.what == "" && st.about == nil {
				t.Errorf("%s › %s doesn't say what it does", p.name, st.label)
			}
		}
	}
}

// An agent other than the built-in one keeps what its sessions start with
// apart from Claude Code's.
func TestStartForKind(t *testing.T) {
	var d state.Dispatch
	d.SetStartFor("claude", state.Start{Model: "opus", Mode: "plan"})
	d.SetStartFor("codex", state.Start{Effort: "high", Mode: "auto"})
	if d.Model != "opus" || d.Permission != "plan" {
		t.Fatalf("Claude's start isn't in Model and Permission: %+v", d)
	}
	if got := d.StartFor("codex"); got.Effort != "high" || got.Mode != "auto" || got.Model != "" {
		t.Fatalf("codex start = %+v", got)
	}
	d.SetStartFor("codex", state.Start{})
	if _, ok := d.Starts["codex"]; ok {
		t.Fatal("an emptied start is kept")
	}
}

// Providers is one page whatever is installed: 1-9 pick a provider, enter
// goes into it and esc back, and an agent's settings open on its own.
func TestSettingsProvidersOnePage(t *testing.T) {
	m, _ := accountsModel(t)
	m.setView(placeSettings)
	if n := len(m.settingsPages()); n != pageRemote+1 {
		t.Fatalf("%d pages: a provider has a page of its own again", n)
	}
	m.setSettingsPage(pageProviders)
	if it := m.provPicked(); it.provider != "claude" {
		t.Fatalf("Providers opens on %+v, not the first", it)
	}
	m.Update(tea.KeyPressMsg{Code: '2', Text: "2"})
	if it := m.provPicked(); it != m.provItems()[1] {
		t.Fatalf("2 picked %+v", it)
	}
	m.dialog.cursor = slices.Index(m.provItems(), provItem{provider: "zcodex"})
	m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if it := m.provPicked(); !m.dialog.inside || it.provider != "zcodex" {
		t.Fatalf("enter: inside %v on %+v", m.dialog.inside, it)
	}
	body := ansi.Strip(strings.Join(m.dialogBody(150), "\n"))
	for _, want := range []string{"Account", "one@example.com", "New sessions start with", "When a limit stops a session", "Harnesses", "What it can do"} {
		if !strings.Contains(body, want) {
			t.Errorf("zcodex doesn't show %q:\n%s", want, body)
		}
	}
	m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	if m.dialog == nil || m.dialog.inside || m.provPicked().provider != "zcodex" {
		t.Fatal("esc didn't go back to the list, on the provider")
	}
	// ← on a row with no choices goes back out too; on one with choices it
	// goes back through them.
	m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	secs := m.provForm(m.provPicked())
	for i, r := range flat(secs) {
		if len(r.choices) == 0 && r.key == nil {
			m.dialog.cursor = i
			break
		}
	}
	m.Update(tea.KeyPressMsg{Code: tea.KeyLeft})
	if m.dialog.inside || m.provPicked().provider != "zcodex" {
		t.Fatal("← didn't go back to the list, on the provider")
	}
	m.openAgentSettings("zplain")
	if m.dialog.page != pageProviders || !m.dialog.inside || m.provPicked().provider != "zplain" {
		t.Fatal("openAgentSettings didn't land in the agent's provider")
	}
	// Narrow, the list and what's picked take turns.
	if body := ansi.Strip(strings.Join(m.dialogBody(80), "\n")); strings.Contains(body, "│") || !strings.Contains(body, "What it can do") {
		t.Fatalf("narrow and inside, not the provider alone:\n%s", body)
	}
}

// Profiles makes a profile, changes its agents and what it does at a
// limit, gives it a folder, and makes it the default, all by keys.
func TestSettingsProfiles(t *testing.T) {
	m, _ := accountsModel(t)
	m.setView(placeSettings)
	m.setSettingsPage(pageProfiles)
	cfg := &m.store.Config
	editing := func() (state.Profile, bool) { return cfg.ProfileNamed(m.provPicked().profile) }
	press := func(keys ...string) {
		for _, k := range keys {
			switch k {
			case "enter":
				m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
			case "esc":
				m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
			case "down":
				m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
			case "right":
				m.Update(tea.KeyPressMsg{Code: tea.KeyRight})
			default:
				for _, r := range k {
					m.Update(tea.KeyPressMsg{Code: r, Text: string(r)})
				}
			}
		}
	}
	before := len(cfg.Profiles)
	press("n")
	if got := string(m.dialog.input); got != "claudecode:work" {
		t.Fatalf("a new profile is offered the name %q, not its harness and account", got)
	}
	m.dialog.input = nil
	press("client", "enter")
	if len(cfg.Profiles) != before+1 || !m.dialog.inside || m.provPicked().profile != "client" {
		t.Fatalf("n didn't make and open a profile: %d profiles, open %+v", len(cfg.Profiles), m.provPicked())
	}
	body := ansi.Strip(strings.Join(m.dialogBody(130), "\n"))
	for _, want := range []string{"client", "Providers", "Starts with", "When a limit stops a session", "Folders"} {
		if !strings.Contains(body, want) {
			t.Fatalf("the open profile doesn't show %q:\n%s", want, body)
		}
	}
	// Add the second agent, then set it to hand off at a limit.
	p, _ := editing()
	n := len(p.Providers)
	press("down", "enter")
	if p, _ = editing(); len(p.Providers) == n {
		t.Fatalf("enter on an agent didn't add or drop it: %v", p.Providers)
	}
	if body := ansi.Strip(strings.Join(m.dialogBody(130), "\n")); !strings.Contains(body, "When the first is out") {
		t.Fatalf("with two providers, no choice of what to do when the first is out:\n%s", body)
	}
	rows := flat(m.provForm(m.provPicked()))
	for i, r := range rows {
		if r.label == "When a limit stops a session" {
			m.dialog.cursor = i
		}
		if body := ansi.Strip(strings.Join(m.dialogBody(130), "\n")); !strings.Contains(body, "When the first is out") {
			t.Fatalf("with two providers, no choice of what to do when the first is out:\n%s", body)
		}
	}
	press("right")
	if p, _ = editing(); p.Limit() != state.LimitHandoff {
		t.Fatalf("→ on the limit row: %q", p.Limit())
	}
	// A folder for it.
	for i, r := range flat(m.provForm(m.provPicked())) {
		if r.label == "+ add a folder" {
			m.dialog.cursor = i
		}
	}
	press("enter")
	m.dialog.input = []rune("~/src/client")
	press("enter")
	if r, ok := cfg.RuleFor(state.ExpandHome("~/src/client/app")); !ok || r.Profile != "client" {
		t.Fatalf("folder rule: %+v %v", r, ok)
	}
	// esc goes back to the list, not out of Settings.
	press("esc")
	if m.dialog == nil || m.dialog.inside {
		t.Fatal("esc in a profile left Settings")
	}
	m.dialog.cursor = slices.Index(m.provItems(), provItem{profile: "client"})
	press("*")
	if cfg.Default().Name != "client" {
		t.Fatalf("* didn't make it the default: %s", cfg.Default().Name)
	}
}

// An agent's own settings file shows as its adapter describes it, and a
// row changed there is saved to that file; an agent with none shows none.
func TestAgentFileSections(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	m := &Model{snap: &fleet.Snapshot{}, store: &state.Store{}, dialog: &dialog{}}
	secs := m.fileSections("claude")
	if len(secs) != 2 || secs[0].title != "settings.json" || secs[1].title != "Environment" {
		t.Fatalf("sections: %+v", secs)
	}
	for _, st := range secs[0].rows {
		if st.label == "Default model" {
			st.set("opus")
		}
	}
	b, err := os.ReadFile(m.dialog.settings.Path)
	if err != nil || !strings.Contains(string(b), `"model": "opus"`) {
		t.Fatalf("settings.json after a change: %s %v", b, err)
	}
	if defs := m.agentDefs("claude"); len(defs) == 0 || defs[0].Path != "" {
		t.Fatalf("the built-in definition isn't first: %+v", defs)
	}
	if secs := m.fileSections("nosuch"); secs != nil {
		t.Fatalf("an unknown agent has sections: %+v", secs)
	}
}

// What a model takes says images, PDFs and its window when the agent
// knows them, and that it doesn't when it doesn't.
func TestModelTakes(t *testing.T) {
	if got := ansi.Strip(modelTakes("claude", "opus")); got != "✓ images · ✓ PDFs · 1M context" {
		t.Errorf("opus takes %q", got)
	}
	if got := ansi.Strip(modelTakes("nobody", "m")); !strings.Contains(got, "unknown to rush") {
		t.Errorf("an agent that knows nothing of its model: %q", got)
	}
}

// A plugin's requirements are a cell each, met here or not.
func TestNeedCells(t *testing.T) {
	r := plugin.Requires{OS: []string{runtime.GOOS, "plan9"}, Arch: []string{runtime.GOARCH}, Bin: []string{"git", "nope"}}
	got := ansi.Strip(needCells(r, []string{"nope"}))
	want := "✓ " + cmp.Or(plugin.OSNames[runtime.GOOS], runtime.GOOS) + "  – plan9  ✓ " + runtime.GOARCH + "  ✓ git on PATH  – nope on PATH"
	if got != want {
		t.Fatalf("needCells = %q, want %q", got, want)
	}
}

// A setting that changes the Agents list shows it under About, as it will
// look: two-line rows always, then one line wherever it fits.
func TestSettingPreviewsTheList(t *testing.T) {
	m, _ := benchModel(200, 60)
	m.store.Config.SetView("list") // the list alone, wide enough for one-line rows
	m.full, m.preview = false, false
	m.setView(placeSettings)
	m.setSettingsPage(pageGeneral)
	var stack setting
	for i, r := range flat(m.settingsPages()[pageGeneral].form(m)) {
		if r.label == "Two-line rows" {
			m.dialog.cursor, stack = i, r
		}
	}
	if stack.preview == nil {
		t.Fatal("Two-line rows has no preview")
	}
	shown := func() string { return ansi.Strip(strings.Join(m.dialogBody(190), "\n")) }
	q := m.snap.Agents[0] // only a row that asks something has a second line
	q.State, q.PID, q.Seen, q.Checking = "blocked", 1, false, false
	m.rebuild()
	m.store.Config.StackAt = 100
	if !strings.Contains(shown(), "╰ ") {
		t.Errorf("always: no second lines in the preview:\n%s", shown())
	}
	m.store.Config.StackAt = -1
	if strings.Contains(shown(), "  ╰ ") {
		t.Errorf("narrow only, on a wide list: second lines in the preview:\n%s", shown())
	}
}

// Spaces and tabs in diffs shows a Session beside the list, its diff marked
// or not as the setting says.
func TestSpacesPreviewShowsASession(t *testing.T) {
	m, _ := benchModel(200, 60)
	m.store.Config.SetView("split")
	m.setView(placeSettings)
	m.setSettingsPage(pageGeneral)
	for i, r := range flat(m.settingsPages()[pageGeneral].form(m)) {
		if r.label == "Spaces and tabs in diffs" {
			m.dialog.cursor = i
		}
	}
	shown := func() string { return ansi.Strip(strings.Join(m.dialogBody(190), "\n")) }
	defer convo.SetShowWhitespace(false)
	convo.SetShowWhitespace(true)
	if s := shown(); !strings.Contains(s, "Session") || !strings.Contains(s, "→   →   err") {
		t.Fatalf("shown: no marked Session in the preview:\n%s", s)
	}
	if s := shown(); !strings.Contains(s, "retry the upload") {
		t.Errorf("with room for it, the example shows from its start:\n%s", s)
	}
	convo.SetShowWhitespace(false)
	if s := shown(); strings.Contains(s, "→   →") || strings.Contains(s, "·send") {
		t.Errorf("hidden: marks in the preview:\n%s", s)
	}
}

// Clicking a tab in the header goes to its place, and a page's name under
// it to that page.
func TestClickingTabs(t *testing.T) {
	m, _ := benchModel(200, 60)
	col := func(line, name string) int {
		before, _, _ := strings.Cut(ansi.Strip(line), name)
		return cellw.String(before) + 1
	}
	m.setView(placeAgents)
	if _, ok := m.clickTab(col(m.header()[2], "Settings"), 2); !ok || m.view != placeSettings || m.dialog == nil {
		t.Fatalf("clicking Settings left the view at %d", m.view)
	}
	if _, ok := m.clickTab(col(m.underHead()[0], "Keys"), m.headH()); !ok || m.dialog.page != pageKeys {
		t.Errorf("clicking Keys: page %d", m.dialog.page)
	}
	if _, ok := m.clickTab(0, m.headH()); ok {
		t.Error("a click left of the pages turned one")
	}
}

// With the logo hidden the header is three rows of text, and its tabs and
// pages still click through.
func TestHiddenLogo(t *testing.T) {
	m, _ := benchModel(200, 60)
	m.store.Config.HideLogo = true
	h := m.header()
	if len(h) != 3 || m.headH() != 3 || !strings.HasPrefix(ansi.Strip(h[0]), "  rush") || strings.Contains(ansi.Strip(strings.Join(h, "")), "RUSH") {
		t.Fatalf("header:\n%s", ansi.Strip(strings.Join(h, "\n")))
	}
	col := func(line, name string) int {
		before, _, _ := strings.Cut(ansi.Strip(line), name)
		return cellw.String(before) + 1
	}
	m.setView(placeAgents)
	if _, ok := m.clickTab(col(h[2], "Settings"), 2); !ok || m.view != placeSettings {
		t.Fatalf("clicking Settings left the view at %d", m.view)
	}
	if _, ok := m.clickTab(col(m.underHead()[0], "Keys"), m.headH()); !ok || m.dialog.page != pageKeys {
		t.Errorf("clicking Keys: page %d", m.dialog.page)
	}
}

// Anthropic's subscription and its API key are providers of their own:
// the key has no account or limits, and says whether there's one yet.
func TestProvidersSplitByBilling(t *testing.T) {
	m, _ := benchModel(160, 50)
	m.setView(placeSettings)
	m.setSettingsPage(pageProviders)
	items := m.provItems()
	i := slices.Index(items, provItem{provider: "claude"})
	if i < 0 || i+1 >= len(items) || items[i+1].provider != "claude-key" {
		t.Fatalf("no Anthropic key after its subscription: %+v", items)
	}
	m.dialog.cursor = i + 1
	body := ansi.Strip(strings.Join(m.dialogBody(150), "\n"))
	for _, want := range []string{"Anthropic · API key", "none yet · $ adds one", "Harnesses", "What it can do"} {
		if !strings.Contains(body, want) {
			t.Errorf("the key doesn't show %q:\n%s", want, body)
		}
	}
	if strings.Contains(body, "Account ·") || strings.Contains(body, "When a limit stops") {
		t.Errorf("the key shows accounts or limits:\n%s", body)
	}
}
