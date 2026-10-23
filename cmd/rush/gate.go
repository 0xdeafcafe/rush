package main

import (
	"context"
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	bgate "github.com/0xdeafcafe/rush/internal/bundled/gate"
	"github.com/0xdeafcafe/rush/internal/bundled/prwatch"
	"github.com/0xdeafcafe/rush/internal/events"
	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/proc"
	"github.com/0xdeafcafe/rush/internal/script"
	"github.com/0xdeafcafe/rush/internal/watch"
)

const gateUsage = `rush gate — share the machine between intensive programs your agents run (the gate plugin)

  rush gate run [--name N] [--dir D] -- cmd args…
                    run cmd once there's memory for it, and watch it
  rush gate shim <name> args…
                    what the gate's shims run: <name> off PATH, gated
  rush gate hold --ready F --pid P [--dir D] -- script args…
                    what the node preload runs: gates node process P
                    running script, saying so in F, and watches it
  rush gate status  what runs and what waits, and why, and the shims and
                    node preload brought up to date
  rush gate hook    Claude Code's PreToolUse hook: a gated Bash call runs
                    under rush gate run

Turn it on with "rush plugin on gate"; its settings are in Settings, Plugins.
`

func gateCmd(args []string) int {
	if len(args) == 0 {
		fmt.Print(gateUsage)
		return 0
	}
	// The real program is found past the shims, so a shim never runs itself.
	_ = os.Setenv("PATH", withoutDir(os.Getenv("PATH"), gate.BinDir()))
	switch args[0] {
	case "run":
		name, _, argv, err := gateRunArgs(args[1:]) // memory is the machine's: where it runs doesn't matter
		if err != nil {
			fmt.Fprintf(os.Stderr, "rush gate: %v\nusage: rush gate run [--name N] [--dir D] -- cmd args…\n", err)
			return 2
		}
		return gateRun(name, argv)
	case "shim":
		if len(args) < 2 {
			fmt.Fprintln(os.Stderr, "usage: rush gate shim <name> args…")
			return 2
		}
		return gateRun(args[1], args[1:])
	case "hold":
		return gateHold(args[1:])
	case "status":
		// The plugin's own process runs with its data folder for a home, so
		// its shims land there: the ones sessions find are written here, as a
		// session's start writes them.
		bgate.WriteShims()
		gateStatus(os.Stdout)
		return 0
	case "hook":
		gateHook(os.Stdin, os.Stdout)
		return 0
	}
	fmt.Fprint(os.Stderr, gateUsage)
	return 2
}

func gateRunArgs(args []string) (name, dir string, argv []string, err error) {
	for len(args) > 0 {
		switch a := args[0]; {
		case a == "--":
			if len(args) < 2 {
				return "", "", nil, errors.New("no command")
			}
			return name, dir, args[1:], nil
		case (a == "--name" || a == "--dir") && len(args) > 1:
			if a == "--name" {
				name = args[1]
			} else {
				dir = args[1]
			}
			args = args[2:]
		case strings.HasPrefix(a, "-"):
			return "", "", nil, fmt.Errorf("unknown flag %s", a)
		default:
			return name, dir, args, nil
		}
	}
	return "", "", nil, errors.New("no command")
}

// gateRun runs argv once the gate lets it, and is its exit code. Without
// a rule for it, with the gate off, or inside a gated program, it just runs.
func gateRun(name string, argv []string) int {
	path, err := exec.LookPath(argv[0])
	if err != nil {
		fmt.Fprintln(os.Stderr, "rush gate:", err)
		return 127
	}
	if name == "" {
		name = filepath.Base(argv[0])
	}
	var r gate.Rule
	ok := false
	if os.Getenv(gate.HeldEnv) == "" && !gate.HeldFor(os.Getpid()) {
		r, ok = bgate.Rules()[name]
	}
	// A shim's run is judged by its words; the hook judged a wrapped one.
	if ok && filepath.Base(argv[0]) == name && !gate.Heavy(name, argv[1:]) {
		ok = false
	}
	if !ok {
		events.Flush()
		err := syscall.Exec(path, argv, os.Environ())
		fmt.Fprintln(os.Stderr, "rush gate:", err)
		return 126
	}
	own := gateOwner()
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	run, err := own.acquire(ctx, gate.Ask{Name: name, Rule: r, Session: own.key, Reserve: bgate.Reserve()}, true)
	stop()
	if err != nil {
		if ctx.Err() != nil {
			return 130
		}
		fmt.Fprintln(os.Stderr, "rush gate:", err)
		return 1
	}
	defer run.Release()
	cmd := exec.Command(path)
	cmd.Args, cmd.Env = argv, append(os.Environ(), gate.HeldEnv+"=system")
	cmd.Env = append(cmd.Env, gate.WorkerEnv(name, argv)...)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = os.Stdin, os.Stdout, os.Stderr
	sigs := make(chan os.Signal, 2)
	signal.Notify(sigs, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(sigs)
	if err := cmd.Start(); err != nil {
		fmt.Fprintln(os.Stderr, "rush gate:", err)
		return 126
	}
	go func() {
		for s := range sigs {
			_ = cmd.Process.Signal(s)
			run.Wake()
		}
	}()
	watching, done := context.WithCancel(context.Background())
	watched := make(chan struct{})
	go func() {
		defer close(watched)
		run.Watch(watching, own.watch(cmd.Process.Pid, func(s string) { fmt.Fprintln(os.Stderr, s) }))
	}()
	_ = cmd.Wait()
	done()
	<-watched
	if ws, ok := cmd.ProcessState.Sys().(syscall.WaitStatus); ok && ws.Signaled() {
		return 128 + int(ws.Signal())
	}
	return cmd.ProcessState.ExitCode()
}

// owner is who a gated run is for: its rush session, if any, and the
// agent's program above it, if any.
type owner struct {
	session  string
	agentPID int
	key      string // the agent, for the fair order
}

// gateOwner is who the run this process holds is for.
func gateOwner() owner {
	o := owner{session: os.Getenv("RUSH_SESSION")}
	tab := proc.Snapshot(nil)
	for i, pid := 0, os.Getppid(); i < 64 && pid > 1; i++ {
		p := tab.Procs[pid]
		if p == nil {
			break
		}
		if agent.IsProgram(p.Comm) {
			o.agentPID = pid
			break
		}
		pid = p.PPID
	}
	switch {
	case o.session != "":
		o.key = o.session
	case o.agentPID > 0:
		o.key = "pid " + strconv.Itoa(o.agentPID)
	default:
		o.key = "pid " + strconv.Itoa(os.Getppid())
	}
	return o
}

// log records an event of the run's: in its session's log, else rush's
// with the agent it's for.
func (o owner) log(kind string, f map[string]any) {
	if o.session != "" {
		host.LogEvent(o.session, kind, f)
		return
	}
	f["agent"] = o.key
	events.Log(kind, f)
}

// acquire is gate.Acquire, logging each change while it waits, the run
// let in, and a failure; tell says why it waits on stderr too.
func (o owner) acquire(ctx context.Context, a gate.Ask, tell bool) (*gate.Run, error) {
	start := time.Now()
	waited := false
	run, err := gate.Acquire(ctx, a, func(w gate.Wait) {
		waited = true
		o.log("gate-waiting", map[string]any{"name": a.Name, "ahead": w.Ahead, "running": w.Running, "need": w.Need, "room": w.Room})
		if tell {
			tellWait(a.Name, w)
		}
	})
	switch {
	case err == nil:
		f := map[string]any{"name": a.Name, "memory": a.Rule.Memory}
		if waited {
			f["waited"] = time.Since(start).Round(time.Second).String()
		}
		o.log("gate-admitted", f)
	case ctx.Err() != nil:
		if waited {
			o.log("gate-gave-up", map[string]any{"name": a.Name, "why": "its waiting was stopped", "waited": time.Since(start).Round(time.Second).String()})
		}
	default:
		o.log("gate-failed", map[string]any{"name": a.Name, "err": err.Error()})
	}
	return run, err
}

// watch is what the watchdog needs to look after root's run: echo, if
// set, hears what the agent is told, as well as its inbox.
func (o owner) watch(root int, echo func(string)) gate.Watch {
	w := gate.Watch{Root: root, AgentPID: o.agentPID, Log: o.log, Floor: bgate.Floor()}
	if o.session != "" {
		w.SessionAlive = func() bool { return host.SessionAlive(o.session) }
	}
	w.Tell = func(s string) {
		if echo != nil {
			echo(s)
		}
		if o.session != "" {
			err := host.Notice(o.session, s)
			f := map[string]any{"text": clipText(s)}
			if err != nil {
				f["err"] = err.Error()
			}
			o.log("gate-told", f)
		}
	}
	return w
}

// The guard tells an agent, and logs in its session, why its program was
// stopped; with no session, in rush's own log.
func init() {
	bgate.Tell = func(session, text string) { _ = host.Notice(session, text) }
	bgate.Log = func(kind string, f map[string]any) {
		if s, _ := f["session"].(string); s != "" {
			host.LogEvent(s, kind, f)
			return
		}
		events.Log(kind, f)
	}
}

// tellWait says on stderr why name waits.
func tellWait(name string, w gate.Wait) {
	if w.Need > 0 {
		fmt.Fprintf(os.Stderr, "rush gate: %s waiting for memory: it needs %s, there's %s past what's kept free (%d running)…\n",
			name, gate.Size(w.Need), gate.Size(w.Room), w.Running)
		return
	}
	fmt.Fprintf(os.Stderr, "rush gate: %s waiting its turn: %d ahead, the agents running least first (%d running)…\n", name, w.Ahead, w.Running)
}

// gateHold is the node preload's holder: it says in --ready whether node
// process --pid, running a script, is gated, then holds and watches its
// run until that process is gone. It answers "free" when the script isn't
// gated, or can't be judged; the preload runs it unheld then, as without a
// gate.
func gateHold(args []string) int {
	var ready string
	pid := 0
	for len(args) > 0 && args[0] != "--" {
		if len(args) < 2 {
			return 2
		}
		switch args[0] {
		case "--ready":
			ready = args[1]
		case "--dir": // memory is the machine's: where it runs doesn't matter now
		case "--pid":
			pid, _ = strconv.Atoi(args[1])
		default:
			return 2
		}
		args = args[2:]
	}
	if len(args) > 0 {
		args = args[1:] // the "--"
	}
	if ready == "" || pid <= 0 || len(args) < 2 {
		if ready != "" {
			_ = os.WriteFile(ready, []byte("free\n"), 0o600)
		}
		return 2
	}
	// It beats while it decides and waits, so the preload knows it's alive.
	beating, beat := make(chan struct{}), make(chan struct{})
	go func() {
		defer close(beat)
		for {
			now := time.Now()
			if os.Chtimes(ready+".beat", now, now) != nil {
				_ = os.WriteFile(ready+".beat", nil, 0o600)
			}
			select {
			case <-beating:
				return
			case <-time.After(time.Second):
			}
		}
	}()
	said := false
	say := func(lines ...string) {
		if said {
			return
		}
		said = true
		close(beating)
		<-beat
		_ = os.Remove(ready + ".beat")
		_ = os.WriteFile(ready+".tmp", []byte(strings.Join(lines, "\n")+"\n"), 0o600)
		_ = os.Rename(ready+".tmp", ready)
	}
	defer say("free")
	rules := bgate.Rules()
	name := gate.NodeName(args[1], rules)
	r, ok := rules[name]
	if !ok || !gate.Heavy(name, args[2:]) {
		say("free")
		return 0 // not gated: nothing done, nothing logged
	}
	// Waiting ends with the node process: Ctrl-C there, or its agent stopping it.
	ctx, stop := context.WithCancel(context.Background())
	defer stop()
	go func() {
		for syscall.Kill(pid, 0) == nil && !proc.Zombie(pid) {
			time.Sleep(250 * time.Millisecond)
		}
		stop()
	}()
	own := gateOwner()
	run, err := own.acquire(ctx, gate.Ask{Name: name, Rule: r, Session: own.key, Reserve: bgate.Reserve()}, true)
	if err != nil {
		own.log("gate-hold", map[string]any{"name": name, "pid": pid, "script": clipText(args[1]), "held": false, "why": "its waiting ended: " + err.Error()})
		say("free")
		return 0
	}
	defer run.Release()
	own.log("gate-hold", map[string]any{"name": name, "pid": pid, "script": clipText(args[1]), "held": true})
	say(append([]string{"held", gate.HeldEnv + "=system"}, gate.WorkerEnv(name, args[2:])...)...)
	// The node process's stderr is no longer ours to keep open.
	_ = os.Stderr.Close()
	run.Watch(ctx, own.watch(pid, nil))
	return 0
}

// gateHook answers Claude Code's PreToolUse hook: a Bash call that runs a
// gated program comes back with its command under rush gate run, and one
// of more than a few commands runs as a script rush can follow and stop;
// the rest of its input as it was. Anything else, or anything it can't
// read, gets nothing: the call goes ahead as it is.
func gateHook(r io.Reader, w io.Writer) {
	var in struct {
		Tool  string                    `json:"tool_name"`
		Input map[string]jsontext.Value `json:"tool_input"`
		Cwd   string                    `json:"cwd"`
		ID    string                    `json:"tool_use_id"`
	}
	b, err := io.ReadAll(io.LimitReader(r, 4<<20))
	if err != nil || jsonx.Unmarshal(b, &in) != nil || in.Tool != "Bash" || in.Input == nil {
		return
	}
	var command string
	if jsonx.Unmarshal(in.Input["command"], &command) != nil {
		return
	}
	exe := bgate.Exe()
	var timeout float64
	var background bool
	_ = jsonx.Unmarshal(in.Input["timeout"], &timeout)
	_ = jsonx.Unmarshal(in.Input["run_in_background"], &background)
	// Waiting on a pull request's checks is rush's to do, not the agent's,
	// while the watch plugin is on.
	var watched, deny string
	if prwatch.On() {
		watched, deny = watch.Redirect(command, background, time.Duration(timeout)*time.Millisecond, exe)
	}
	hookLog := func(kind string, f map[string]any) {
		f["tool_use_id"], f["command"] = in.ID, clipText(command)
		if id := os.Getenv("RUSH_SESSION"); id != "" {
			host.LogEvent(id, kind, f)
		} else {
			events.Log(kind, f)
		}
	}
	if deny != "" {
		hookLog("hook-denied", map[string]any{"why": clipText(deny)})
		out, err := jsonx.Marshal(map[string]any{"hookSpecificOutput": map[string]any{
			"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": deny}})
		if err == nil {
			_, _ = w.Write(append(out, '\n'))
		}
		return
	}
	name := gate.Gated(command, bgate.Rules())
	var wrapped, why string
	switch {
	case watched != "":
		wrapped, why = watched, "it waits on a pull request's checks: rush watch pr does that without polling"
	case name != "" && exe != "":
		if in.Cwd == "" {
			in.Cwd, _ = os.Getwd()
		}
		wrapped, why = gate.Wrap(exe, name, in.Cwd, command), "it runs "+name+", which the gate shares out"
	case script.Long(command) && !strings.Contains(command, "rush-script"):
		wrapped, _ = script.Wrap(script.Key(in.ID, command), command)
		why = "it's long: run as a script rush can follow and stop"
	}
	if wrapped == "" {
		return
	}
	hookLog("hook-rewrote", map[string]any{"to": clipText(wrapped), "why": why})
	v, err := jsonx.Marshal(wrapped)
	if err != nil {
		return
	}
	in.Input["command"] = v
	out, err := jsonx.Marshal(map[string]any{"hookSpecificOutput": map[string]any{
		"hookEventName": "PreToolUse", "updatedInput": in.Input}})
	if err == nil {
		_, _ = w.Write(append(out, '\n'))
	}
}

// gateStatus writes the gate's rules, then each run going and each
// waiter in the order they go, with why.
func gateStatus(w io.Writer) {
	rules := bgate.Rules()
	if rules == nil {
		fmt.Fprintln(w, `The gate is off: "rush plugin on gate" turns it on.`)
	} else {
		names := make([]string, 0, len(rules))
		for n := range rules {
			names = append(names, n)
		}
		slices.Sort(names)
		for _, n := range names {
			r := rules[n]
			fmt.Fprintf(w, "%-14s expects %s; stopped past %s, or holding %s\n", n, gate.Size(r.Memory), r.Limit, gate.Size(r.Cap()))
		}
	}
	reserve := bgate.Reserve()
	if avail, total := gate.Memory(); total > 0 {
		fmt.Fprintf(w, "\nMemory: %s available of %s, %s kept free; pressure %s.\n", gate.Size(avail), gate.Size(total),
			gate.Size(uint64(float64(total)*reserve)), pressureWord(gate.PressureLevel()))
	}
	running, waiting := gate.Status(reserve)
	if len(running)+len(waiting) == 0 {
		fmt.Fprintln(w, "\nNothing running or waiting.")
		return
	}
	names := map[string]string{}
	agentName := func(key string) string {
		if n, ok := names[key]; ok {
			return n
		}
		n := key
		if info, err := host.ReadInfo(key); err == nil && info.Name != "" {
			n = info.Name
		}
		names[key] = n
		return n
	}
	if len(running) > 0 {
		fmt.Fprintln(w, "\nRunning")
	}
	for _, t := range running {
		why := ""
		if t.Paused() {
			why = "  paused: " + t.Why
		}
		fmt.Fprintf(w, "  %-12s %-24s %8s  nice %-2d  %9s%s\n", t.Name, short(agentName(t.Session), 24),
			time.Since(t.Since).Round(time.Second), t.Nice, gate.Size(t.Footprint), why)
	}
	if len(waiting) > 0 {
		fmt.Fprintln(w, "\nWaiting, in the order they go")
	}
	for _, t := range waiting {
		fmt.Fprintf(w, "  %d %-12s %-24s %8s  %s\n", t.Place, t.Name, short(agentName(t.Session), 24),
			time.Since(t.Since).Round(time.Second), t.Why)
	}
}

// pressureWord is a memory pressure level in words.
func pressureWord(l int) string {
	switch l {
	case gate.PressureNormal:
		return "normal"
	case gate.PressureWarn:
		return "raised"
	case gate.PressureCritical:
		return "critical"
	}
	return "unknown"
}

// clipText is the start of s, enough to know it by, for the event log.
func clipText(s string) string {
	if r := []rune(s); len(r) > 300 {
		return string(r[:300]) + "…"
	}
	return s
}

// short is s in at most n runes.
func short(s string, n int) string {
	if r := []rune(s); len(r) > n {
		return string(r[:n-1]) + "…"
	}
	return s
}

// withoutDir is path without d.
func withoutDir(path, d string) string {
	d = filepath.Clean(d)
	return strings.Join(slices.DeleteFunc(filepath.SplitList(path), func(p string) bool {
		return p == "" || filepath.Clean(p) == d
	}), string(filepath.ListSeparator))
}
