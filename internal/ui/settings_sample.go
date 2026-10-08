package ui

import (
	"strings"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/adapters/claude/headless"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/0xdeafcafe/rush/internal/host"
)

// sampleSession is a short made-up conversation with an edit in it, for
// Settings to show a change to how a Session looks: its diff indented with
// tabs, and a line whose only change is spaces for a tab.
var sampleSession = func() *convo.Session {
	s := convo.New()
	s.Info.Cwd = "/work/app"
	raw := func(v any) []byte { b, _ := jsonx.Marshal(v); return b }
	at := time.Date(2026, 9, 30, 9, 0, 0, 0, time.UTC)
	for i, ev := range []any{
		host.Sent{Text: "retry the upload when it times out"},
		headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "tool_use", ID: "e1", Name: "Edit",
			Input: raw(map[string]any{"file_path": "/work/app/upload.go"})}}},
		headless.Message{Role: "user", Blocks: []headless.Block{{Type: "tool_result", ToolUseID: "e1", Text: "ok"}},
			ToolResult: raw(map[string]any{"structuredPatch": []map[string]any{{"oldStart": 12, "oldLines": 3, "newStart": 12, "newLines": 6,
				"lines": []string{" \tfor try := range 3 {", "-        err = send(ctx, f)", "+\t\terr = send(ctx, f)", "+\t\tif err == nil {", "+\t\t\tbreak", "+\t\t}", " \t}"}}}})},
		headless.Message{Role: "assistant", Blocks: []headless.Block{{Type: "text", Text: "Uploads now retry up to **three** times."}}},
		headless.Result{Subtype: "success"},
	} {
		s.Apply(ev, at.Add(time.Duration(i)*time.Second))
	}
	return s
}()

// sessionPreview is sampleSession as a Session draws it now, w wide.
func sessionPreview(w, rows int) []string {
	var out []string
	blank := true // no blank line first, nor two together
	for _, l := range sampleSession.Render(convo.Options{Width: w, Now: time.Now(), Open: map[string]bool{}}) {
		b := strings.TrimSpace(ansi.Strip(l.Text)) == ""
		if !b || !blank {
			out = append(out, l.Text)
		}
		blank = b
	}
	if blank && len(out) > 0 {
		out = out[:len(out)-1]
	}
	if len(out) > rows {
		out = out[len(out)-rows:]
	}
	return out
}
