package ui

import (
	"fmt"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// artifact is a page the session published with the Artifact tool, at its
// latest version.
type artifact struct {
	URL      string
	File     string
	About    string
	Version  int
	Versions int
	Turn     int
	At       time.Time
}

var artifactVer = regexp.MustCompile(`\(Version (\d+)\)`)

// artifacts lists what the session published, newest first, one row per
// page however many versions it went through.
func artifacts(s *convo.Session) []*artifact {
	byURL := map[string]*artifact{}
	var order []*artifact
	for _, t := range s.Turns {
		for _, it := range t.Items {
			if it.Kind != convo.KStep || it.Step.Tool != "Artifact" || it.Step.Status != convo.OK {
				continue
			}
			st := it.Step
			var in struct {
				Action      string `json:"action"`
				FilePath    string `json:"file_path"`
				Description string `json:"description"`
				URL         string `json:"url"`
			}
			_ = jsonx.Unmarshal(st.Input, &in)
			if in.Action != "" && in.Action != "publish" {
				continue
			}
			url := convo.URLIn(st.Output)
			if url == "" {
				url = in.URL
			}
			if url == "" {
				continue
			}
			a := byURL[url]
			if a == nil {
				a = &artifact{URL: url}
				byURL[url] = a
				order = append(order, a)
			}
			a.Versions++
			if m := artifactVer.FindStringSubmatch(st.Output); m != nil {
				a.Version, _ = strconv.Atoi(m[1])
			}
			a.File = firstNonEmpty(in.FilePath, a.File)
			a.About = firstNonEmpty(in.Description, a.About)
			a.Turn, a.At = t.N, st.End
		}
	}
	for i, j := 0, len(order)-1; i < j; i, j = i+1, j-1 {
		order[i], order[j] = order[j], order[i]
	}
	return order
}

// artifactsOf is artifacts, worked out again only when the session changes.
func (c *hostConn) artifactsOf() []*artifact {
	key := fmt.Sprint(c.sess.Last.UnixNano(), len(c.sess.Turns))
	if c.artsKey != key {
		c.arts, c.artsKey = artifacts(c.sess), key
	}
	return c.arts
}

// artifactLines is the artifacts view: each published page with what it
// is, its version and when; enter opens it in the browser.
func (m *Model) artifactLines(c *hostConn, o convo.Options) []convo.Line {
	w := o.Width
	arts := c.artifactsOf()
	var out []convo.Line
	line := func(text, ref string) { out = append(out, convo.Line{Text: fit(text, w), Ref: ref}) }
	line("  "+paint(cSub+bold, fmt.Sprintf("Artifacts  %d", len(arts)))+"   "+dim("pages this session published · enter opens one"), "")
	line("  "+faint(strings.Repeat("─", max(0, w-4))), "")
	for _, a := range arts {
		ref := "art:" + a.URL
		name := strings.TrimSuffix(filepath.Base(a.File), filepath.Ext(a.File))
		if name == "" || name == "." {
			name = a.URL[strings.LastIndex(a.URL, "/")+1:]
		}
		meta := fmt.Sprintf("#%d", a.Turn)
		if a.Version > 0 {
			meta = fmt.Sprintf("v%d · ", a.Version) + meta
		}
		if !a.At.IsZero() {
			meta += " · " + dur(o.Now.Sub(a.At)) + " ago"
		}
		top := spread("  "+paint(cBlue, "◆ ")+paint(cText+bold, name), dim(meta)+"  ", w)
		rows := []string{top, "      " + paint(cSub, ansi.Truncate(oneLine(a.About), max(10, w-10), "…")), "      " + faint(a.URL)}
		for _, r := range rows {
			if ref == o.Selected {
				bar := faint("▍")
				if o.Focused {
					bar = paint(cOrange, "▍")
				}
				r = selBG + strings.ReplaceAll(bar+fit(r, w)[1:], reset, reset+selBG) + reset
			}
			line(r, ref)
		}
		line("", "")
	}
	return out
}
