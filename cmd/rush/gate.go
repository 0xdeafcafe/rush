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
	"runtime"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	bgate "github.com/0xdeafcafe/rush/internal/bundled/gate"
	"github.com/0xdeafcafe/rush/internal/gate"
	"github.com/0xdeafcafe/rush/internal/script"
)

const gateUsage = `rush gate — queue intensive programs your agents run (the gate plugin)

  rush gate run [--name N] [--dir D] -- cmd args…
                    run cmd once a slot is free in its queue
  rush gate shim <name> args…
                    what the gate's shims run: <name> off PATH, queued
  rush gate hold --ready F --pid P [--dir D] -- script args…
                    what the node preload runs: holds a slot for node
                    process P running script, saying so in F
  rush gate status  each queue: what runs and what waits, and the
                    shims and node preload brought up to date
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
		name, dir, argv, err := gateRunArgs(args[1:])
		if err != nil {
			fmt.Fprintf(os.Stderr, "rush gate: %v\nusage: rush gate run [--name N] [--dir D] -- cmd args…\n", err)
			return 2
		}
		return gateRun(name, dir, argv)
	case "shim":
		if len(args) < 2 {
			fmt.Fprintln(os.Stderr, "usage: rush gate shim <name> args…")
			return 2
		}
		return gateRun(args[1], "", args[1:])
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
func gateRun(name, dir string, argv []string) int {
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
	if os.Getenv(gate.HeldEnv) == "" {
		r, ok = bgate.Rules()[name]
	}
	// A shim's run is judged by its words; the hook judged a wrapped one.
	if ok && filepath.Base(argv[0]) == name && !gate.Heavy(name, argv[1:]) {
		ok = false
	}
	if !ok {
		err := syscall.Exec(path, argv, os.Environ())
		fmt.Fprintln(os.Stderr, "rush gate:", err)
		return 126
	}
	if dir == "" {
		dir, _ = os.Getwd()
	}
	key, label := gate.Key(r.Scope, dir)
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	release, err := gate.Acquire(ctx, r, key, label, name, func(w gate.Wait) { tellWait(name, w) })
	stop()
	if err != nil {
		if ctx.Err() != nil {
			return 130
		}
		fmt.Fprintln(os.Stderr, "rush gate:", err)
		return 1
	}
	defer release()
	cmd := exec.Command(path)
	cmd.Args, cmd.Env = argv, append(os.Environ(), gate.HeldEnv+"="+key)
	cmd.Env = append(cmd.Env, gate.WorkerEnv(r, name, argv)...)
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
		}
	}()
	_ = cmd.Wait()
	if ws, ok := cmd.ProcessState.Sys().(syscall.WaitStatus); ok && ws.Signaled() {
		return 128 + int(ws.Signal())
	}
	return cmd.ProcessState.ExitCode()
}

// tellWait says on stderr why name waits.
func tellWait(name string, w gate.Wait) {
	if w.Load > 0 {
		fmt.Fprintf(os.Stderr, "rush gate: %s waiting, the machine is busy (load %.0f on %d cores, %d running in %s)…\n",
			name, w.Load, runtime.NumCPU(), w.Running, w.Label)
		return
	}
	fmt.Fprintf(os.Stderr, "rush gate: %s waiting for a slot in %s (%d running, %d ahead)…\n", name, w.Label, w.Running, w.Ahead)
}

// gateHold is the node preload's holder: it says in --ready whether node
// process --pid, running a script, is gated, then holds its slot until
// that process is gone. It answers "free" when the script isn't gated, or
// can't be judged; the preload runs it unheld then, as without a gate.
func gateHold(args []string) int {
	var ready, dir string
	pid := 0
	for len(args) > 0 && args[0] != "--" {
		if len(args) < 2 {
			return 2
		}
		switch args[0] {
		case "--ready":
			ready = args[1]
		case "--dir":
			dir = args[1]
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
		return 0
	}
	if dir == "" {
		dir, _ = os.Getwd()
	}
	key, label := gate.Key(r.Scope, dir)
	// Waiting ends with the node process: Ctrl-C there, or its agent stopping it.
	ctx, stop := context.WithCancel(context.Background())
	defer stop()
	go func() {
		for syscall.Kill(pid, 0) == nil {
			time.Sleep(250 * time.Millisecond)
		}
		stop()
	}()
	release, err := gate.Acquire(ctx, r, key, label, name, func(w gate.Wait) { tellWait(name, w) })
	if err != nil {
		say("free")
		return 0
	}
	defer release()
	say(append([]string{"held", gate.HeldEnv + "=" + key}, gate.WorkerEnv(r, name, args[2:])...)...)
	// The node process's stderr is no longer ours to keep open.
	_ = os.Stderr.Close()
	<-ctx.Done()
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
	name := gate.Gated(command, bgate.Rules())
	exe := bgate.Exe()
	var wrapped string
	switch {
	case name != "" && exe != "":
		if in.Cwd == "" {
			in.Cwd, _ = os.Getwd()
		}
		wrapped = gate.Wrap(exe, name, in.Cwd, command)
	case script.Long(command) && !strings.Contains(command, "rush-script"):
		wrapped, _ = script.Wrap(script.Key(in.ID, command), command)
	}
	if wrapped == "" {
		return
	}
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
			fmt.Fprintf(w, "%-14s %s, %d at once, %s between starts, waits above load %g a core\n", n, r.Scope, r.Parallel, r.Stagger, r.Busy)
		}
	}
	scopes := gate.Status()
	if len(scopes) == 0 {
		fmt.Fprintln(w, "\nNothing running or waiting.")
	}
	for _, s := range scopes {
		fmt.Fprintf(w, "\n%s\n", s.Label)
		for _, h := range s.Running {
			fmt.Fprintf(w, "  running  pid %-7d %-10s %s\n", h.PID, h.Name, time.Since(h.Since).Round(time.Second))
		}
		for _, h := range s.Waiting {
			fmt.Fprintf(w, "  waiting  pid %-7d %-10s %s\n", h.PID, h.Name, time.Since(h.Since).Round(time.Second))
		}
	}
}

// withoutDir is path without d.
func withoutDir(path, d string) string {
	d = filepath.Clean(d)
	return strings.Join(slices.DeleteFunc(filepath.SplitList(path), func(p string) bool {
		return p == "" || filepath.Clean(p) == d
	}), string(filepath.ListSeparator))
}
