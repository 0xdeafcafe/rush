package acp

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agtools"
)

// Agent is an agent rush knows only through ACP: how to start it, and
// where it keeps its things. The agents below need nothing more yet.
type Agent struct {
	ID      agent.Kind
	Title   string
	Company string   // whose models it runs, when that isn't Title: Google for Gemini CLI
	Command string   // its program
	Args    []string // what makes it speak ACP
	ACP     string   // the program that does, beside Command, when it's another: vibe-acp
	Home    string   // its config folder, under the home folder: ".kimi-code"
	// Once is how Command runs one task and prints the answer, "<task>"
	// for the prompt; Model is the flag that picks its model, or a VAR=.
	Once, Model string
	// Flags are the flags of a one-shot run rush hosts, by what each
	// sets: prompt, model, cwd, mode (its value), mode=m, "=v" for one
	// that must be v, "" for one that changes nothing printed.
	Flags map[string]string
	// Creds are files in Home, and Keys environment variables, any of
	// which means it's signed in; with none, rush takes it as it is.
	Creds, Keys []string
	// SignIn says how to sign it in, when it isn't.
	SignIn string
	// More are features it has, or lacks, beyond what ACP gives every
	// agent (Features).
	More map[agent.Feature]agent.Support
	// Tried is how far rush's support for it has been tried.
	Tried agent.Level
	// Pub is where it's published, when that's known.
	Pub agent.Published
}

// Published is where the agent's program is published.
func (a Agent) Published() agent.Published { return a.Pub }

// Known are the ACP agents rush runs. An agent that grows more than ACP
// gives (history, limits) moves to a package of its own, as Copilot,
// Vibe and Gemini have.
var Known = []Agent{
	{ID: "kimi", Title: "Kimi CLI", Company: "Moonshot", Command: "kimi", Args: []string{"acp"}, Home: ".kimi",
		Once: `kimi -p "<task>"`, Model: "-m", Flags: map[string]string{"-p": "prompt", "--prompt": "prompt",
			"-m": "model", "--model": "model", "--output-format": "=text"}},
	{ID: "opencode", Title: "OpenCode", Command: "opencode", Args: []string{"acp"}, Home: ".config/opencode",
		Pub:  agent.Published{NPM: "opencode-ai", Update: []string{"upgrade"}},
		Once: `opencode run "<task>"`, Model: "-m", Flags: map[string]string{"-m": "model", "--model": "model",
			"--dir": "cwd", "--format": "=default"}},
}

func init() {
	for _, a := range Known {
		if a.ID == "kimi" {
			agent.Register(Kimi{a})
		} else {
			agent.Register(a)
		}
	}
}

func (a Agent) Kind() agent.Kind { return a.ID }
func (a Agent) Name() string     { return a.Title }
func (a Agent) Maker() string    { return a.Company }

// features are what ACP gives any agent. Rewind, fork, context breakdowns,
// background tasks and a screen of its own have no ACP equivalent.
var features = map[agent.Feature]agent.Support{
	agent.FeatureRun: agent.Yes, agent.FeatureResume: agent.Yes, agent.FeatureInterrupt: agent.Yes,
	agent.FeatureModel: agent.Yes.With("when the agent offers models"),
	agent.FeatureModes: agent.Yes.With("the agent's own"), agent.FeaturePlan: agent.Yes.With("when the agent has a plan mode"),
	agent.FeatureImages: agent.Yes, agent.FeatureQuestions: agent.Yes, agent.FeatureMCP: agent.Yes,
	agent.FeatureHandoffIn: agent.Yes, agent.FeatureSubagents: agent.Yes.With("when the agent has a subagent tool"),
	agent.FeatureBackground: agent.No.With("ACP has no background tasks"),
	agent.FeatureHistory:    agent.Planned, agent.FeaturePricing: agent.Planned,
}

// Features are ACP's, with the agent's own over them.
func (a Agent) Features() map[agent.Feature]agent.Support {
	if len(a.More) == 0 {
		return features
	}
	out := make(map[agent.Feature]agent.Support, len(features)+len(a.More))
	for f, s := range features {
		out[f] = s
	}
	for f, s := range a.More {
		out[f] = s
	}
	return out
}

func (a Agent) Level() agent.Level { return a.Tried }

// Program is its program, which its installer may put in its config
// folder's bin.
func (a Agent) Program() (string, []string) {
	return a.Command, []string{filepath.Join(a.Home, "bin")}
}

// Profiles is its config folder, when it's installed.
func (a Agent) Profiles() []agent.Profile {
	if !agent.Installed(a.ID) {
		return nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return nil
	}
	return []agent.Profile{{Kind: a.ID, Name: a.Title, Dir: filepath.Join(home, a.Home)}}
}

// CheckKey is whether it's signed in: a file it keeps, or a key set.
func (a Agent) CheckKey(p agent.Profile) error {
	if len(a.Creds)+len(a.Keys) == 0 {
		return nil
	}
	for _, k := range a.Keys {
		if os.Getenv(k) != "" {
			return nil
		}
	}
	for _, f := range a.Creds {
		if _, err := os.Stat(filepath.Join(p.Dir, f)); err == nil {
			return nil
		}
	}
	return errors.New(a.Title + " isn't signed in: " + a.SignIn)
}

// ErrNoFork is a fork asked of an agent ACP can't fork.
var ErrNoFork = errors.New("acp: this agent can't fork a session")

// Start runs it over ACP, then puts it in the mode and model asked for.
func (a Agent) Start(ctx context.Context, o agent.StartOptions) (agent.Conn, error) {
	if o.Fork {
		return nil, ErrNoFork
	}
	cmd := a.Command
	if o.Binary != "" {
		cmd = o.Binary
	} else if _, err := exec.LookPath(cmd); err != nil {
		if p := agent.Path(a.ID); p != "" {
			cmd = p
		}
	}
	if a.ACP != "" {
		cmd = filepath.Join(filepath.Dir(cmd), a.ACP)
	}
	opts := Options{Command: cmd, Args: append(append([]string(nil), a.Args...), o.Flags...), Env: o.Env, Dir: o.Dir, Adapter: string(a.ID)}
	if o.Resume {
		opts.Resume = o.SessionID
	}
	for _, server := range o.Tools {
		if server.Name == agtools.Server && server.Args != nil {
			exe, err := os.Executable()
			if err != nil {
				return nil, err
			}
			descriptor, err := jsonx.Marshal(map[string]any{"name": server.Name, "command": exe, "args": server.Args, "env": agtools.Env()})
			if err != nil {
				return nil, err
			}
			opts.MCPServers = append(opts.MCPServers, descriptor)
			for _, n := range server.Trusted {
				opts.Trusted = append(opts.Trusted, "mcp__"+server.Name+"__"+n)
			}
		}
	}

	s, err := Start(ctx, opts)
	if err != nil {
		return nil, err
	}
	if o.Mode != "" {
		if err := s.SetMode(o.Mode); err != nil {
			_ = s.Close()
			return nil, err
		}
	}
	if o.Model != "" {
		if err := s.SetModel(o.Model); err != nil {
			_ = s.Close()
			return nil, err
		}
	}
	// A saved effort the model doesn't offer (switched to one that
	// doesn't think) starts it at the model's own.
	if o.Effort != "" {
		if err := s.SetEffort(o.Effort); err != nil && !errors.Is(err, errors.ErrUnsupported) {
			_ = s.Close()
			return nil, err
		}
	}
	return s, nil
}

// SpawnCommand is its one-shot run, as a session's shell writes it.
func (a Agent) SpawnCommand() (cmd, modelFlag string) { return a.Once, a.Model }

// ReadOnce reads args as its one-shot run: the prompt after Once's flag,
// or the words after its subcommand, and what Flags set. A flag it
// doesn't name, or two that set one thing two ways, runs the real one.
func (a Agent) ReadOnce(args []string) (agent.Once, bool) {
	var o agent.Once
	sub := ""
	if w := strings.Fields(a.Once); len(w) > 1 && !strings.HasPrefix(w[1], "-") {
		sub = w[1] // opencode run
	}
	var words []string
	started := false
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if !strings.HasPrefix(arg, "-") {
			switch {
			case sub != "" && !started && arg == sub:
				started = true
			case sub != "" && started:
				words = append(words, arg)
			default:
				return o, false // a prompt for its own screen
			}
			continue
		}
		name, val, eq := strings.Cut(arg, "=")
		role, ok := a.Flags[name]
		bare := role == "" || strings.HasPrefix(role, "mode=")
		switch {
		case !ok || bare && eq:
			return o, false
		case !bare && !eq:
			if i+1 >= len(args) || strings.HasPrefix(args[i+1], "-") {
				return o, false
			}
			i++
			val = args[i]
		}
		var set *string
		switch {
		case role == "prompt":
			started, words = true, append(words, val)
		case role == "model":
			set = &o.Model
		case role == "cwd":
			set = &o.Cwd
		case role == "mode":
			set = &o.Mode
		case strings.HasPrefix(role, "mode="):
			set, val = &o.Mode, strings.TrimPrefix(role, "mode=")
		case strings.HasPrefix(role, "=") && val != role[1:]:
			return o, false
		}
		if set != nil && *set != "" && *set != val {
			return o, false
		}
		if set != nil {
			*set = val
		}
	}
	if v, ok := strings.CutSuffix(a.Model, "="); ok && o.Model == "" {
		o.Model = os.Getenv(v)
	}
	o.Prompt = strings.Join(words, " ")
	return o, started && o.Prompt != ""
}

var (
	_ agent.Adapter    = Agent{}
	_ agent.Driver     = Agent{}
	_ agent.OnceReader = Agent{}
	_ agent.KeyChecker = Agent{}
	_ agent.Conn       = (*Session)(nil)
	_ agent.Answerer   = (*Session)(nil)
)
