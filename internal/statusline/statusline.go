// Package statusline is rush's status line for Claude Code: the layout
// you build in rush's /statusline, and `rush statusline`, the command
// Claude Code runs to draw it (session JSON on stdin, one line out).
package statusline

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Layout is which segments show on which line, in order, and how they're
// joined.
type Layout struct {
	Lines [][]string `json:"lines"`
	Sep   string     `json:"sep"`
	Plain bool       `json:"plain,omitzero"` // no colour
	// Custom is a status line command of your own, drawn by the "custom"
	// segment: the one rush's replaced, kept.
	Custom string `json:"custom,omitempty"`
}

// MaxLines is how many lines the builder offers.
const MaxLines = 3

// Default is what a new status line starts as.
func Default() Layout {
	return Layout{Lines: [][]string{{"model", "folder", "branch", "context", "cost"}}, Sep: " · "}
}

// Clone is a copy whose lines can be changed without touching l's.
func (l Layout) Clone() Layout {
	c := l
	c.Lines = make([][]string, len(l.Lines))
	for i, ln := range l.Lines {
		c.Lines[i] = append([]string{}, ln...)
	}
	return c
}

// Shown says whether a segment is on any line.
func (l Layout) Shown(id string) bool {
	for _, ln := range l.Lines {
		for _, s := range ln {
			if s == id {
				return true
			}
		}
	}
	return false
}

// Seps are the separators the builder offers.
var Seps = []string{" · ", " │ ", "  ", " / ", " ▸ "}

// Segment is one thing the line can show.
type Segment struct {
	ID, Name, About string
	draw            func(in Input, x *extra) (text, color string)
}

// Input is what Claude Code sends the status line command.
type Input struct {
	SessionID string `json:"session_id"`
	Cwd       string `json:"cwd"`
	Model     struct {
		ID          string `json:"id"`
		DisplayName string `json:"display_name"`
	} `json:"model"`
	Effort struct {
		Level string `json:"level"`
	} `json:"effort"`
	Workspace struct {
		CurrentDir string `json:"current_dir"`
		ProjectDir string `json:"project_dir"`
		Repo       *struct {
			Owner string `json:"owner"`
			Name  string `json:"name"`
		} `json:"repo"`
	} `json:"workspace"`
	Version     string `json:"version"`
	OutputStyle struct {
		Name string `json:"name"`
	} `json:"output_style"`
	Cost struct {
		USD          float64 `json:"total_cost_usd"`
		DurationMS   float64 `json:"total_duration_ms"`
		LinesAdded   int     `json:"total_lines_added"`
		LinesRemoved int     `json:"total_lines_removed"`
	} `json:"cost"`
	Context struct {
		Input   int      `json:"total_input_tokens"`
		Size    int      `json:"context_window_size"`
		UsedPct *float64 `json:"used_percentage"`
	} `json:"context_window"`
	Fast bool `json:"fast_mode"`

	raw []byte // as it came, for a command of your own
}

// extra is what the line knows beyond the input: found out before it's
// drawn, so drawing never waits.
type extra struct {
	custom    string
	configDir string
	branch    string
	usage     *usage.Quota
	acct      string
	now       time.Time
	own       string // your own command's output
}

const (
	orange = "\x1b[38;2;217;119;87m"
	blue   = "\x1b[38;2;122;162;247m"
	green  = "\x1b[38;2;130;190;120m"
	red    = "\x1b[38;2;224;108;117m"
	yellow = "\x1b[38;2;229;192;123m"
	grey   = "\x1b[38;2;168;162;152m"
	dim    = "\x1b[38;2;122;117;108m"
	reset  = "\x1b[0m"
)

func level(p float64) string {
	switch {
	case p >= 85:
		return red
	case p >= 60:
		return yellow
	}
	return green
}

var paren = regexp.MustCompile(`\s*\(.*\)$`)

// Segments is every segment, in the order the builder lists them.
var Segments = []Segment{
	{"model", "Model", "the model this session runs", func(in Input, _ *extra) (string, string) {
		return ModelName(firstOf(in.Model.DisplayName, in.Model.ID)), orange
	}},
	{"effort", "Effort", "the effort level", func(in Input, _ *extra) (string, string) {
		return in.Effort.Level, grey
	}},
	{"folder", "Folder", "the folder it works in", func(in Input, _ *extra) (string, string) {
		d := firstOf(in.Workspace.CurrentDir, in.Cwd)
		if d == "" {
			return "", ""
		}
		return filepath.Base(d), blue
	}},
	{"repo", "Repository", "owner/name of the git remote", func(in Input, _ *extra) (string, string) {
		if r := in.Workspace.Repo; r != nil && r.Name != "" {
			return r.Owner + "/" + r.Name, blue
		}
		return "", ""
	}},
	{"branch", "Git branch", "the branch checked out", func(in Input, x *extra) (string, string) {
		if b := x.branch; b != "" {
			return "⎇ " + b, green
		}
		return "", ""
	}},
	{"context", "Context", "how full the context window is", func(in Input, _ *extra) (string, string) {
		p := -1.0
		if in.Context.UsedPct != nil {
			p = *in.Context.UsedPct
		} else if in.Context.Size > 0 && in.Context.Input > 0 {
			p = 100 * float64(in.Context.Input) / float64(in.Context.Size)
		}
		if p < 0 {
			return "", ""
		}
		return fmt.Sprintf("ctx %.0f%%", p), level(p)
	}},
	{"cost", "Cost", "what the session has cost so far", func(in Input, _ *extra) (string, string) {
		if in.Cost.USD <= 0 {
			return "", ""
		}
		return fmt.Sprintf("$%.2f", in.Cost.USD), yellow
	}},
	{"duration", "Time", "how long the session has run", func(in Input, _ *extra) (string, string) {
		if in.Cost.DurationMS <= 0 {
			return "", ""
		}
		return short(time.Duration(in.Cost.DurationMS) * time.Millisecond), grey
	}},
	{"lines", "Lines changed", "lines added and removed", func(in Input, _ *extra) (string, string) {
		if in.Cost.LinesAdded+in.Cost.LinesRemoved == 0 {
			return "", ""
		}
		return fmt.Sprintf("%s+%d%s %s−%d", green, in.Cost.LinesAdded, reset, red, in.Cost.LinesRemoved), ""
	}},
	{"usage", "Plan usage", "the 5-hour and weekly limits, as rush last read them", func(_ Input, x *extra) (string, string) {
		u := x.usage
		if u == nil {
			return "", ""
		}
		var parts []string
		for _, w := range u.Windows {
			parts = append(parts, fmt.Sprintf("%s%s %.0f%%%s", level(w.Percent), w.Label, w.Percent, reset))
		}
		return strings.Join(parts, " "), ""
	}},
	{"account", "Account", "which Claude account this is", func(_ Input, x *extra) (string, string) {
		return x.acct, grey
	}},
	{"style", "Output style", "the output style, when it isn't the default", func(in Input, _ *extra) (string, string) {
		if in.OutputStyle.Name == "" || in.OutputStyle.Name == "default" {
			return "", ""
		}
		return in.OutputStyle.Name, grey
	}},
	{"fast", "Fast mode", "“fast” while fast mode is on", func(in Input, _ *extra) (string, string) {
		if !in.Fast {
			return "", ""
		}
		return "fast", yellow
	}},
	{"version", "Version", "Claude Code's version", func(in Input, _ *extra) (string, string) {
		if in.Version == "" {
			return "", ""
		}
		return "v" + in.Version, dim
	}},
	{"clock", "Clock", "the time of day", func(_ Input, x *extra) (string, string) {
		return x.now.Format("15:04"), dim
	}},
	{"session", "Session id", "the conversation's short id", func(in Input, _ *extra) (string, string) {
		if len(in.SessionID) < 8 {
			return "", ""
		}
		return in.SessionID[:8], dim
	}},
	{"custom", "Your own line", "what your own status line command prints", func(in Input, x *extra) (string, string) {
		if x.custom == "" {
			return "", ""
		}
		return x.own, ""
	}},
}

// ModelName is how a model is shown: "Opus 5.5" for claude-opus-5-5[1m].
func ModelName(s string) string { return agent.ModelName(agent.Kind(state.LoginsKind), s) }

// runOwn runs your own status line command with the session on stdin,
// for at most two seconds.
func runOwn(cmd string, in Input) string {
	raw := in.raw
	if raw == nil {
		raw, _ = jsonx.Marshal(in)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	c := exec.CommandContext(ctx, "sh", "-c", cmd)
	c.Stdin = bytes.NewReader(raw)
	if st, err := os.Stat(in.Cwd); err == nil && st.IsDir() {
		c.Dir = in.Cwd
	}
	out, _ := c.Output()
	return strings.TrimRight(string(out), "\n ")
}

// Find is the segment with id.
func Find(id string) (Segment, bool) {
	for _, s := range Segments {
		if s.ID == id {
			return s, true
		}
	}
	return Segment{}, false
}

// Render draws the lines: on each, the segments that have something to
// say, joined. A segment that prints several lines (your own command)
// adds the rest as lines of their own. What's slow (the branch, plan
// usage, the account's name, your own command) is found out first, for
// the segments shown.
func Render(in Input, l Layout, configDir string, now time.Time) string {
	x := &extra{configDir: configDir, now: now, custom: l.Custom}
	if l.Shown("branch") {
		x.branch = gitBranch(firstOf(in.Workspace.CurrentDir, in.Cwd))
	}
	if l.Shown("usage") {
		x.usage = planUsage(configDir)
	}
	if l.Shown("account") {
		x.acct = accountName(configDir)
	}
	if l.Custom != "" && l.Shown("custom") {
		x.own = runOwn(l.Custom, in)
	}
	return render(in, l, x)
}

// Renderer draws lines over and over, as the /statusline preview does many
// times a second. It never waits: what's slow to find out (the branch,
// plan usage, the account's name) it looks up off to the side, at most
// once a second, and your own command runs off to the side too. Until
// they're in, their segments show nothing, and then the last found.
type Renderer struct {
	mu                 sync.Mutex
	facts              facts // what was last looked up, for factsDir and factsCwd
	factsDir, factsCwd string
	factsAt            time.Time
	looking            bool
	own, ownKey        string
	ownAt              time.Time
	running            bool
}

// facts are what's slow to find out: git's branch, the plan's usage and
// the account's name.
type facts struct {
	branch string
	usage  *usage.Quota
	acct   string
}

// Render is Render, with what's slow looked up off to the side.
func (r *Renderer) Render(in Input, l Layout, configDir string, now time.Time) string {
	r.mu.Lock()
	if r.factsDir != configDir || r.factsCwd != in.Cwd {
		r.facts, r.factsDir, r.factsCwd, r.factsAt = facts{}, configDir, in.Cwd, time.Time{}
	}
	if !r.looking && now.Sub(r.factsAt) > time.Second {
		r.looking = true
		go r.lookUp(configDir, in.Cwd, firstOf(in.Workspace.CurrentDir, in.Cwd))
	}
	f := r.facts
	r.mu.Unlock()
	x := extra{configDir: configDir, custom: l.Custom, now: now, branch: f.branch, usage: f.usage, acct: f.acct}
	if l.Custom != "" {
		x.own = r.ownLine(l.Custom, in)
	}
	return render(in, l, &x)
}

// lookUp finds the facts for configDir and cwd, off the UI.
func (r *Renderer) lookUp(configDir, cwd, dir string) {
	f := facts{branch: gitBranch(dir), usage: planUsage(configDir), acct: accountName(configDir)}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.looking = false
	if r.factsDir == configDir && r.factsCwd == cwd {
		r.facts, r.factsAt = f, time.Now()
	}
}

// ownLine is your own command's last output, starting it again when that
// is a couple of seconds old.
func (r *Renderer) ownLine(cmd string, in Input) string {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.ownKey != cmd {
		r.own, r.ownKey, r.ownAt = "", cmd, time.Time{}
	}
	if !r.running && time.Since(r.ownAt) > 2*time.Second {
		r.running = true
		go func() {
			out := runOwn(cmd, in)
			r.mu.Lock()
			defer r.mu.Unlock()
			if r.ownKey == cmd {
				r.own, r.ownAt = out, time.Now()
			}
			r.running = false
		}()
	}
	return r.own
}

func render(in Input, l Layout, x *extra) string {
	sep := l.Sep
	if sep == "" {
		sep = Default().Sep
	}
	var out []string
	for _, ids := range l.Lines {
		var parts, more []string
		for _, id := range ids {
			s, ok := Find(id)
			if !ok {
				continue
			}
			text, col := s.draw(in, x)
			if text == "" {
				continue
			}
			first, rest, _ := strings.Cut(text, "\n")
			if rest != "" {
				more = append(more, strings.Split(rest, "\n")...)
			}
			if first == "" {
				continue
			}
			if col != "" {
				first = col + first + reset
			}
			parts = append(parts, first)
		}
		if len(parts) > 0 {
			out = append(out, strings.Join(parts, dim+sep+reset))
		}
		out = append(out, more...)
	}
	s := strings.Join(out, "\n")
	if l.Plain {
		s = stripSGR(s)
	}
	return s
}

var sgr = regexp.MustCompile("\x1b\\[[0-9;]*m")

func stripSGR(s string) string { return sgr.ReplaceAllString(s, "") }

func gitBranch(dir string) string {
	if dir == "" {
		return ""
	}
	out, err := exec.Command("git", "-C", dir, "branch", "--show-current").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// planUsage is the limits of the account the folder is signed in to, as
// rush last read them.
func planUsage(configDir string) *usage.Quota {
	var q usage.Quota
	k := agent.Kind(state.LoginsKind)
	if lq, ok := agent.As[agent.LastQuotaReader](k); ok {
		q, _ = lq.LastQuota(agent.Profile{Kind: k, Dir: configDir})
	}
	return &q
}

// accountName is the login the config folder at dir is signed in as.
func accountName(dir string) string {
	var id string
	k := agent.Kind(state.LoginsKind)
	if r, ok := agent.As[agent.SignInReader](k); ok {
		id = r.SignedInAs(agent.Profile{Kind: k, Dir: dir})
	}
	for _, l := range state.Load().Config.Logins {
		if l.ID == id && id != "" {
			return l.Name
		}
	}
	return strings.TrimPrefix(filepath.Base(dir), ".claude-")
}

func short(d time.Duration) string {
	switch {
	case d >= time.Hour:
		return fmt.Sprintf("%dh%02dm", int(d.Hours()), int(d.Minutes())%60)
	case d >= time.Minute:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	}
	return fmt.Sprintf("%ds", int(d.Seconds()))
}

func firstOf(xs ...string) string {
	for _, x := range xs {
		if x != "" {
			return x
		}
	}
	return ""
}

// Path is where the layout is kept.
func Path() string { return filepath.Join(state.Dir(), "statusline.json") }

// Load reads the layout, or Default.
func Load() Layout {
	l := Default()
	if b, err := os.ReadFile(Path()); err == nil {
		var got Layout
		if jsonx.Unmarshal(b, &got) == nil && got.Lines != nil {
			l = got
		}
	}
	return l
}

// Save writes the layout.
func Save(l Layout) error {
	b, err := jsonx.MarshalIndent(l)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(Path()), 0o700); err != nil {
		return err
	}
	return os.WriteFile(Path(), append(b, '\n'), 0o600)
}

// Run is `rush statusline`: read the session from stdin, print the line.
func Run(stdin io.Reader, stdout io.Writer) error {
	raw, err := io.ReadAll(stdin)
	if err != nil {
		return err
	}
	var in Input
	if err := jsonx.Unmarshal(raw, &in); err != nil {
		return err
	}
	in.raw = raw
	var dir string
	if p, ok := agent.As[agent.StatusLineProfiler](agent.Kind(state.LoginsKind)); ok {
		dir = p.StatusLineProfile().Dir
	}
	_, err = fmt.Fprint(stdout, Render(in, Load(), dir, time.Now()))
	return err
}

// Command is the statusLine command rush writes into settings.json.
func Command() string {
	bin, err := exec.LookPath("rush")
	if err != nil {
		if bin, err = os.Executable(); err != nil {
			bin = "rush"
		}
	}
	return shellQuote(bin) + " statusline"
}

// Ours says whether a statusLine command is rush's.
func Ours(cmd string) bool {
	return strings.HasSuffix(strings.TrimSpace(cmd), "rush statusline") || strings.HasSuffix(strings.TrimSpace(cmd), "rush' statusline")
}

func shellQuote(s string) string {
	if !strings.ContainsAny(s, " '\"\\$`!*?[](){}<>|&;#~") {
		return s
	}
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}
