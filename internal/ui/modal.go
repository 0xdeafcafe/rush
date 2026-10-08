package ui

import (
	"strings"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/fleet"
)

// A usage limit to decide on opens as a modal over the conversation and
// takes the keys, rather than waiting unseen at the foot of the dock. esc
// sets it aside for later: it waits in the dock then, as a card, ↑ from the
// box to answer it there. What the agent itself asks (a tool call to allow,
// Claude's questions) is part of the conversation, so it never opens as a
// modal: it waits in the dock, ↑ from the box to answer it.

// cardGrace is how long a card that opened while you were typing ignores
// keys, so the end of a message can't answer it.
const cardGrace = 600 * time.Millisecond

// modalCard is whether the card waiting is one that opens as a modal.
func modalCard(c *hostConn) bool { return cardKind(c) == "limit" }

// cardModal reports whether a card shows as a modal now, and gives a modal
// just come the keys.
func (m *Model) cardModal(c *hostConn) bool {
	id := cardID(c)
	if id != c.cardShown {
		c.cardShown, c.cardAt, c.memPick = id, time.Now(), 0
		c.cardTyping = time.Since(c.lastKeyAt) < time.Second
		// What waits on you takes the keys as it comes, unless you were
		// typing: then ↑ gives it them.
		if id != "" && id != c.cardLater && (modalCard(c) || !c.cardTyping && len(c.input) == 0) {
			c.cardFocus, c.sel = true, ""
		}
	}
	return modalCard(c) && id != c.cardLater && c.client != nil
}

// cardGuarded is whether a key comes too soon after a card opened on you
// mid-typing to be meant for it.
func (c *hostConn) cardGuarded() bool {
	return c.cardTyping && time.Since(c.cardAt) < cardGrace
}

// cardLaterNow sets the card waiting aside: it goes to the dock, and the
// keys back to the box.
func (c *hostConn) cardLaterNow() {
	c.cardLater, c.cardFocus = cardID(c), false
}

// modalTitle names the card in the modal's top edge.
func modalTitle(c *hostConn) string {
	if cardKind(c) == "limit" {
		return "usage limit"
	}
	return "needs you"
}

// cardOverlay draws the waiting card as a modal over the pane's body rows
// out[top:top+rows], dimming what's behind it.
func (m *Model) cardOverlay(a *fleet.Agent, c *hostConn, out []string, top, rows, w int) {
	if rows < 6 || w < 30 {
		return
	}
	bw := min(w-4, 104)
	if cardKind(c) == "limit" {
		bw = min(bw, 88)
	}
	c.inModal = true
	card := m.cardRows(a, c, bw-2, rows-3)
	c.inModal = false
	if len(card) == 0 {
		return
	}
	if len(card)+4 <= rows {
		card = append(append([]string{""}, card...), "") // room to breathe
	}
	card = card[:min(len(card), rows-2)]
	for y := top; y < top+rows && y < len(out); y++ {
		out[y] = faint(ansi.Strip(fit(out[y], w)))
	}
	col := cYellow
	if c.cardFocus {
		col = cOrange
	}
	edge := func(s string) string { return paint(col, s) }
	title := " " + modalTitle(c) + " "
	later := " esc later "
	if !c.cardFocus {
		later = " ↑ to answer " // typing your own answer in the box
	}
	fill := bw - 2 - cellw.String(title) - cellw.String(later) - 2
	box := []string{edge("╭─") + paint(col+bold, title) + edge(strings.Repeat("─", max(0, fill))) + dim(later) + edge("─╮")}
	for _, r := range card {
		box = append(box, edge("│")+fit(r, bw-2)+edge("│"))
	}
	box = append(box, edge("╰"+strings.Repeat("─", bw-2)+"╯"))
	y := top + max(0, (rows-len(box))/2)
	pasteAt(out, box, y, (w-bw)/2)
}
