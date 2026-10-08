package main

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// What rush takes in sidebar.set: more is refused whole.
const (
	maxSidebarAgents = 2000
	maxSidebarName   = 200
)

var sessionIDRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`)

func clipRunes(s string, n int) string {
	if r := []rune(s); len(r) > n {
		return string(r[:n-1]) + "…"
	}
	return s
}

// sidebarPayload is the board as rush's agent list shows it: a section
// per column, in board order, and each card's agent under the card's name,
// in the card's place in its column. Only cards with a conversation are
// there, as rush knows agents by it.
func sidebarPayload(cards []card) map[string]any {
	type section struct {
		Title string `json:"title"`
	}
	type agent struct {
		Name    string `json:"name,omitempty"`
		Section string `json:"section"`
		Order   int    `json:"order"`
	}
	sections := []section{}
	agents := map[string]agent{}
	for _, col := range columns {
		var in []card
		for _, c := range cards {
			if c.Column == col.id && c.sessionID() != "" {
				in = append(in, c)
			}
		}
		if len(in) == 0 {
			continue
		}
		sortColumn(in)
		sections = append(sections, section{Title: col.title})
		for i, c := range in {
			id := c.sessionID()
			if _, dup := agents[id]; dup || !sessionIDRE.MatchString(id) || len(agents) >= maxSidebarAgents {
				continue
			}
			name := c.title()
			if name == c.ID {
				name = "" // no name of its own: rush keeps the agent's
			}
			agents[id] = agent{Name: clipRunes(name, maxSidebarName), Section: col.title, Order: i}
		}
	}
	return map[string]any{"title": "Kanban", "sections": sections, "agents": agents}
}

// showBoard keeps rush's list arranged as the board: it looks at
// links.json every two seconds, and sends the arrangement when it changed.
func showBoard() {
	var mod time.Time
	var sent []byte
	t := time.NewTicker(2 * time.Second)
	defer t.Stop()
	for {
		if fi, err := os.Stat(filepath.Join(kanbanHome(), "links.json")); err == nil && !fi.ModTime().Equal(mod) {
			if cards, err := readCards(); err == nil {
				p := sidebarPayload(cards)
				b, _ := jsonx.Marshal(p)
				if string(b) == string(sent) {
					mod = fi.ModTime()
				} else if err := conn.Call(context.Background(), "sidebar.set", p, nil); err == nil {
					mod, sent = fi.ModTime(), b
				} else {
					logf("arranging rush's list: %v", err)
					var e *plugin.Error
					if errors.As(err, &e) && e.Code == plugin.CodeDenied {
						return // not approved for it
					}
				}
			}
		}
		select {
		case <-conn.Done():
			return
		case <-t.C:
		}
	}
}
