package main

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/host"
	"github.com/0xdeafcafe/rush/internal/memory"
)

const memoryUsage = `rush memory — the memory notes agents keep for this project

  rush memory              the notes, what each is about, and how often sessions read it
  rush memory add <text>   remember something: a cheap model writes the note (its name,
                           description and type) and its line in MEMORY.md
      --type <t>           user, feedback, project or reference
      --name <n> --description <d>
                           with --type too, the note is written as given, no model asked
  rush memory review       the notes worth a look, and why
  rush memory tidy         add MEMORY.md lines for notes without one, take out lines
                           for notes that don't exist

It works on the project of the folder it's run in (through any worktree, the
repository's), for the session in RUSH_SESSION's account, or your first.`

func memoryCmd(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	fail := func(err error) int {
		fmt.Fprintln(stderr, "rush memory:", err)
		return 1
	}
	if len(args) > 0 && (args[0] == "help" || args[0] == "--help" || args[0] == "-h") {
		fmt.Fprintln(stdout, memoryUsage)
		return 0
	}
	p, cwd, err := memoryWhere()
	if err != nil {
		return fail(err)
	}
	mr, _ := agent.As[agent.MemoryReader](p.Kind)
	mk, _ := agent.As[agent.MemoryKeeper](p.Kind)
	if mr == nil || mk == nil {
		return fail(fmt.Errorf("%s keeps no memory rush can read", p.Kind))
	}
	cmd := "list"
	if len(args) > 0 {
		cmd, args = args[0], args[1:]
	}
	switch cmd {
	case "list", "ls":
		mem := mr.Memory(p, cwd, "")
		mk.CountOpens(p, &mem)
		n := 0
		for _, f := range mem.Files {
			if f.Group != "Auto memory" || f.Name == "MEMORY.md" || f.Missing {
				continue
			}
			n++
			fmt.Fprintf(stdout, "%-10s %-30s %s · %s\n", f.Kind, f.Name, f.About, readsText(f))
		}
		if n == 0 {
			fmt.Fprintln(stdout, "no memory notes for", cwd)
		}
		if mem.Dir != "" {
			fmt.Fprintln(stdout, "\nin", mem.Dir)
		}
		return 0
	case "review":
		mem := mr.Memory(p, cwd, "")
		n := 0
		for _, f := range mem.Files {
			if why := memory.Why(f, time.Now()); why != "" {
				n++
				fmt.Fprintf(stdout, "%-30s %s\n", f.Name, why)
			}
		}
		if n == 0 {
			fmt.Fprintln(stdout, "nothing to review")
		}
		return 0
	case "tidy":
		mem := mr.Memory(p, cwd, "")
		if mem.Dir == "" {
			return fail(fmt.Errorf("no memory for %s", cwd))
		}
		did, err := mk.TidyMemory(mem.Dir)
		if err != nil {
			return fail(err)
		}
		if len(did) == 0 {
			did = []string{"nothing to tidy"}
		}
		fmt.Fprintln(stdout, strings.Join(did, "\n"))
		return 0
	case "add":
		n, words, err := addFlags(args)
		if err != nil {
			return fail(err)
		}
		text := strings.Join(words, " ")
		if text == "-" || text == "" && !isTerminal(stdin) {
			b, _ := io.ReadAll(stdin)
			text = strings.TrimSpace(string(b))
		}
		if text == "" {
			return fail(fmt.Errorf("nothing to remember: rush memory add <text>"))
		}
		if n.Name == "" || n.Description == "" || n.Type == "" {
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
			defer cancel()
			d, err := memory.Draft(ctx, text, "")
			if err != nil {
				return fail(err)
			}
			n.Name, n.Description, n.Type = first(n.Name, d.Name), first(n.Description, d.Description), first(n.Type, d.Type)
			n.Title, n.Body = d.Title, d.Body
		} else {
			n.Body = text
		}
		path, err := mk.Remember(p, cwd, n)
		if err != nil {
			return fail(err)
		}
		if w, err := mk.ReadNote(path); err == nil {
			n = w
		}
		fmt.Fprintf(stdout, "remembered (%s): %s\n%s\n", n.Type, n.Description, path)
		return 0
	}
	return fail(fmt.Errorf("no command %q: rush memory help", cmd))
}

// addFlags reads add's --type, --name and --description (as --x v or
// --x=v); the rest, and all after --, is the text.
func addFlags(args []string) (agent.MemoryNote, []string, error) {
	var n agent.MemoryNote
	var words []string
	to := map[string]*string{"--type": &n.Type, "--name": &n.Name, "--description": &n.Description}
	for i := 0; i < len(args); i++ {
		a := args[i]
		if a == "--" {
			words = append(words, args[i+1:]...)
			break
		}
		k, v, eq := strings.Cut(a, "=")
		dst, ok := to[k]
		if !ok {
			words = append(words, a)
			continue
		}
		if !eq {
			if i+1 >= len(args) {
				return n, nil, fmt.Errorf("%s needs a value", k)
			}
			v, i = args[i+1], i+1
		}
		*dst = v
	}
	if n.Type != "" && !memory.ValidType(n.Type) {
		return n, nil, fmt.Errorf("--type is one of %s", strings.Join(memory.Types, ", "))
	}
	return n, words, nil
}

// isTerminal is whether r is a terminal, which add doesn't wait on.
func isTerminal(r io.Reader) bool {
	f, ok := r.(*os.File)
	if !ok {
		return false
	}
	st, err := f.Stat()
	return err == nil && st.Mode()&os.ModeCharDevice != 0
}

func first(s ...string) string {
	for _, v := range s {
		if v != "" {
			return v
		}
	}
	return ""
}

func readsText(f agent.MemoryFile) string {
	switch {
	case !f.Counted || f.Reads == 0:
		return "only its line in MEMORY.md"
	case f.Reads == 1:
		return "opened once, " + f.LastRead.Format("2 Jan")
	}
	return fmt.Sprintf("opened %d×, last %s", f.Reads, f.LastRead.Format("2 Jan"))
}

// memoryWhere is the profile and folder rush memory works on: the
// CLAUDE_CONFIG_DIR set (a Claude Code session's own), else the rush
// session's account when its agent keeps memory rush can write, else
// Claude Code's first.
func memoryWhere() (agent.Profile, string, error) {
	cwd, _ := os.Getwd()
	a, ok := agent.Get(agent.LegacyKind)
	if !ok {
		return agent.Profile{}, "", fmt.Errorf("no Claude Code")
	}
	if d := os.Getenv("CLAUDE_CONFIG_DIR"); d != "" { // in a session, its account's
		return agent.Profile{Kind: agent.LegacyKind, Name: filepath.Base(d), Dir: d}, cwd, nil
	}
	if id := os.Getenv("RUSH_SESSION"); id != "" {
		if cfg, err := host.ReadConfig(id); err == nil && cfg.Account.Dir != "" {
			if cfg.Account.Kind == "" {
				cfg.Account.Kind = agent.LegacyKind
			}
			if _, ok := agent.As[agent.MemoryKeeper](cfg.Account.Kind); ok {
				return cfg.Account, cwd, nil
			}
		}
	}
	ps := a.Profiles()
	if len(ps) == 0 {
		return agent.Profile{}, "", fmt.Errorf("no Claude Code account")
	}
	return ps[0], cwd, nil
}
