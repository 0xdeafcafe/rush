package ui

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	tea "charm.land/bubbletea/v2"
	"rsc.io/qr"

	"github.com/0xdeafcafe/rush/internal/hooks"
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
	on      map[string]bool // serve's plugins, by name
	cfg     remote.Config
	devs    []remote.Device
	devErr  error // serve didn't answer
}

func remoteNow() (mode bool, on map[string]bool, cfg remote.Config, devs []remote.Device, devErr error) {
	remoteSnap.Lock()
	read := time.Since(remoteSnap.at) > 2*time.Second && !remoteSnap.reading
	remoteSnap.reading = remoteSnap.reading || read
	remoteSnap.Unlock()
	if read {
		goOff(func() {
			mode := remote.RemoteOn()
			cfg, _ := remote.LoadConfig()
			on := map[string]bool{}
			for _, n := range []string{remote.PluginRemote, remote.PluginTailscale, remote.PluginCloudflared, remote.PluginSSH} {
				on[n] = plugin.BundledOn(n)
			}
			devs, err := remote.LocalDevices(context.Background())
			remoteSnap.Lock()
			remoteSnap.at, remoteSnap.reading, remoteSnap.mode, remoteSnap.on, remoteSnap.cfg = time.Now(), false, mode, on, cfg
			remoteSnap.devs, remoteSnap.devErr = devs, err
			remoteSnap.Unlock()
		})
	}
	remoteSnap.Lock()
	defer remoteSnap.Unlock()
	return remoteSnap.mode, remoteSnap.on, remoteSnap.cfg, remoteSnap.devs, remoteSnap.devErr
}

// remoteChanged has the next frame read the switches again.
func remoteChanged() {
	remoteSnap.Lock()
	remoteSnap.at = time.Time{}
	remoteSnap.Unlock()
}

func onOff(b bool) string { return map[bool]string{true: "on", false: "off"}[b] }

// switchRow is one of remote.json's switches as a setting: serve reads
// it again within a few seconds.
func switchRow(label string, on bool, set func(*remote.Config, bool), what, ifOn, ifOff string) setting {
	return setting{
		label: label, value: onOff(on), choices: []string{"off", "on"}, unset: "off",
		what: what, means: map[string]string{"on": ifOn, "off": ifOff},
		run: func(v string) tea.Cmd {
			return func() tea.Msg {
				err := remote.UpdateConfig(func(c *remote.Config) { set(c, v == "on") })
				remoteChanged()
				return doneMsg{text: strings.ToLower(label) + " " + v, err: err}
			}
		},
	}
}

// serveSwitch is one of serve's plugins as a setting: the same switch as
// Settings › Plugins, which serve reads again within a few seconds.
func serveSwitch(label, name string, on bool, what, ifOn, ifOff string, done func(on bool) string) setting {
	return setting{
		label: label, value: onOff(on), choices: []string{"off", "on"}, unset: "off",
		what: what + " (the " + name + " plugin)", means: map[string]string{"on": ifOn, "off": ifOff},
		run: func(v string) tea.Cmd {
			return func() tea.Msg {
				err := plugin.SetBundled(name, v == "on")
				if err == nil {
					err = hooks.Reload()
				}
				remote.SwitchesChanged()
				remoteChanged()
				text := strings.ToLower(label) + " " + v
				if done != nil {
					text = done(v == "on")
				}
				return doneMsg{text: text, err: err}
			}
		},
	}
}

func (m *Model) remoteSections() []section {
	modeOn, on, cfg, devs, devErr := remoteNow()
	mode := serveSwitch("Remote mode", remote.PluginRemote, on[remote.PluginRemote] || modeOn,
		"Keeps rush serve running from login on, even with rush closed, reached and let in as the rows below say: your sessions on your phone or another device, at home or not",
		"serve runs at login and after a crash, reached as the rows below say.",
		"serve is stopped and its addresses go; your sessions are only on this machine.",
		func(on bool) string {
			if on {
				return "remote mode on · Pair a device when it's up"
			}
			return "remote mode off"
		})
	tailnet := serveSwitch("Over Tailscale", remote.PluginTailscale, on[remote.PluginTailscale],
		"Devices signed in to your tailnet reach serve, and nothing else does",
		"https on port 8443, and SSH on 2222 with the terminal on.", "nothing on your tailnet reaches serve.", nil)
	tunnel := serveSwitch("Through Cloudflare", remote.PluginCloudflared, on[remote.PluginCloudflared],
		"Serve's web app is reached through your own Cloudflare tunnel, its token in cloudflared-token in rush's folder",
		"the web app is at your tunnel's address.", "the tunnel is stopped.", nil)
	web := switchRow("Web app", cfg.Web, func(c *remote.Config, on bool) { c.Web = on },
		"The app your phone and browsers open, signed in by pairing",
		"it's served; a device signs in with a pairing code.", "only rush on your other machines can use serve.")
	ssh := serveSwitch("Terminal (SSH)", remote.PluginSSH, on[remote.PluginSSH],
		"Rush itself, the whole terminal app, from any SSH app (Termius, Blink, ssh) with a paired key. Like tmux, with no tmux: each device has its own rush, kept 30 minutes after it drops",
		"reconnect and it's as you left it.", "serve's SSH stops, and with it each device's rush.", nil)
	pair := setting{
		label: "Pair a device", value: "a QR code for a phone, a code for SSH",
		about: func() (string, string, string) {
			return "Pair a device", "Shows a QR code for your phone's camera: it opens the app and signs it in. With SSH on, the same code pairs an SSH key: connect, and type it when asked. A code works once, within 10 minutes. The device needs Tailscale signed in to your tailnet. On an iPhone, Share › Add to Home Screen afterwards, for notifications too.", ""
		},
		key: func(s string) (tea.Cmd, bool) {
			if s != "enter" {
				return nil, false
			}
			return m.openPairSheet(), true
		},
		keys: []string{"enter", "pair a device"},
	}
	serve := paint(cGreen, "running")
	switch {
	case devErr != nil && modeOn:
		serve = paint(cOrange, "starting, or not answering") + dim(" · "+remote.ServiceLog())
	case devErr != nil:
		serve = dim("not running")
	default:
		live := 0
		for _, d := range devs {
			live += min(d.Live, 1)
		}
		serve += dim(fmt.Sprintf(" · %d paired, %d connected now", len(devs), live))
	}
	status := setting{
		label: "Serve", value: serve,
		about: func() (string, string, string) {
			return "Serve", "Whether rush serve is up on this machine, and how many devices are paired with it and connected now. Its log is " + remote.ServiceLog() + ".", ""
		},
	}
	sections := []section{
		{title: "Remote", note: "your sessions from your phone and other devices", rows: []setting{mode, status, tailnet, tunnel, web, ssh, pair}},
	}
	if len(devs) > 0 {
		var rows []setting
		for _, d := range devs {
			rows = append(rows, m.deviceRow(d))
		}
		sections = append(sections, section{title: "Devices", note: "paired with this machine · x unpairs one", rows: rows})
	}
	if len(cfg.Peers) > 0 {
		sections = append(sections, section{title: "Other machines", note: "added with rush remote add", rows: []setting{
			switchRow("Their sessions here", cfg.AttachPeers, func(c *remote.Config, on bool) { c.AttachPeers = on },
				"Lists the sessions of the machines you've added in this rush, as rush remote attach does, kept by serve with no terminal of its own",
				"they list here as name @machine.", "only this machine's sessions list here."),
		}})
	}
	return sections
}

// unpairing is the device x was pressed on once: a second x unpairs it.
var unpairing string

// deviceRow is a paired device: what it is, when it was last seen, and x
// twice to unpair it, which ends what it has open.
func (m *Model) deviceRow(d remote.Device) setting {
	icon := map[string]string{"web": "◰", "ssh": "❯"}[d.Kind]
	seen := paint(cGreen, "● connected")
	if d.Live == 0 {
		seen = dim("seen " + seenAgo(d.LastSeen))
	}
	if unpairing == d.ID {
		seen = paint(cRed, "x again to unpair")
	}
	return setting{
		label: icon + " " + d.Name, value: seen,
		about: func() (string, string, string) {
			what := map[string]string{"web": "A browser signed in with a pairing code or the token: its cookie is its own.",
				"ssh": "An SSH key, paired with a code: it gets in to rush with no password. " + d.Fingerprint}[d.Kind]
			return d.Name, what + " Paired " + d.Created.Local().Format("2 Jan 15:04") + ".", ""
		},
		key: func(s string) (tea.Cmd, bool) {
			if s != "x" {
				unpairing = ""
				return nil, false
			}
			if unpairing != d.ID {
				unpairing = d.ID
				return nil, true
			}
			unpairing = ""
			id, name := d.ID, d.Name
			return func() tea.Msg {
				err := remote.LocalRevoke(context.Background(), id)
				remoteChanged()
				return doneMsg{text: "unpaired " + name, err: err}
			}, true
		},
		keys: []string{"x", "unpair"},
	}
}

// seenAgo is how long ago t was, as people say it.
func seenAgo(t time.Time) string {
	d := time.Since(t)
	switch {
	case t.IsZero():
		return "never"
	case d < time.Minute:
		return "just now"
	case d < time.Hour:
		return fmt.Sprintf("%dm ago", int(d.Minutes()))
	case d < 48*time.Hour:
		return fmt.Sprintf("%dh ago", int(d.Hours()))
	}
	return t.Local().Format("2 Jan")
}

// pairSheet shows a code that pairs a device with this machine's serve:
// as a QR code for a phone's camera, and typed for an SSH key. It watches
// for the device to turn up, and says so when it does.
type pairSheet struct {
	p      remote.Pairing
	err    error
	opened time.Time
	paired *remote.Device
}

func (m *Model) openPairSheet() tea.Cmd {
	s := &pairSheet{opened: time.Now()}
	m.sheet = s
	return tea.Batch(sheetDo(func() (remote.Pairing, error) { return remote.LocalPairing(context.Background()) },
		func(m *Model, p remote.Pairing, err error) tea.Cmd {
			s.p, s.err = p, err
			return nil
		}), s.watch())
}

// watch looks for a device paired since the sheet opened, every 2s while
// it's open.
func (s *pairSheet) watch() tea.Cmd {
	return tea.Tick(2*time.Second, func(time.Time) tea.Msg {
		devs, err := remote.LocalDevices(context.Background())
		return sheetMsg{apply: func(m *Model) tea.Cmd {
			if m.sheet != s || s.paired != nil {
				return nil
			}
			for _, d := range devs {
				if err == nil && d.Created.After(s.opened) {
					s.paired = &d
					remoteChanged()
					return nil
				}
			}
			return s.watch()
		}}
	})
}

func (s *pairSheet) width(*Model) int { return 72 }

func (s *pairSheet) body(m *Model, w, h int) []string {
	out := []string{sheetTitle("Pair a device", "a phone's camera, or an SSH key", w), ""}
	switch {
	case s.paired != nil:
		out = append(out, "", center(paint(cGreen, "✓ Paired"), w), "",
			center(s.paired.Name, w), "", center(dim("It's under Devices in Settings › Remote."), w), "")
		return append(out, keysFit(w, "enter", "pair another", "esc", "done"))
	case s.err != nil:
		out = append(out, paint(cRed, "rush serve didn't answer: "+s.err.Error()), "",
			dim("Turn Remote mode on, give it a few seconds, and try again."))
	case s.p.Code == "":
		out = append(out, dim("asking rush serve for a code…"))
	default:
		code := remote.PairCode(s.p.Code)
		ssh, or := []string{}, "or "
		if s.p.URL == "" {
			or = ""
		}
		if s.p.SSH != "" {
			ssh = []string{"", center(dim(or+"from an SSH app:  ")+s.p.SSH, w),
				center(dim("then type the code when it asks"), w), center(dim("this Mac's key: "+s.p.HostKey), w)}
		}
		qrs := []string{}
		switch {
		case s.p.URL == "":
			remoteSnap.Lock()
			local := remoteSnap.cfg.Reach == remote.ReachLocal
			remoteSnap.Unlock()
			why, fix := "Not on your tailnet yet,", "sign in to Tailscale on this Mac."
			if local {
				why, fix = "Serve is reached from this Mac only,", "turn on Over Tailscale to pair one."
			}
			qrs = []string{center(paint(cOrange, why+" so no QR code for a phone:"), w), center(dim(fix), w)}
		case len(qrLines(s.p.URL))+len(ssh)+7 > h:
			qrs = []string{dim("Make the window taller for the QR code; meanwhile open"), s.p.URL}
		default:
			for _, l := range qrLines(s.p.URL) {
				qrs = append(qrs, center(l, w))
			}
		}
		out = append(out, qrs...)
		out = append(out, ssh...)
		left := int(max(0, time.Until(s.p.Expires).Round(time.Minute)).Minutes())
		out = append(out, "", center(dim("Code ")+paint(cOrange, code)+dim(fmt.Sprintf(" · works once, for %d min · waiting for the device…", left)), w))
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
// the three finder eyes in its orange (dark enough to read as dark), a
// quiet zone of cream, and the rush bottle in the middle, one module to
// its pixel, which the code's error correction reads past.
func qrLines(text string) []string {
	c, err := qr.Encode(text, qr.H)
	if err == nil && c.Size >= 45 {
		// From version 7 an alignment pattern sits in the middle, under the
		// bottle: a little less correction keeps the code smaller and clear.
		c, err = qr.Encode(text, qr.Q)
	}
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
	bw, bh := len(qrBottle[0])+2, len(qrBottle)+2 // with a module of cream round it
	bx, by := (c.Size-bw)/2, (c.Size-bh)/2
	rgb := func(x, y int) string {
		x, y = x-quiet, y-quiet
		if x >= bx && x < bx+bw && y >= by && y < by+bh {
			if px, py := x-bx-1, y-by-1; py >= 0 && py < len(qrBottle) && px >= 0 && px < len(qrBottle[py]) {
				if ink, ok := qrInk[qrBottle[py][px]]; ok {
					return ink
				}
			}
			return qrLight
		}
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

// qrBottle is the header's bottle (clanker.go) a pixel to a module: its
// cap, neck and shoulders, the yellow label with a red band for RUSH, and
// its body.
var qrBottle = []string{
	"GKGKGK",
	"KGKGKG",
	"kkkkkk",
	".dAAa.",
	"dAAAad",
	"YYYYYY",
	"YRRRRY",
	"YYYYYY",
	"dAAAad",
	"dDDDDd",
}

// qrInk is the bottle's colours, as clkInk's, r;g;b.
var qrInk = map[byte]string{
	'K': "64;64;68", 'G': "112;112;116", 'k': "88;88;92",
	'A': "154;76;20", 'a': "212;128;44", 'd': "104;46;8", 'D': "84;38;6",
	'Y': "250;214;30", 'R': "214;28;24",
}

// The QR code's colours, as r;g;b: fixed, not moved onto the terminal's
// ground, so it scans the same on a light one.
const (
	qrLight = "240;236;228"
	qrDark  = "30;28;26"
	qrEye   = "190;92;60"
)
