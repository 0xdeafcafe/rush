package ui

import (
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/theme"
	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/charmbracelet/x/ansi"
)

// scratch: renders the feed sheet and dock for eyeballing. FEED_SCRATCH=<label>.
func TestFeedScratch(t *testing.T) {
	label := os.Getenv("FEED_SCRATCH")
	if label == "" {
		t.Skip()
	}
	now := time.Now()
	hub := community.Author{SessionID: "s-hub", Name: "Hub Header Sims Integration"}
	larf := community.Author{SessionID: "s-larf", Name: "LARF Performance Threading Investigation"}
	gfx := community.Author{SessionID: "s-gfx", Name: "Full Graphics Toggle For Low Power Mode"}
	mp := community.Author{SessionID: "s-mp", Name: "multi-picker"}
	cc := community.Author{SessionID: "s-cc", Name: "Commit Changes To Main"}
	emi := community.Author{SessionID: "s-emi", Name: "Finish Emiratus Markdown Rendering"}
	wz := community.Author{SessionID: "s-wz", Name: "if i start writing something long as a title"}
	slack := community.Author{SessionID: "s-slack", Name: "Go TUI Slack Client"}
	you := community.Author{Name: "You"}
	var posts []streamPost
	n := 0
	add := func(a community.Author, text string, ago time.Duration, replyTo string) string {
		n++
		id := fmt.Sprintf("t%02d", n)
		p := streamPost{key: id + "/0", id: id, title: text, author: a, text: text, at: now.Add(-ago), project: "/src/rush"}
		if replyTo != "" {
			p = streamPost{key: replyTo + "/" + id, id: replyTo, author: a, text: text, at: now.Add(-ago), reply: true, project: "/src/rush"}
		}
		posts = append(posts, p)
		return id
	}
	d := time.Hour
	add(emi, "Two agents fixed the same adoptStrategy bug (adoptStrategy in home.go), so check the board before you start on it", 50*d, "")
	wzp := add(wz, "home.go and newest-wins: a thought on the order of the list rows when two land in the same tick", 49*d, "")
	add(emi, "You were right, the append happens after the sort, so the order flips when two land in the same tick", 48*d, wzp)
	add(hub, "Tip: voice the sim env before running anything else", 30*d, "")
	add(hub, "Tip: verify the sim env with sims doctor", 29*d, "")
	add(hub, "Blocker++ the sims API is down for everyone, retry after ten minutes", 28*d, "")
	add(larf, "Tip: find the hot path with pprof before threading anything #perf", 27*d, "")
	add(hub, "Gotcha: the header caches for five minutes in the sims", 26*d, "")
	prog := add(hub, "Progress: the header renders in the sims now, and @scout-dodo the threading tip helped", 25*d, "")
	add(you, "loook", 24*d+30*time.Minute, prog)
	add(larf, "Glad it helped; @waffle-sprout the pprof flame graph for the header is in my branch if you want a second look at it", 24*d+20*time.Minute, prog)
	add(gfx, "heads-up: the toggle lives in settings now (strip out the old flag from your branch before you rebase)", 5*d, "")
	add(mp, "From Alex: rebase first. Rebase fixes the conflicts in picker.go", 4*d+30*time.Minute, "")
	add(cc, "Alex: when you push, don't force. Done for today", 4*d, "")
	add(slack, "--help", 3*d, "")
	add(slack, "Heads-up: move to photon for the shared primitives so nobody duplicates them; cellw, uithread, jsonx", 2*d, "")
	add(slack, "rush ui/frame.Gate is the new way to gate frames off the UI thread, come find me", 5*time.Minute, "")
	dump := func(name string, s string) { _ = os.WriteFile("/tmp/feedshots/"+label+"_"+name+".ansi", []byte(s), 0o644) }
	for _, th := range []string{"dark", "light"} {
		if th == "light" {
			applyColors(theme.Light, false)
		}
		for _, size := range [][2]int{{66, 44}, {106, 44}, {166, 44}} {
			m, _ := benchModel(size[0], size[1])
			m.store.Config.Feed = true
			m.stream.posts = posts
			m.openCommunity("")
			dump(fmt.Sprintf("%s_sheet_%d", th, size[0]), m.render())
			m.community.move(m.community.shown(m), -6) // scrolled up: the view opens partway down a run
			dump(fmt.Sprintf("%s_scrolled_%d", th, size[0]), m.render())
			m.h = 30
			dump(fmt.Sprintf("%s_scrolledshort_%d", th, size[0]), m.render())
			m.h = size[1]
			if size[0] == 106 && th == "dark" {
				bw := m.sheetWidth()
				for _, l := range m.community.body(m, bw-4, 24) {
					fmt.Println("|" + ansi.Strip(l) + "|")
				}
			}
			m.sheet = nil
			m.hover = streamKeyPrefix + posts[len(posts)-2].key
			lines, _ := m.streamLines(size[0]*2/3, 8)
			dump(fmt.Sprintf("%s_docklit_%d", th, size[0]), strings.Join(lines, "\n"))
			m.hover = ""
			lines, _ = m.streamLines(size[0]*2/3, 8)
			dump(fmt.Sprintf("%s_dockquiet_%d", th, size[0]), strings.Join(lines, "\n"))
		}
	}
	applyColors(theme.Dark, false)
	m, _ := benchModel(106, 44)
	m.store.Config.Feed = true
	m.stream.posts = posts
	m.openCommunity("")
	for h := 10; h < 30; h++ {
		fmt.Printf("\n===== h %d =====\n", h)
		for _, l := range m.community.body(m, 96, h) {
			fmt.Println("|" + ansi.Strip(l) + "|")
		}
	}
}
