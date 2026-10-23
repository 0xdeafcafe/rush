package main

import (
	"errors"
	"fmt"
	"io"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/room"
	"github.com/0xdeafcafe/rush/internal/state"
)

const roomUsage = `rush room: a group chat of fresh agents arguing a topic to a verdict

  rush room new [--cwd DIR] [--rounds N] [--watch] --agent KIND[:MODEL[:EFFORT]]... <topic>
        opens a room with a fresh panel (two or more --agent), prints its id
  rush room show <id> [--follow]     the transcript; --follow until it ends
  rush room say <id> [text]          post as you (text or stdin): "@name ..." to
                                     one agent, "/verdict" to end on a verdict, "/stop"
  rush room pause <id>               stop the speaker now; no turn starts until resume
  rush room resume <id>              carry on: the paused agent picks its turn up again
  rush room list

In rush itself, #room opens the full-screen view.
`

func roomCmd(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	if len(args) == 0 || args[0] == "help" || args[0] == "-h" || args[0] == "--help" {
		io.WriteString(stdout, roomUsage)
		return 0
	}
	var err error
	switch args[0] {
	case "run": // the driver: rush room new starts it
		if len(args) != 2 {
			err = errors.New("usage: rush room run <id>")
			break
		}
		background("")
		err = room.Run(args[1])
	case "new":
		err = roomNew(args[1:], stdout)
	case "show":
		err = roomShow(args[1:], stdout)
	case "say":
		err = roomSay(args[1:], stdin)
	case "pause", "resume", "stop", "verdict":
		if len(args) != 2 {
			err = fmt.Errorf("usage: rush room %s <id>", args[0])
			break
		}
		err = roomSay([]string{args[1], "/" + args[0]}, stdin)
	case "list":
		for _, r := range room.List() {
			status := "over"
			if room.Running(r.ID) {
				status = "live"
			}
			var names []string
			for _, m := range r.Members {
				names = append(names, m.Name)
			}
			fmt.Fprintf(stdout, "%s  %s  %s  (%s)\n", r.ID, status, r.Topic, strings.Join(names, ", "))
		}
	default:
		err = fmt.Errorf("unknown room command %q; run rush room help", args[0])
	}
	if err != nil {
		fmt.Fprintln(stderr, "rush:", err)
		return 1
	}
	return 0
}

func roomNew(args []string, stdout io.Writer) error {
	fs := newFlags("new")
	var (
		cwd    string
		rounds int
		watch  bool
		agents multi
	)
	fs.StringVar(&cwd, "cwd", ".", "")
	fs.IntVar(&rounds, "rounds", room.DefaultRounds, "")
	fs.BoolVar(&watch, "watch", false, "")
	fs.Var(&agents, "agent", "")
	if err := fs.Parse(args); err != nil {
		return err
	}
	topic := strings.TrimSpace(strings.Join(fs.Args(), " "))
	if topic == "" || len(agents) < 2 {
		return errors.New("usage: rush room new --agent KIND[:MODEL] --agent KIND[:MODEL]... <topic>")
	}
	abs, err := filepath.Abs(cwd)
	if err != nil {
		return err
	}
	r := room.Room{Topic: topic, Cwd: abs, Rounds: rounds}
	var taken []string
	for _, a := range agents {
		parts := strings.SplitN(a, ":", 3)
		parts = append(parts, "", "")
		cfg, err := roomMemberConfig(parts[0], parts[1], parts[2], abs)
		if err != nil {
			return err
		}
		name := room.Name(cfg.Kind, cfg.Model, taken)
		taken = append(taken, name)
		r.Members = append(r.Members, room.Member{Name: name, Config: cfg})
	}
	if r, err = room.Create(r); err != nil {
		return err
	}
	if err := room.Start(r.ID); err != nil {
		return err
	}
	fmt.Fprintf(stdout, "room %s: %s\n", r.ID, strings.Join(taken, ", "))
	if watch {
		return roomShow([]string{r.ID, "--follow"}, stdout)
	}
	return nil
}

// roomMemberConfig is a fresh session of kind as Settings starts it, on
// model and effort when given.
func roomMemberConfig(kind, model, effort, cwd string) (host.Config, error) {
	st := state.Load()
	d := st.Config.Dispatch
	cfg := host.Config{Cwd: cwd}
	if kind == state.LoginsKind {
		cfg.Account = st.Config.ActiveAccount().Profile()
	}
	if err := cfg.UseAgent(kind); err != nil {
		return cfg, err
	}
	_ = host.SignInIfOut(kind, cfg.Account)
	start := d.ResolvedFor(kind)
	cfg.Lean = d.QuickStart()
	cfg.Model = or(model, start.Model)
	cfg.Effort = or(effort, start.Effort)
	cfg.PermissionMode = start.Mode
	return cfg, nil
}

func roomShow(args []string, stdout io.Writer) error {
	if len(args) == 0 {
		return errors.New("usage: rush room show <id> [--follow]")
	}
	id, follow := args[0], len(args) > 1 && args[1] == "--follow"
	if _, err := room.Load(id); err != nil {
		return err
	}
	var off int64
	for {
		es, next, err := room.Read(id, off)
		if err != nil {
			return err
		}
		off = next
		for _, e := range es {
			if line := roomLine(e); line != "" {
				fmt.Fprintln(stdout, line)
			}
			if e.Kind == room.End {
				return nil
			}
		}
		if !follow {
			return nil
		}
		if len(es) == 0 && !room.Running(id) {
			return errors.New("the room's driver isn't running")
		}
		time.Sleep(300 * time.Millisecond)
	}
}

// roomLine is an entry as rush room show prints it.
func roomLine(e room.Entry) string {
	at := e.At.Format("15:04:05")
	switch e.Kind {
	case room.Say:
		who := e.From
		if who == room.User {
			who = "you"
			if e.To != "" {
				who += " → " + e.To
			}
		}
		ready := ""
		if e.Ready != "" {
			ready = "  [READY: " + e.Ready + "]"
		}
		round := ""
		if e.Round > 0 {
			round = " · round " + strconv.Itoa(e.Round)
		}
		return fmt.Sprintf("\n%s  %s%s%s\n%s", at, who, round, ready, e.Text)
	case room.Note:
		return fmt.Sprintf("%s  %s (on the way): %s", at, e.From, e.Text)
	case room.Tool:
		return fmt.Sprintf("%s  %s ⏺ %s  %s", at, e.From, e.Text, e.Detail)
	case room.Done:
		if e.Failed {
			return fmt.Sprintf("%s  %s ✗ tool failed", at, e.From)
		}
		return ""
	case room.Verdict:
		return fmt.Sprintf("\n%s  ══ VERDICT: each agent's final position (%s) ══\n%s", at, e.Detail, e.Text)
	case room.Paused:
		return fmt.Sprintf("%s  ⏸ %s", at, e.Text)
	case room.Uses:
		return fmt.Sprintf("%s  — %s runs %s", at, e.From, e.Text)
	case room.Control:
		return fmt.Sprintf("%s  you: /%s", at, e.Text)
	case room.Turn:
		return fmt.Sprintf("%s  … %s is speaking", at, e.From)
	case room.Info, room.End:
		return fmt.Sprintf("%s  — %s", at, e.Text)
	}
	return ""
}

func roomSay(args []string, stdin io.Reader) error {
	if len(args) == 0 {
		return errors.New("usage: rush room say <id> [text]")
	}
	r, err := room.Load(args[0])
	if err != nil {
		return err
	}
	text := strings.Join(args[1:], " ")
	if text == "" {
		b, err := io.ReadAll(io.LimitReader(stdin, 1<<20))
		if err != nil {
			return err
		}
		text = string(b)
	}
	if strings.TrimSpace(text) == "" {
		return errors.New("nothing to say")
	}
	if !room.Running(r.ID) {
		return errors.New("the room is over")
	}
	return room.Post(r.ID, room.Parse(text, r.Members))
}
