package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"strings"
	"syscall"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/photon/termimg"
	"github.com/0xdeafcafe/rush/internal/advisor"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agtools"
	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/instances"
	"github.com/0xdeafcafe/rush/internal/menubar"
	"github.com/0xdeafcafe/rush/internal/plugin"
	"github.com/0xdeafcafe/rush/internal/plugind"
	"github.com/0xdeafcafe/rush/internal/state"
	"github.com/0xdeafcafe/rush/internal/statusline"
	"github.com/0xdeafcafe/rush/internal/ui"
	"github.com/0xdeafcafe/rush/internal/update"
)

var version = "0.1.0"

// A go install'd rush knows which one it is.
func init() {
	if i, ok := update.Current(); ok {
		version = i.Short()
	}
}

const usage = `rush — a lighter agents view for Claude Code

  rush             open the view
  rush on          make "claude agents" open this view (adds one line to your shell rc)
  rush off         give "claude agents" back to Claude Code (instant, no shell reload)
  rush status      show whether it is on
  rush update      install the newest rush, with go install
  rush reload      reload every running rush view in place, as #reload does
  rush menubar     put rush in the menu bar: usage, what's working, and
                    questions you can answer from their notification
  rush menubar off take it out again
  rush plugin      sandboxed plugins: list, approve, revoke
  rush gate        queue intensive programs agents run: run, status
  rush feed        the feed agents share, once on ("rush feed help")
  rush room        a group chat of fresh agents arguing a topic to a verdict
                    ("rush room help")
  rush session     start, send to, stop and list rush-mode sessions without
                    the view ("rush session help" for the commands)
  rush open <id> --hosted
                    the view of one rush-mode session alone, full width
  rush serve       this machine's sessions to a browser and to rush elsewhere (docs/remote.md)
  rush --dump      print what the view sees, for debugging
`

func main() {
	termimg.Current = termimg.Detect(os.Getenv, "RUSH_GRAPHICS")
	args := os.Args[1:]
	if len(args) > 0 {
		switch args[0] {
		case "--version", "version":
			fmt.Println("rush", version)
			return
		case "--help", "-h", "help":
			fmt.Print(usage + pluginsUsage())
			return
		case "--dump":
			dump()
			return
		case "--render":
			render(args[1:])
			return
		case "--soak":
			profiled(func() { soak(args[1:]) })
			return
		case "attach":
			if len(args) < 2 {
				exitIf(errors.New("usage: rush attach <id>"))
			}
			j, ok := agent.As[agent.Joiner](agent.Kind(state.LoginsKind))
			if !ok {
				exitIf(errors.New("no agent here opens a session's own screen"))
			}
			exitIf(j.Join(state.Load().Config.ActiveAccount().Profile(), args[1]).Run())
			return
		case "host":
			// rush host run <id>: the detached process a rush-mode session
			// lives in. rush starts it; it is not meant to be run by hand.
			if len(args) < 3 || args[1] != "run" {
				exitIf(errors.New("usage: rush host run <id>"))
			}
			background("") // it lowers GOGC itself once it runs turns
			var err error
			profiled(func() { err = host.Run(args[2]) })
			exitIf(err)
			return
		case "feed", "twotter", "twatter", "twitter", "twattr", "community": // its old names still work
			os.Exit(communityCmd(args[1:], os.Stdin, os.Stdout, os.Stderr))
		case "room", "rooms":
			os.Exit(roomCmd(args[1:], os.Stdin, os.Stdout, os.Stderr))
		case "session", "sessions":
			os.Exit(sessionCmd(args[1:], os.Stdin, os.Stdout, os.Stderr))
		case "serve":
			exitIf(serveCmd(args[1:]))
			return
		case "remote":
			exitIf(remoteCmd(args[1:], os.Stdout))
			return
		case "inbox":
			// A session's hook after each tool call: not meant to be run
			// by hand.
			if len(args) > 1 {
				exitIf(host.Inbox(args[1], os.Stdin, os.Stdout))
			}
			return
		case "spawn":
			// What a session's stand-in for codex or claude runs: not meant
			// to be run by hand.
			os.Exit(spawnCmd(args[1:]))
		case "open":
			exitIf(openHosted(args[1:]))
			return
		case "menubar":
			exitIf(menuBar(args[1:]))
			return
		case "mcp-tools":
			// rush's own tools, for a session's agent that runs MCP servers
			// as processes: not meant to be run by hand.
			id := ""
			if len(args) > 2 && args[1] == "--session" {
				id = args[2]
			}
			// The agents it starts find the real programs, not the session's stand-ins.
			_ = os.Setenv("PATH", host.WithoutShims(os.Getenv("PATH")))
			exitIf(agtools.Serve(os.Stdin, os.Stdout, agtools.Handler(host.AgentTools(id))))
			return
		case "plugin", "plugins":
			exitIf(pluginCmd(args[1:]))
			return
		case "gate":
			os.Exit(gateCmd(args[1:]))
		case "plugind":
			// The plugin broker. rush starts it when a plugin is approved;
			// it is not meant to be run by hand.
			background("")
			exitIf(plugind.Run())
			return
		case "statusline":
			// Claude Code's statusLine command, set up by /statusline in a
			// Session: the session's JSON in, one line out.
			exitIf(statusline.Run(os.Stdin, os.Stdout))
			return
		case "reload":
			fmt.Printf("reloaded %d rush view(s)\n", instances.Reload(0))
			return
		case "update":
			exitIf(selfUpdate())
			return
		case "on":
			exitIf(turnOn())
			return
		case "off":
			exitIf(os.WriteFile(offFlag(), nil, 0o600))
			fmt.Println(`off — "claude agents" opens the native view. "rush on" turns it back on.`)
			return
		case "status":
			if isOn() {
				fmt.Println(`on — "claude agents" opens rush`)
			} else {
				fmt.Println(`off — "claude agents" opens the native view`)
			}
			return
		}
		// A plugin's own commands: its name, where rush's commands take
		// theirs first.
		if m, ok := plugin.CLIPlugins()[args[0]]; ok {
			os.Exit(pluginCLI(args[0], m, args[1:], os.Stdout, os.Stderr))
		}
	}
	// 120 frames a second: a streamed delta reaches the terminal within
	// about 8ms of being drawn, and nothing is drawn when nothing changed.
	if pid, ok := instances.Lock(); !ok {
		fmt.Fprintf(os.Stderr, "rush is already running (pid %d) on %s: one runs at a time, as it switches logins and restarts sessions for all of them. Use that one, or quit it first.\n", pid, state.Dir())
		os.Exit(1)
	}
	viewGC()
	state.WriteBehind() // the UI goroutine never waits on a save
	agent.NeverWait()   // nor on looking for agents' programs
	p := tea.NewProgram(ui.New(state.Load(), version), tea.WithFPS(120))
	// So the menu bar app comes back to this terminal: noted while the
	// first frame draws, not before it.
	here := make(chan func(), 1)
	// SIGUSR1 is `rush reload`: #reload, asked for from outside. Only a view
	// that's listed is signalled, and it's listed once this is set.
	reloads := make(chan os.Signal, 1)
	signal.Notify(reloads, syscall.SIGUSR1)
	go func() {
		for range reloads {
			p.Send(ui.ReloadMsg())
		}
	}()
	unlist := instances.Register()
	go func() { here <- menubar.Here() }()
	var err error
	var last tea.Model
	profiled(func() { last, err = p.Run() })
	ui.StopSchemeReports()
	// Unlisted before a reload's exec, which starts with SIGUSR1 unhandled.
	unlist()
	advisor.Stop() // a pass still running would spend on an answer nobody reads
	_ = state.Flush()
	(<-here)()
	// #reload: the installed rush in this one's place, the terminal already
	// given back; the sessions' hosts never stopped.
	if m, ok := last.(*ui.Model); ok && err == nil {
		if env, ok := m.Reload(); ok {
			if exe, xerr := os.Executable(); xerr == nil {
				_ = syscall.Exec(exe, os.Args, append(os.Environ(), env)) // only returns if it failed
			}
		}
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "rush:", err)
		os.Exit(1)
	}
}

// background sets up one of rush's own long-lived processes (a session's
// host, the menu bar feed, the plugin broker): one runs per agent, so each
// should cost a few MB. They mostly wait, so two Ps is plenty: fewer
// threads, and less cached per P. The runtime only takes that as it starts
// (set later it costs more than it saves), so the process starts again
// with it, once, and with gogc when that's set. Heap profiling is off
// unless asked for.
func background(gogc string) {
	const mark = "RUSH_GOENV"
	if set, ok := os.LookupEnv(mark); ok {
		// Not for Claude Code, or anything it runs.
		for _, k := range strings.Fields(set) {
			_ = os.Unsetenv(k)
		}
		_ = os.Unsetenv(mark)
	} else {
		var env, set []string
		for _, kv := range [][2]string{{"GOMAXPROCS", "2"}, {"GOGC", gogc}} {
			if os.Getenv(kv[0]) == "" && kv[1] != "" {
				env, set = append(env, kv[0]+"="+kv[1]), append(set, kv[0])
			}
		}
		if exe, err := os.Executable(); err == nil && len(set) > 0 {
			env = append(append(os.Environ(), env...), mark+"="+strings.Join(set, " "))
			_ = syscall.Exec(exe, os.Args, env) // only returns if it failed
		}
	}
	if os.Getenv("RUSH_MEMPROFILE") == "" {
		runtime.MemProfileRate = 0
	}
}

// viewGC tunes the collector for the view. Most of the time it holds
// 10-20 MB and allocates a few MB a second, so collecting at half again the
// live heap rather than double keeps some 8 MB less resident for about a
// collection a second more. There's no memory limit: with a few long
// sessions open the live heap passes any fixed one, and past it the
// collector runs without pause, on up to half the machine's cores. Memory
// goes back when a big session is let go (freeSoon, in the ui).
func viewGC() {
	if os.Getenv("GOGC") == "" {
		debug.SetGCPercent(50)
	}
}

func exitIf(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, "rush:", err)
		os.Exit(1)
	}
}

func offFlag() string { return filepath.Join(state.Dir(), "off") }

func isOn() bool {
	_, err := os.Stat(offFlag())
	if err == nil {
		return false
	}
	b, _ := os.ReadFile(rcFile())
	return strings.Contains(string(b), hookLine())
}

func hookPath() string { return filepath.Join(state.Dir(), "shell.sh") }
func hookLine() string { return `[ -f "` + hookPath() + `" ] && . "` + hookPath() + `"` }

func rcFile() string {
	home, _ := os.UserHomeDir()
	if strings.HasSuffix(os.Getenv("SHELL"), "bash") {
		return filepath.Join(home, ".bashrc")
	}
	return filepath.Join(home, ".zshrc")
}

// The hook only intercepts "claude agents"; every other claude call passes
// through untouched, and the off flag is read per call so toggling is instant.
const hook = `# added by "rush on" — remove with "rush off" or delete this file
claude() {
  if [ "$1" = "agents" ] && [ ! -f "%s" ] && command -v rush >/dev/null 2>&1; then
    shift
    command rush "$@"
  else
    command claude "$@"
  fi
}
`

func turnOn() error {
	if err := os.MkdirAll(state.Dir(), 0o700); err != nil {
		return err
	}
	if err := os.WriteFile(hookPath(), []byte(fmt.Sprintf(hook, offFlag())), 0o644); err != nil {
		return err
	}
	_ = os.Remove(offFlag())
	rc := rcFile()
	b, _ := os.ReadFile(rc)
	if !strings.Contains(string(b), hookLine()) {
		f, err := os.OpenFile(rc, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
		if err != nil {
			return err
		}
		defer f.Close()
		if _, err := f.WriteString("\n" + hookLine() + "\n"); err != nil {
			return err
		}
		fmt.Printf("on — added one line to %s. Open a new terminal (or: . %s)\n", rc, hookPath())
		return nil
	}
	fmt.Println(`on — "claude agents" opens rush`)
	return nil
}

func dump() {
	st := state.Load()
	l := fleet.NewLoader(st)
	snap := l.Load(false)
	var targets []fleet.Target
	for _, a := range snap.Agents {
		if a.TranscriptPath != "" {
			targets = append(targets, fleet.Target{Key: a.Key, Path: a.TranscriptPath, Live: a.Live() || a.PID != 0})
		}
	}
	sc := fleet.NewScanner()
	t0 := time.Now()
	l.SetSpend(sc.Run(targets))
	sc.Flush()
	scan := time.Since(t0)
	l.Load(true)
	time.Sleep(time.Second)
	snap = l.Load(true)
	now := time.Now()
	for _, a := range snap.Agents {
		fmt.Printf("%-8s %-8s %-40.40s %6.1f%% %7.0fM %8.2f %9s %6s %d prs\n", a.ID, a.State, a.DisplayName,
			a.CPU, float64(a.Mem)/(1<<20), a.Spend.Cost, a.Elapsed(now).Round(time.Minute), a.Age(now).Round(time.Second), len(a.PRs))
	}
	for _, av := range snap.Accounts {
		fmt.Printf("account %s daemon=%v live=%d agents=%d spend=$%.2f today=$%.2f",
			av.Name, av.Daemon, av.Live, av.Agents, av.Spend, av.Today)
		for _, w := range av.Quota.Windows {
			fmt.Printf(" %s=%.0f%%", w.Label, w.Percent)
		}
		fmt.Println()
	}
	for _, r := range snap.Machine.Rows {
		fmt.Printf("proc %6d role=%d %-40.40s %5.1f%% %7.0fM n=%d\n", r.PID, r.Role, r.Label, r.CPU, float64(r.Mem)/(1<<20), r.Procs)
	}
	fmt.Printf("machine mem=%.0fM cpu=%.1f%% spares=%d scan=%s\n", float64(snap.Machine.TotalMem)/(1<<20), snap.Machine.TotalCPU, snap.Machine.Spares, scan)
}

// render prints one frame, e.g. rush --render 160x45 tab
func render(args []string) {
	w, h := 160, 45
	if len(args) > 0 {
		fmt.Sscanf(args[0], "%dx%d", &w, &h)
	}
	var keys []tea.KeyPressMsg
	for _, k := range args[min(1, len(args)):] {
		switch k {
		case "tab":
			keys = append(keys, tea.KeyPressMsg{Code: tea.KeyTab})
		case "down":
			keys = append(keys, tea.KeyPressMsg{Code: tea.KeyDown})
		case "enter":
			keys = append(keys, tea.KeyPressMsg{Code: tea.KeyEnter})
		case "?":
			keys = append(keys, tea.KeyPressMsg{Code: '?', Text: "?"})
		default:
			if t, ok := strings.CutPrefix(k, "text="); ok {
				for _, r := range t {
					keys = append(keys, tea.KeyPressMsg{Code: r, Text: string(r)})
				}
				continue
			}
			if strings.HasPrefix(k, "ctrl+") {
				keys = append(keys, tea.KeyPressMsg{Code: rune(k[5]), Mod: tea.ModCtrl})
			}
			if strings.HasPrefix(k, "alt+") {
				keys = append(keys, tea.KeyPressMsg{Code: rune(k[4]), Mod: tea.ModAlt})
			}
		}
	}
	fmt.Println(ui.New(state.Load(), version).Frame(w, h, keys...))
}

// menuBar runs the menu bar icon, or its feed (which the icon runs).
func menuBar(args []string) error {
	st := state.Load()
	switch {
	case len(args) > 0 && args[0] == "feed":
		// It reloads everything every couple of seconds, a few MB of
		// garbage over a small live heap: collecting sooner keeps less of it.
		background("50")
		return menubar.Feed(os.Stdin, os.Stdout)
	case len(args) > 0 && args[0] == "off":
		st.Config.MenuBar, st.Config.MenuBarAsked = false, true
		menubar.Stop()
		menubar.Forget()
		fmt.Println("off — the menu bar icon is gone")
		return st.SaveConfig()
	case len(args) > 0:
		return errors.New("usage: rush menubar [off]")
	}
	fmt.Println("starting the menu bar icon (the first time builds it, a few seconds)…")
	if err := menubar.Start(); err != nil {
		return err
	}
	st.Config.MenuBar, st.Config.MenuBarAsked = true, true
	fmt.Println("on — rush is in your menu bar. Its menu can open it at login.")
	return st.SaveConfig()
}

// selfUpdate is rush update: the newest rush over this one, when there is
// a newer one.
func selfUpdate() error {
	ctx := context.Background()
	l, err := update.Latest(ctx)
	if err != nil {
		return err
	}
	if cur, ok := update.Current(); ok && !update.Newer(ctx, cur, l) {
		fmt.Println("rush", cur.Short(), "is the newest")
		return nil
	}
	fmt.Println("installing rush", l.Short()+"…")
	to, err := update.Install(ctx)
	if err != nil {
		return err
	}
	fmt.Println("rush", to.Short(), "installed · reopen rush to use it")
	return nil
}
