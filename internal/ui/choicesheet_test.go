package ui

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/charmbracelet/x/ansi"
)

// /model and /effort with nothing named open the agent's list, the one
// running marked; enter switches to the one chosen, esc leaves it be.
func TestChoicePicker(t *testing.T) {
	m, c := infoModel(t) // runs claude-opus-5-5
	c.input = []rune("/model")
	m.sendPane(c, false)
	s, ok := m.sheet.(*choiceSheet)
	if !ok || len(c.input) != 0 {
		t.Fatalf("/model opened %T, input %q", m.sheet, string(c.input))
	}
	text := ansi.Strip(strings.Join(s.body(m, 84, 40), "\n"))
	for _, want := range []string{"Model", "default", "opus[1m]", "haiku", "esc cancel"} {
		if !strings.Contains(text, want) {
			t.Errorf("picker lacks %q:\n%s", want, text)
		}
	}
	if s.opts[s.now].ID != "opus" || s.cur != s.now {
		t.Fatalf("running model not marked: now %d cur %d\n%s", s.now, s.cur, text)
	}

	s.key(m, tea.KeyPressMsg{}, "esc")
	if m.sheet != nil || c.picked != nil {
		t.Fatalf("esc: sheet %T, picked %v", m.sheet, c.picked)
	}

	// Down twice from opus is sonnet; enter switches to it, and it's the
	// one marked from then on, here and as you type /model <name>.
	m.openChoices(c, "model")
	s = m.sheet.(*choiceSheet)
	s.key(m, tea.KeyPressMsg{}, "down")
	s.key(m, tea.KeyPressMsg{}, "down")
	if cmd := s.key(m, tea.KeyPressMsg{}, "enter"); cmd == nil || m.sheet != nil || c.picked["model"] != "sonnet" {
		t.Fatalf("enter: cmd %v sheet %T picked %v", cmd != nil, m.sheet, c.picked)
	}
	if !strings.Contains(m.status, "model: sonnet") {
		t.Fatalf("status %q", m.status)
	}
	c.input = []rune("/model so")
	if got := argMatches(c); len(got) != 1 || !strings.HasSuffix(got[0].Description, "now") {
		t.Fatalf("/model so: %v", got)
	}

	// /model <name> still switches right away; default is the agent's own.
	c.input = []rune("/model default")
	m.sendPane(c, false)
	if m.sheet != nil || c.picked["model"] != "" {
		t.Fatalf("/model default: sheet %T picked %v", m.sheet, c.picked)
	}
	m.openChoices(c, "model")
	if s = m.sheet.(*choiceSheet); s.now != 0 {
		t.Fatalf("default not marked: %d", s.now)
	}

	// A model not listed gets a row of its own, marked; effort's list is
	// the agent's efforts.
	c.picked, c.sess.Model = nil, "claude-mythos-5"
	m.openChoices(c, "model")
	s = m.sheet.(*choiceSheet)
	if last := s.opts[len(s.opts)-1]; s.now != len(s.opts)-1 || last.ID != "claude-mythos-5" {
		t.Fatalf("model not listed: now %d last %+v", s.now, last)
	}
	c.sess.Info.Effort = "xhigh"
	m.openChoices(c, "effort")
	s = m.sheet.(*choiceSheet)
	if s.opts[s.now].ID != "xhigh" || !strings.Contains(ansi.Strip(strings.Join(s.body(m, 84, 40), "\n")), "next turn") {
		t.Fatalf("effort: %+v", s.opts[s.now])
	}
}

// The words of a full model id pick out the alias it runs as.
func TestArgNowReadsAModelID(t *testing.T) {
	_, c := infoModel(t)
	opts := argOptions(c, "model")
	for id, want := range map[string]string{"claude-opus-5-5[1m]": "opus[1m]", "claude-sonnet-5": "sonnet", "haiku": "haiku"} {
		c.sess.Model = id
		if got, ok := argNow(c, "model", opts); !ok || got != want {
			t.Errorf("%s: %q %v", id, got, ok)
		}
	}
	c.sess.Model = "gpt-6"
	if _, ok := argNow(c, "model", opts); ok {
		t.Error("gpt-6 read as one of Claude's")
	}
}

func TestModelCommandsAlwaysOpenPicker(t *testing.T) {
	for _, kind := range []string{"claude", "codex"} {
		for _, native := range []bool{false, true} {
			for _, command := range []string{"/model", "#model"} {
				t.Run(kind+command+map[bool]string{true: "native", false: "hosted"}[native], func(t *testing.T) {
					m, c := infoModel(t)
					c.kind = agent.Kind(kind)
					c.sess.Info.Kind = kind
					if native {
						c.client = nil
					}
					c.input = []rune(command)
					m.sendPane(c, false)
					if _, ok := m.sheet.(*choiceSheet); !ok {
						t.Fatalf("%s should open model picker, got %T, status %q", command, m.sheet, m.status)
					}
					if len(c.input) != 0 {
						t.Fatalf("command left in message box: %q", string(c.input))
					}
					if len(c.picked) > 0 {
						t.Fatal("opening a picker must not reset or change the model")
					}
				})
			}
		}
	}
}

func TestSessionShiftTabPreservesDraft(t *testing.T) {
	m, c := infoModel(t)
	m.paneFocus = true
	c.input = []rune("unfinished draft")
	c.back = 4
	pressKeys(m, "shift+tab")
	s, ok := m.sheet.(*startSheet)
	if !ok || s.conn != c.key {
		t.Fatalf("Shift+Tab should open current session controls, got %T, %q", m.sheet, m.status)
	}
	if string(c.input) != "unfinished draft" || c.back != 4 {
		t.Fatal("opening session controls altered draft")
	}
	s.key(m, tea.KeyPressMsg{}, "esc")
	if m.sheet != nil || string(c.input) != "unfinished draft" {
		t.Fatal("cancel lost draft")
	}
}

func TestEmptyModelCatalogDoesNotResetModel(t *testing.T) {
	m, c := infoModel(t)
	c.kind = "codex"
	c.sess.Info.Kind = "codex"
	m.listed = map[string][]agent.Choice{}
	c.picked = map[string]string{"model": "custom-current-model"}
	if !m.openChoices(c, "model") {
		t.Fatal("empty model catalog should still open picker")
	}
	if c.picked["model"] != "custom-current-model" {
		t.Fatal("picker reset current model")
	}
}

func TestDiscoveredModelsExtendDefaults(t *testing.T) {
	m, _ := infoModel(t)
	m.listed = map[string][]agent.Choice{"claude": {{ID: "configured-model", Note: "from provider config"}}}
	models := m.models("claude")
	if len(models) == 0 || models[0].ID != "configured-model" {
		t.Fatal("configured model must precede defaults")
	}
	found := false
	for _, model := range models {
		found = found || model.ID == "opus"
	}
	if !found {
		t.Fatal("built-in default models must remain selectable")
	}
}

func TestHashPickerHidesUnavailableSessionActions(t *testing.T) {
	m, c := infoModel(t)
	m.paneFocus = true
	a := m.agentByKey(c.key)
	if a == nil {
		t.Fatal("fixture has no agent")
	}
	a.Kind = "installed"
	a.Acct.Kind = "installed"
	c.kind = "installed"
	c.sess.Info.Kind = "installed"
	for _, name := range []string{"compact", "full", "pin", "slim"} {
		for _, command := range m.availableFleetCommands() {
			if command.Name == name {
				t.Errorf("unsupported #%s still offered", name)
			}
		}
	}
}
