package ui

import (
	"errors"
	"net/url"
	"os/exec"
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/convo"
)

// linkAct is one thing the menu on a link can do with it.
type linkAct struct {
	label string
	do    func(m *Model) tea.Cmd
}

// previewable are the files Preview opens; the rest get Quick Look.
var previewable = map[string]bool{
	".png": true, ".jpg": true, ".jpeg": true, ".gif": true, ".webp": true, ".heic": true,
	".tif": true, ".tiff": true, ".bmp": true, ".pdf": true, ".psd": true, ".ico": true,
}

// linkAt is the URL of the link drawn at cell col of a styled row, or
// nothing where there's none.
func linkAt(s string, col int) string {
	at, u := 0, ""
	for {
		i := strings.Index(s, "\x1b]8;;")
		seg := s
		if i >= 0 {
			seg = s[:i]
		}
		w := cellw.String(ansi.Strip(seg))
		if u != "" && col >= at && col < at+w {
			return u
		}
		at += w
		if i < 0 || col < at {
			return ""
		}
		s = s[i+len("\x1b]8;;"):]
		j := strings.Index(s, "\x1b\\")
		if j < 0 {
			return ""
		}
		u, s = s[:j], s[j+2:]
	}
}

// pickedLink is the first file linked in the rows of the picked step:
// its own, and those under it with no step of their own, as its pictures.
func pickedLink(c *hostConn) string {
	in := false
	for _, r := range c.shown {
		if r.Ref != "" {
			in = r.Ref == c.sel
		}
		if !in {
			continue
		}
		for rest := r.Text; ; {
			i := strings.Index(rest, "\x1b]8;;")
			if i < 0 {
				break
			}
			rest = rest[i+len("\x1b]8;;"):]
			j := strings.Index(rest, "\x1b\\")
			if j < 0 {
				break
			}
			if u := rest[:j]; strings.HasPrefix(u, "file:") {
				return u
			}
			rest = rest[j:]
		}
	}
	return ""
}

// linkMenu opens the menu for the link under a right-click in the pane,
// reporting whether there was one.
func (m *Model) linkMenu(c *hostConn, x, y int) bool {
	i := y - m.paneTop
	if i < 0 || i >= len(c.rowBody) || c.rowBody[i] < 0 || m.viewName(c) == "screen" {
		return false
	}
	at, ok := m.textCell(c, x, y)
	if !ok || at.row >= len(c.shown) {
		return false
	}
	return m.openLinkMenu(linkAt(c.shown[at.row].Text, at.col))
}

// openLinkMenu opens the menu for a link, reporting whether it could.
func (m *Model) openLinkMenu(target string) bool {
	if target == "" {
		return false
	}
	acts := linkActs(target)
	if acts == nil {
		return false
	}
	title := target
	if u, err := url.Parse(target); err == nil && u.Scheme == "file" {
		title = tildify(u.Path)
	}
	m.picker = &picker{title: title, acts: acts}
	if u, err := url.Parse(target); err == nil && u.Scheme == "file" && previewable[strings.ToLower(filepath.Ext(u.Path))] {
		m.picker.img = u.Path
	}
	return true
}

// shotMsg is a menu's image drawn, for the menu on path at that size.
type shotMsg struct {
	path string
	big  bool
	rows []string
}

// pickerWide is whether the menu takes the window's width, for its image.
func (p *picker) pickerWide() bool { return p != nil && p.big && p.shot != nil }

// drawShot draws the menu's image off the UI goroutine: small, or with v
// as big as the window leaves room for and finer; nil when there's no
// image to draw.
func (m *Model) drawShot() tea.Cmd {
	p := m.picker
	if p == nil || p.img == "" {
		return nil
	}
	// The box's edges and padding, the title, the acts and the keys.
	w, h, path, big := min(m.w-10, 48), min(m.h-12-len(p.acts), 12), p.img, p.big
	if big {
		w, h = m.w-10, m.h-12-len(p.acts)
	}
	if w < 8 || h < 4 {
		return nil
	}
	return func() tea.Msg { return shotMsg{path, big, convo.Preview(path, w, h, big)} }
}

// linkActs are what can be done with a link: a file can be opened, in
// Preview when it's an image or PDF, looked at, found or copied; a web
// address opened or copied.
func linkActs(target string) []linkAct {
	u, err := url.Parse(target)
	if err != nil {
		return nil
	}
	if u.Scheme != "file" {
		return []linkAct{
			{"Open in browser", func(*Model) tea.Cmd { return browse(target) }},
			{"Copy link", func(m *Model) tea.Cmd { m.copyText(target); return nil }},
		}
	}
	p := u.Path
	acts := []linkAct{{"Open", func(*Model) tea.Cmd { return run("opened "+filepath.Base(p), "open", p) }}}
	if previewable[strings.ToLower(filepath.Ext(p))] {
		acts = append(acts, linkAct{"Open with Preview", func(*Model) tea.Cmd {
			return run("opened "+filepath.Base(p)+" in Preview", "open", "-a", "Preview", p)
		}})
	}
	return append(acts,
		linkAct{"Quick Look", func(*Model) tea.Cmd { return quickLook(p) }},
		linkAct{"Reveal in Finder", func(*Model) tea.Cmd { return run("revealed "+filepath.Base(p), "open", "-R", p) }},
		linkAct{"Copy path", func(m *Model) tea.Cmd { m.copyText(p); return nil }},
	)
}

// run runs a command, saying done when it's done or why it couldn't.
func run(done string, name string, args ...string) tea.Cmd {
	return func() tea.Msg {
		if out, err := exec.Command(name, args...).CombinedOutput(); err != nil {
			if s := strings.TrimSpace(string(out)); s != "" {
				return doneMsg{err: errors.New(s)}
			}
			return doneMsg{err: err}
		}
		return doneMsg{text: done}
	}
}

// quickLook shows a file in a Quick Look panel. qlmanage stays until the
// panel is closed, so it's left to run.
func quickLook(p string) tea.Cmd {
	return func() tea.Msg {
		cmd := exec.Command("qlmanage", "-p", p)
		if err := cmd.Start(); err != nil {
			return doneMsg{err: err}
		}
		go cmd.Wait()
		return nil
	}
}
