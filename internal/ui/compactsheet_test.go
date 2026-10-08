package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/charmbracelet/x/ansi"
)

func TestOldHostKeepsNativeCompactionButBlocksExternal(t *testing.T) {
	m := &Model{store: &state.Store{}}
	c := &hostConn{key: "test", client: &host.Client{}, sess: &convo.Session{Info: host.Info{Proto: 8, State: "idle"}}}
	m.host = c
	cmd := m.openCompact(c, &fleet.Agent{Kind: "claude", ID: "test"})
	if cmd != nil {
		t.Fatal("old host should not discover external models")
	}
	s, ok := m.sheet.(*compactSheet)
	if !ok {
		t.Fatal("no compaction picker")
	}
	if len(s.opts) != 1 || s.opts[0].model != "" {
		t.Fatalf("native option lost: %v", s.opts)
	}
	body := strings.Join(strings.Fields(ansi.Strip(strings.Join(s.body(m, 100, 30), "\n"))), " ")
	if !strings.Contains(body, "Restart") || !strings.Contains(body, "native compaction remains available") {
		t.Fatal(body)
	}
	if cmd := m.compactBy(c, "test", summarizer{kind: "ollama", model: "writer"}, ""); cmd != nil {
		t.Fatal("unguarded external compaction scheduled")
	}
}

func TestCompactionDefaultIsExplicitAndPersistent(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("RUSH_HOME", dir)
	m := &Model{store: &state.Store{}}
	s := &compactSheet{opts: []summarizer{{label: "native"}, {kind: agent.Kind("ollama"), model: "writer", label: "Ollama"}}, cur: 1}
	m.sheet = s
	if cmd := s.key(m, tea.KeyPressMsg{}, "d"); cmd != nil {
		t.Fatal("saving default must not execute compaction")
	}
	if m.store.Config.CompactKind != "ollama" || m.store.Config.CompactModel != "writer" {
		t.Fatal("choice not saved")
	}
	b, err := os.ReadFile(filepath.Join(dir, "config.json"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), `"compactModel":"writer"`) && !strings.Contains(string(b), `"compactModel": "writer"`) {
		t.Fatalf("not persisted: %s", b)
	}
	if m.sheet != s {
		t.Fatal("saving preference closed picker")
	}
	s.cur = 0
	s.key(m, tea.KeyPressMsg{}, "d")
	if m.store.Config.CompactKind != "" || m.store.Config.CompactModel != "" {
		t.Fatal("native preference not restored")
	}
}

func TestEmptyCompactionPickerCannotPanic(t *testing.T) {
	m := &Model{store: &state.Store{}}
	s := &compactSheet{}
	for _, k := range []string{"down", "enter", "d", "up", "enter"} {
		s.key(m, tea.KeyPressMsg{}, k)
	}
}

func TestCompactionExplanationsStayInsideSheetAndScroll(t *testing.T) {
	m := &Model{store: &state.Store{}}
	s := &compactSheet{opts: []summarizer{{label: "native"}}, errs: []string{strings.Repeat("Classifier explanation details. ", 30) + "LAST EXPLANATION"}}
	first := s.body(m, 50, 16)
	if len(first) > 16 {
		t.Fatalf("height=%d", len(first))
	}
	for _, line := range first {
		if ansi.StringWidth(line) > 50 {
			t.Fatalf("too wide: %s", ansi.Strip(line))
		}
	}
	for range 30 {
		s.key(m, tea.KeyPressMsg{}, "pgdown")
		s.body(m, 50, 16)
	}
	last := ansi.Strip(strings.Join(s.body(m, 50, 16), "\n"))
	if !strings.Contains(last, "LAST EXPLANATION") {
		t.Fatalf("cannot inspect final reason: %s", last)
	}
}

func TestCompactionUsesAuthoritativeHostKind(t *testing.T) {
	m := &Model{store: &state.Store{}}
	c := &hostConn{key: "test", client: &host.Client{}, sess: &convo.Session{Info: host.Info{Proto: 9, Kind: "unknown-no-compact", State: "idle"}}}
	m.openCompact(c, &fleet.Agent{Kind: "claude"})
	s := m.sheet.(*compactSheet)
	if len(s.opts) != 0 {
		t.Fatal("stale fleet kind advertised unsupported native compaction")
	}
}

func TestCodexOffersNativeCompactionWithoutExternalSummaries(t *testing.T) {
	for _, kind := range []string{"codex", "ollama-codex"} {
		m := &Model{store: &state.Store{}}
		c := &hostConn{key: "test", client: &host.Client{}, sess: &convo.Session{Info: host.Info{Proto: 10, Kind: kind, State: "idle"}}}
		if cmd := m.openCompact(c, &fleet.Agent{Kind: kind}); cmd != nil {
			t.Fatal("discovering external summaries without rewind support")
		}
		s := m.sheet.(*compactSheet)
		if len(s.opts) != 1 || s.opts[0].model != "" || s.loading {
			t.Fatalf("%s: missing native compaction: %+v", kind, s)
		}
	}
}
