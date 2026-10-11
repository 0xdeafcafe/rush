package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/community"
	"github.com/charmbracelet/x/ansi"
)

// Each agent that's messaged another is a mark with its name, each pair a
// line with its count, and nothing runs past the box.
func TestFeedGraphDrawsWhoTalksToWhom(t *testing.T) {
	now := time.Now()
	post := func(from, to string) streamPost {
		return streamPost{author: community.Author{Handle: from, Name: from}, to: "@" + to, at: now.Add(-time.Hour)}
	}
	posts := []streamPost{post("pepper-bean", "scout-dodo"), post("scout-dodo", "pepper-bean"), post("pepper-bean", "scout-dodo"),
		post("pepper-bean", "waffle-bramble"), post("waffle-bramble", "marmite-bramble"), {title: "a chirp", author: community.Author{Name: "x"}}}
	nodes, edges := feedGraph(posts)
	if len(nodes) != 4 || nodes[0] != "@pepper-bean" || len(edges) != 3 || edges[0].n != 3 {
		t.Fatalf("nodes %v edges %+v", nodes, edges)
	}
	rows := drawGraph(nodes, edges, 80, 16, now)
	plain := ansi.Strip(strings.Join(rows, "\n"))
	if os.Getenv("SHOW") != "" {
		t.Log("\n" + plain)
	}
	for _, n := range nodes {
		if !strings.Contains(plain, n) {
			t.Fatalf("%s missing:\n%s", n, plain)
		}
	}
	if !strings.Contains(plain, " 3 ") || strings.Count(plain, "●") != 4 {
		t.Fatalf("counts and marks:\n%s", plain)
	}
	for _, r := range rows {
		if cellw.String(r) > 80 {
			t.Fatalf("a row runs past the box:\n%s", plain)
		}
	}
}
