package ui

import (
	"fmt"
	"mime"
	"os"
	"os/exec"
	"strconv"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/photon/termimg"
	"github.com/0xdeafcafe/rush/internal/agent/event"
	"github.com/0xdeafcafe/rush/internal/convo"
	"github.com/charmbracelet/x/ansi"
)

// messageSheet reads a snapshot of complete user messages. Wrapping is cached;
// image decoding and disk reads happen off the UI goroutine.
type messageSheet struct {
	messages              []convo.UserMessage
	at, tab, scroll, page int
	textKey               string
	rows                  []string
	picture               []string
	strip                 []string // the images small, when there are several
	stripY                int      // the body line the strip starts on
	stripAt               [][2]int // each thumbnail's left edge, and the tab it shows
	flip                  bool     // the picture drawn the other way from the Pictures setting
}

// sharp is whether the picture shown is drawn sharp: as the Pictures
// setting says, unless s flipped it.
func (s *messageSheet) sharp() bool {
	return termimg.Current == termimg.Kitty && termimg.Drawn() != s.flip
}

func (s *messageSheet) width(m *Model) int { return max(40, m.w-6) }
func (s *messageSheet) images() []*event.ImageData {
	msg := s.messages[s.at]
	if len(msg.Pictures) > 0 {
		return msg.Pictures
	}
	out := make([]*event.ImageData, len(msg.Images))
	for i, p := range msg.Images {
		out[i] = &event.ImageData{Path: p}
	}
	return out
}

// stripRoom is what a message of several images gives its strip under the
// picture: a blank, the count, thumbnails three rows tall and a mark.
const stripRoom = 6

// prepare fits the image shown to w by h cells, as convo.Picture makes it
// off the UI goroutine, with a strip of all of them under it when there are
// several; until they're made, a Cmd that lands once they are.
func (s *messageSheet) prepare(w, h int) tea.Cmd {
	s.picture, s.strip, s.stripAt = nil, nil, nil
	if s.tab == 0 {
		return nil
	}
	pics := s.images()
	if s.tab > len(pics) {
		s.tab = 0
		return nil
	}
	var waits []<-chan struct{}
	if len(pics) > 1 {
		h -= stripRoom
		s.stripRow(pics, w, &waits)
	}
	rows, ready, made := convo.PictureDrawn(pics[s.tab-1], max(8, w), max(4, h), s.sharp())
	switch {
	case !ready:
		waits = append(waits, made)
	case len(rows) == 0:
		s.picture = []string{dim("Image preview unavailable. The original file may no longer exist.")}
	default:
		pad := strings.Repeat(" ", max(0, (w-ansi.StringWidth(rows[0]))/2))
		s.picture = make([]string, len(rows))
		for i, r := range rows {
			s.picture[i] = pad + r
		}
	}
	if len(waits) == 0 {
		return nil
	}
	return func() tea.Msg {
		for _, c := range waits {
			<-c
		}
		return sheetMsg{apply: func(*Model) tea.Cmd { return nil }}
	}
}

// stripRow lays the message's images out small in one row w wide, as many
// as fit around the one shown, each where a click shows it; one not made
// yet stands as its number.
func (s *messageSheet) stripRow(pics []*event.ImageData, w int, waits *[]<-chan struct{}) {
	const tw = 12
	n := max(1, min(len(pics), (w+2)/(tw+2)))
	from := max(0, min(s.tab-1-n/2, len(pics)-n))
	thumbs := make([][]string, n)
	for i := range n {
		rows, ready, made := convo.Picture(pics[from+i], tw, 3)
		if !ready {
			*waits = append(*waits, made)
		}
		if len(rows) == 0 {
			rows = []string{paint(cBlue, "▣ ") + paint(cText, fit(strconv.Itoa(from+i+1), tw-2))}
		}
		thumbs[i] = rows
	}
	rows, at := convo.Tile(thumbs, w, 2, nil)
	lead := max(0, (w-(at[n-1][0]+at[n-1][2]))/2)
	for i, r := range rows {
		rows[i] = strings.Repeat(" ", lead) + r
	}
	for len(rows) < 3 {
		rows = append(rows, "")
	}
	cur := at[s.tab-1-from]
	mark := strings.Repeat(" ", lead+cur[0]) + paint(cOrange, strings.Repeat("▔", cur[2]))
	s.strip = append(rows, mark)
	s.stripAt = make([][2]int, n)
	for i, a := range at {
		s.stripAt[i] = [2]int{lead + a[0], from + i + 1} // its left edge, and its tab
	}
}

func (s *messageSheet) body(m *Model, w, h int) []string {
	s.page = max(1, h-5)
	s.prepare(w, s.page)
	msg := s.messages[s.at]
	title := fmt.Sprintf("Message %d of %d", s.at+1, len(s.messages))
	tabs := []string{"Text"}
	for i := range s.images() {
		tabs = append(tabs, fmt.Sprintf("Image %d", i+1))
	}
	out := []string{sheetTitle(title, "complete sent text and attachments", w), messageTabs(tabs, s.tab, w), ""}
	var rows []string
	page := s.page
	hint := "↑↓ scroll · pgup/pgdown · [ ] messages · tab attachments · c copy · esc close"
	imageKeys := "o open original"
	if termimg.Current == termimg.Kitty {
		imageKeys = map[bool]string{true: "s pixelated", false: "s sharp"}[s.sharp()] + " · " + imageKeys
	}
	if s.tab == 0 {
		key := fmt.Sprintf("%d/%d", s.at, w)
		if key != s.textKey {
			s.textKey = key
			s.rows = strings.Split(ansi.Hardwrap(ansi.Strip(convo.ExpandedMessage(msg.Text)), max(1, w), true), "\n")
		}
		rows = s.rows
	} else {
		rows = s.picture
		if rows == nil {
			rows = []string{dim("Loading image…")}
		}
		if s.strip != nil {
			page -= stripRoom
		}
		hint = "↑↓ scroll · [ ] messages · tab attachments · " + imageKeys + " · c copy · esc close"
	}
	s.scroll = max(0, min(s.scroll, max(0, len(rows)-page)))
	out = append(out, rows[s.scroll:min(len(rows), s.scroll+page)]...)
	if s.strip != nil {
		for len(out) < 3+page {
			out = append(out, "")
		}
		count := fmt.Sprintf("%d / %d", s.tab, len(s.images()))
		out = append(out, "", strings.Repeat(" ", max(0, (w-len(count))/2))+paint(cText+bold, count))
		s.stripY = len(out)
		out = append(out, s.strip...)
		hint = "←→ images · click one to show it · [ ] messages · tab text · " + imageKeys + " · esc close"
	}
	return append(out, "", dim(hint))
}
func (s *messageSheet) key(m *Model, k tea.KeyPressMsg, key string) tea.Cmd {
	switch key {
	case "esc":
		m.sheet = nil
		return nil
	case "c":
		m.copyText(convo.ExpandedMessage(s.messages[s.at].Text))
	case "s":
		if s.tab > 0 && termimg.Current == termimg.Kitty {
			s.flip = !s.flip
		}
	case "o":
		if s.tab > 0 && s.tab <= len(s.images()) {
			return openImage(s.images()[s.tab-1])
		}
	case "up", "k":
		s.scroll = max(0, s.scroll-1)
	case "down", "j":
		s.scroll++
	case "pgup":
		s.scroll = max(0, s.scroll-max(1, s.page))
	case "pgdown", "space":
		s.scroll += max(1, s.page)
	case "home":
		s.scroll = 0
	case "end":
		s.scroll = int(^uint(0) >> 1)
	case "[", "ctrl+home":
		if key == "ctrl+home" {
			s.at = 0
		} else {
			s.at = max(0, s.at-1)
		}
		s.tab, s.scroll = 0, 0
	case "]", "ctrl+end":
		if key == "ctrl+end" {
			s.at = len(s.messages) - 1
		} else {
			s.at = min(len(s.messages)-1, s.at+1)
		}
		s.tab, s.scroll = 0, 0
	case "tab", "shift+tab":
		n := len(s.images()) + 1
		d := 1
		if key == "shift+tab" {
			d = -1
		}
		s.tab = (s.tab + d + n) % n
		s.scroll = 0
	case "left", "right":
		// Round the images: from the text, → is the first.
		if n := len(s.images()); n > 0 {
			d := 1
			if key == "left" {
				d = -1
			}
			if s.tab == 0 {
				s.tab = 1
			} else {
				s.tab = (s.tab-1+d+n)%n + 1
			}
			s.scroll = 0
		}
	}
	return s.prepare(m.sheetWidth()-4, max(1, m.h-11))
}
func (s *messageSheet) mouse(m *Model, ev mouseEv, x, y int) tea.Cmd {
	if ev == mousePress && y == 1 {
		col := 0
		labels := []string{"Text"}
		for i := range s.images() {
			labels = append(labels, fmt.Sprintf("Image %d", i+1))
		}
		for i, label := range labels {
			n := ansi.StringWidth(label)
			if x >= col && x < col+n {
				s.tab, s.scroll = i, 0
				return s.prepare(m.sheetWidth()-4, max(1, m.h-11))
			}
			col += n + 5
		}
	}
	if ev == mousePress && s.strip != nil && y >= s.stripY && y < s.stripY+len(s.strip) {
		for i := len(s.stripAt) - 1; i >= 0; i-- {
			if x >= s.stripAt[i][0] {
				s.tab, s.scroll = s.stripAt[i][1], 0
				return s.prepare(m.sheetWidth()-4, max(1, m.h-11))
			}
		}
	}
	switch ev {
	case mouseWheelUp:
		s.scroll = max(0, s.scroll-3)
	case mouseWheelDown:
		s.scroll += 3
	}
	return nil
}
func (m *Model) openUserMessage(c *hostConn, ref string) tea.Cmd {
	if c == nil || c.sess == nil {
		return nil
	}
	msgs := c.sess.UserMessages()
	if len(msgs) == 0 {
		return nil
	}
	at := len(msgs) - 1
	if ref == "" {
		ref = c.sel
	}
	turn, _, _ := strings.Cut(ref, ":")
	for i, msg := range msgs {
		if msg.Ref == turn {
			at = i
		}
		if msg.Ref == ref {
			at = i
			break
		}
	}
	s := &messageSheet{messages: msgs, at: at}
	m.sheet = s
	return nil
}
func (m *Model) jumpUserMessage(c *hostConn, last bool) {
	msgs := c.sess.UserMessages()
	if len(msgs) == 0 {
		return
	}
	at := 0
	if last {
		at = len(msgs) - 1
	}
	ref := msgs[at].Ref
	turn, _, _ := strings.Cut(ref, ":")
	if c.open == nil {
		c.open = map[string]bool{}
	}
	c.open[turn] = true
	m.showView(c, "conversation")
	c.sel, c.selMoved, c.scrollOnly, c.pinTop = ref, true, false, false
}

// Sent paste and image chips carry internal links, handled before text selection.
func (m *Model) clickMessage(c *hostConn, x, y int) (tea.Cmd, bool) {
	at, ok := m.textCell(c, x, y)
	if !ok || at.row >= len(c.shown) {
		return nil, false
	}
	target := linkAt(c.shown[at.row].Text, at.col)
	return m.messageLink(c, target)
}
func (m *Model) messageLink(c *hostConn, target string) (tea.Cmd, bool) {
	rest, ok := strings.CutPrefix(target, "rush:message/")
	if !ok {
		return nil, false
	}
	ref, number, image := strings.Cut(rest, "/image/")
	m.openUserMessage(c, ref)
	if s, ok := m.sheet.(*messageSheet); ok && image {
		n, _ := strconv.Atoi(number)
		if n > 0 && n <= len(s.images()) {
			s.tab = n
			return s.prepare(m.sheetWidth()-4, max(1, m.h-11)), true
		}
	}
	return nil, true
}
// openImage opens img in the system's viewer, at full size: its file, or
// one written for it when rush keeps no copy.
func openImage(img *event.ImageData) tea.Cmd {
	return func() tea.Msg {
		path := img.Path
		if path == "" {
			ext := ".png"
			if exts, _ := mime.ExtensionsByType(img.MediaType); len(exts) > 0 {
				ext = exts[len(exts)-1]
			}
			f, err := os.CreateTemp("", "rush-image-*"+ext)
			if err != nil {
				return doneMsg{err: fmt.Errorf("couldn't keep the image to open: %w", err)}
			}
			_, err = f.Write(img.Data)
			if cerr := f.Close(); err == nil {
				err = cerr
			}
			if err != nil {
				return doneMsg{err: fmt.Errorf("couldn't keep the image to open: %w", err)}
			}
			path = f.Name()
		}
		if err := exec.Command("open", path).Run(); err != nil {
			return doneMsg{err: fmt.Errorf("couldn't open the image: %w", err)}
		}
		return doneMsg{text: "opened the image"}
	}
}

func messageTabs(names []string, selected, w int) string {
	out := make([]string, len(names))
	for i, n := range names {
		if i == selected {
			out[i] = paint(cOrange+bold, n)
		} else {
			out[i] = dim(n)
		}
	}
	return ansi.Truncate(strings.Join(out, "  ·  ")+dim("   tab"), w, "…")
}

// Clicking a draft image opens it without changing the draft or its selection.
func (m *Model) draftImage(x, y int) tea.Cmd {
	which, pos := m.boxAt(x, y)
	var buf []rune
	var refs imageRefs
	if which == 1 && m.host != nil {
		buf, refs = m.host.input, m.host.imgs
	} else if which == 2 {
		buf, refs = m.input, m.imgs
	} else {
		return nil
	}
	sp, ok := chipOn(buf, pos)
	if !ok {
		return nil
	}
	marker := string(buf[sp.from:sp.to])
	if !imageMarkerRe.MatchString(marker) {
		return nil
	}
	_, paths := refs.resolve(marker)
	if len(paths) == 0 {
		return nil
	}
	s := &messageSheet{messages: []convo.UserMessage{{Images: paths}}, tab: 1}
	m.sheet = s
	return s.prepare(m.sheetWidth()-4, max(1, m.h-11))
}
