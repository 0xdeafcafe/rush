package ui

import (
	"context"
	"os"
	"strings"
	"sync"
	"time"

	tea "charm.land/bubbletea/v2"
	"rsc.io/qr"

	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/remote"
)

// Settings, Remote: this machine's sessions from your phone and your other
// machines (docs/remote.md). Remote mode keeps rush serve running from login
// on, reached over your tailnet; Pair a phone shows a QR code that signs the
// phone in once, so it's ready before you leave.

func remotePage() page {
	return page{name: "Remote", form: (*Model).remoteSections}
}

// remoteSnap is the switches as last read, off the UI: a frame never reads
// the disk, and the one after a reading lands shows it.
var remoteSnap struct {
	sync.Mutex
	at      time.Time
	reading bool
	mode    bool
	on      map[string]bool
}

func remoteNow() (mode bool, on map[string]bool) {
	remoteSnap.Lock()
	read := time.Since(remoteSnap.at) > 2*time.Second && !remoteSnap.reading
	remoteSnap.reading = remoteSnap.reading || read
	remoteSnap.Unlock()
	if read {
		goOff(func() {
			mode, on := remote.RemoteOn(), plugin.BundlesOn()
			remoteSnap.Lock()
			remoteSnap.at, remoteSnap.reading, remoteSnap.mode, remoteSnap.on = time.Now(), false, mode, on
			remoteSnap.Unlock()
		})
	}
	remoteSnap.Lock()
	defer remoteSnap.Unlock()
	return remoteSnap.mode, remoteSnap.on
}

// remoteChanged has the next frame read the switches again.
func remoteChanged() {
	remoteSnap.Lock()
	remoteSnap.at = time.Time{}
	remoteSnap.Unlock()
}

func onOff(b bool) string { return map[bool]string{true: "on", false: "off"}[b] }

// switchRow is one of serve's plugin switches as a setting.
func switchRow(label, name string, on map[string]bool, what, ifOn, ifOff string) setting {
	return setting{
		label: label, value: onOff(on[name]), choices: []string{"off", "on"}, unset: "off",
		what: what + " (the " + name + " plugin)", means: map[string]string{"on": ifOn, "off": ifOff},
		run: func(v string) tea.Cmd {
			return func() tea.Msg {
				err := plugin.SetBundled(name, v == "on")
				remoteChanged()
				return doneMsg{text: strings.ToLower(label) + " " + v, err: err}
			}
		},
	}
}

func (m *Model) remoteSections() []section {
	modeOn, on := remoteNow()
	mode := setting{
		label: "Remote mode", value: onOff(modeOn), choices: []string{"off", "on"}, unset: "off",
		what: "Keeps rush serve running from login on, even with rush closed, and turns on Tailscale and the web app below: your sessions open in a browser on any device signed in to your tailnet, at home or not.",
		means: map[string]string{
			"on":  "serve runs at login and after a crash, at https://<this Mac>.<tailnet>.ts.net:8443, tailnet only.",
			"off": "serve is stopped and its tailnet address goes; your sessions are only on this machine.",
		},
		run: func(v string) tea.Cmd {
			on := v == "on"
			return func() tea.Msg {
				exe, err := os.Executable()
				if err == nil {
					err = remote.SetRemote(on, exe)
				}
				remoteChanged()
				text := "remote mode off"
				if on {
					text = "remote mode on · Pair a phone when it's up"
				}
				return doneMsg{text: text, err: err}
			}
		},
	}
	reach := []setting{
		switchRow("Over Tailscale", remote.PluginTailscale, on,
			"Puts serve on your tailnet with the installed Tailscale, on https port 8443",
			"devices signed in to your tailnet reach it; nothing else does.", "serve's tailnet mapping is removed."),
		switchRow("Over Cloudflare", remote.PluginCloudflared, on,
			"Reaches serve through your own Cloudflare tunnel, with the token in cloudflared-token in rush's folder",
			"cloudflared runs the tunnel while serve runs.", "only that tunnel is stopped."),
	}
	clients := []setting{
		switchRow("Web app", remote.PluginWeb, on,
			"Serves the app your phone and browsers open",
			"the app is served; it signs in with a paired code or the token.", "only rush on your other machines can use serve."),
		switchRow("Other machines here", remote.PluginTUI, on,
			"Shows the sessions of the machines in remote.json (rush remote add) in this rush, as rush remote attach does, kept by serve with no terminal of its own",
			"their sessions list here as name @machine.", "only this machine's sessions list here."),
	}
	pair := setting{
		label: "pair",
		line:  func(w int) string { return fit("Pair a phone", 30) + " " + dim("enter shows a QR code") },
		about: func() (string, string, string) {
			return "Pair a phone", "Shows a QR code for your phone's camera: it opens the app and signs it in, once, within 10 minutes. The phone needs the Tailscale app signed in to your tailnet. On an iPhone, Share › Add to Home Screen afterwards, for notifications too.", ""
		},
		key: func(s string) (tea.Cmd, bool) {
			if s != "enter" {
				return nil, false
			}
			return m.openPairSheet(), true
		},
		keys: []string{"enter", "show the QR code"},
	}
	return []section{
		{title: "Remote", note: "your sessions from your phone, over Tailscale", rows: []setting{mode, pair}},
		{title: "Reached", note: "how other devices get to serve", rows: reach},
		{title: "Clients", note: "what serve serves", rows: clients},
	}
}

// pairSheet shows the QR code that pairs a phone with this machine's serve.
type pairSheet struct {
	p   remote.Pairing
	err error
}

func (m *Model) openPairSheet() tea.Cmd {
	m.sheet = &pairSheet{}
	return sheetDo(func() (remote.Pairing, error) { return remote.LocalPairing(context.Background()) },
		func(m *Model, p remote.Pairing, err error) tea.Cmd {
			if s, ok := m.sheet.(*pairSheet); ok {
				s.p, s.err = p, err
			}
			return nil
		})
}

func (s *pairSheet) width(*Model) int { return 72 }

func (s *pairSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle("Pair a phone", "point its camera at the code", w), ""}
	switch {
	case s.err != nil:
		out = append(out, paint(cRed, "rush serve didn't answer: "+s.err.Error()), "",
			dim("Turn Remote mode on, give it a few seconds, and try again."))
	case s.p.Code == "":
		out = append(out, dim("asking rush serve for a code…"))
	case s.p.URL == "":
		out = append(out, paint(cRed, "rush serve isn't on your tailnet yet."), "",
			dim("Turn Remote mode on (or the tailscale plugin), and sign in to Tailscale on this Mac."))
	default:
		code := qrLines(s.p.URL)
		if len(code)+6 > h {
			out = append(out, paint(cRed, "Make the window taller to show the code."))
			break
		}
		for _, l := range code {
			out = append(out, center(l, w))
		}
		left := time.Until(s.p.Expires).Round(time.Minute)
		out = append(out, "", center(dim("Works once, for "+left.String()+" · "+strings.SplitN(s.p.URL, "/#", 2)[0]), w))
	}
	return append(out, "", keysFit(w, "enter", "a new code", "esc", "close"))
}

func (s *pairSheet) key(m *Model, _ tea.KeyPressMsg, k string) tea.Cmd {
	switch k {
	case "esc", "ctrl+c", "q":
		m.sheet = nil
	case "enter", "r":
		return m.openPairSheet()
	}
	return nil
}

// qrLines draws text as a QR code, two modules to a line with half blocks,
// in rush's colours whatever the terminal's: its dark ground on its cream,
// with the three finder eyes in its orange (dark enough to read as dark), and
// a quiet zone of cream.
func qrLines(text string) []string {
	c, err := qr.Encode(text, qr.L)
	if err != nil {
		return []string{paint(cRed, err.Error())}
	}
	const quiet = 2
	n := c.Size + 2*quiet
	eye := func(x, y int) bool { // the 3×3 middles of the finder patterns
		in := func(v, at int) bool { return v >= at+2 && v <= at+4 }
		far := c.Size - 7
		return in(x, 0) && in(y, 0) || in(x, far) && in(y, 0) || in(x, 0) && in(y, far)
	}
	rgb := func(x, y int) string {
		x, y = x-quiet, y-quiet
		switch {
		case !c.Black(x, y):
			return qrLight
		case eye(x, y):
			return qrEye
		}
		return qrDark
	}
	var out []string
	for y := 0; y < n; y += 2 {
		var b strings.Builder
		for x := range n {
			b.WriteString("\x1b[38;2;" + rgb(x, y) + ";48;2;" + rgb(x, y+1) + "m▀")
		}
		b.WriteString("\x1b[0m")
		out = append(out, b.String())
	}
	return out
}

// The QR code's colours, as r;g;b: fixed, not moved onto the terminal's
// ground, so it scans the same on a light one.
const (
	qrLight = "240;236;228"
	qrDark  = "30;28;26"
	qrEye   = "190;92;60"
)
