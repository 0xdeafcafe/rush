package plugin

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"sync"
	"time"
)

// CLISpec is a command a plugin adds to rush's own: `rush <plugin>
// <name> [args]`. rush hands the plugin the arguments and prints what it
// answers; it runs in the plugin, with what the plugin may do, not as you.
type CLISpec struct {
	Name        string `json:"name"`
	Usage       string `json:"usage,omitempty"` // its arguments, as help shows them
	Description string `json:"description"`
}

// CLIRun is a CLI command for a plugin to run: `cli.run`'s params.
type CLIRun struct {
	Command string   `json:"command"`
	Args    []string `json:"args"`
	// Cwd is the folder it was run in: to say what it's about, not to
	// read, which the sandbox doesn't allow.
	Cwd string `json:"cwd,omitempty"`
}

// CLIResult is what a plugin answers `cli.run` with.
type CLIResult struct {
	Stdout string `json:"stdout,omitempty"`
	Stderr string `json:"stderr,omitempty"`
	Exit   int    `json:"exit,omitzero"`
}

// Limits on a plugin's CLI.
const (
	maxCLI       = 16
	maxCLIArgs   = 64
	maxCLIArg    = 4 << 10
	MaxCLIOutput = 1 << 20 // of each of stdout and stderr
	// CLITimeout is how long a CLI command may take.
	CLITimeout = 2 * time.Minute
)

var cliUsageRE = sync.OnceValue(func() *regexp.Regexp { return regexp.MustCompile(`^[^\x00-\x1f\x7f\x{80}-\x{9f}]{0,120}$`) })

func (m Manifest) validateCLI() error {
	if len(m.CLI) == 0 {
		return nil
	}
	if m.Proto() != ProtoRush {
		return errors.New("an MCP plugin cannot add CLI commands: it has no way to run them")
	}
	if len(m.CLI) > maxCLI {
		return fmt.Errorf("cli: at most %d commands", maxCLI)
	}
	seen := map[string]bool{}
	for _, c := range m.CLI {
		if !cmdRE().MatchString(c.Name) {
			return fmt.Errorf("cli command %q: use lowercase letters, digits and dashes, starting with a letter", c.Name)
		}
		if seen[c.Name] {
			return fmt.Errorf("cli command %q is there twice", c.Name)
		}
		seen[c.Name] = true
		if c.Description == "" || !cliUsageRE().MatchString(c.Description) || !cliUsageRE().MatchString(c.Usage) {
			return fmt.Errorf("cli command %q needs a description of one short line, and a usage of one", c.Name)
		}
	}
	return nil
}

// CLICommand is the manifest's CLI command name.
func (m Manifest) CLICommand(name string) (CLISpec, bool) {
	for _, c := range m.CLI {
		if c.Name == name {
			return c, true
		}
	}
	return CLISpec{}, false
}

// CheckCLIRun checks what the CLI hands a plugin.
func (m Manifest) CheckCLIRun(r CLIRun) error {
	if _, ok := m.CLICommand(r.Command); !ok {
		return fmt.Errorf("%s has no command %q", m.Name, r.Command)
	}
	if len(r.Args) > maxCLIArgs {
		return fmt.Errorf("at most %d arguments", maxCLIArgs)
	}
	for _, a := range r.Args {
		if len(a) > maxCLIArg {
			return fmt.Errorf("an argument is over %d bytes", maxCLIArg)
		}
	}
	return nil
}

// Clip keeps a result within MaxCLIOutput.
func (r CLIResult) Clip() CLIResult {
	clip := func(s string) string {
		if len(s) > MaxCLIOutput {
			return s[:MaxCLIOutput] + "\n… (cut)\n"
		}
		return s
	}
	r.Stdout, r.Stderr = clip(r.Stdout), clip(r.Stderr)
	return r
}

// CLIPlugins are the running plugins' manifests that add CLI commands, by
// name.
func CLIPlugins() map[string]Manifest {
	out := map[string]Manifest{}
	for name, a := range Enabled() {
		if len(a.Manifest.CLI) > 0 {
			out[name] = a.Manifest
		}
	}
	return out
}

// RunCLI has the broker run a plugin's CLI command, starting the broker if
// it isn't running.
func RunCLI(ctx context.Context, name string, r CLIRun) (CLIResult, error) {
	c, err := DialBroker()
	if err != nil {
		if err := EnsureBroker(); err != nil {
			return CLIResult{}, err
		}
		for c == nil {
			select {
			case <-ctx.Done():
				return CLIResult{}, errors.New("rush's plugin broker didn't start")
			case <-time.After(50 * time.Millisecond):
			}
			c, _ = DialBroker()
		}
	}
	defer c.Close()
	var out CLIResult
	err = c.Call(ctx, "cli", map[string]any{"plugin": name, "run": r}, &out)
	return out, err
}
