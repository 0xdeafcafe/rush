// Package claude is Claude Code as a rush adapter: its config folders,
// its headless sessions as rush's own events, its prices and commands.
package claude

import (
	"github.com/0xdeafcafe/rush/internal/adapters/claude/claude"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
	"github.com/0xdeafcafe/rush/internal/agent/usage"
	"github.com/0xdeafcafe/rush/internal/state"
)

// Kind is Claude Code's.
const Kind = claude.Kind

func init() { agent.Register(Adapter{}) }

// Adapter is Claude Code.
type Adapter struct {
	// Config is rush's, for the folders it knows; nil loads it.
	Config func() state.Config
}

func (Adapter) Kind() agent.Kind { return Kind }

// ProjectFolder is the folder it keeps in a project.
func (Adapter) ProjectFolder() string { return ".claude" }
func (Adapter) Name() string          { return "Claude Code" }

// LoginHint is how to sign Claude Code in again from a terminal.
func (Adapter) LoginHint() string {
	return "run `claude auth login` in a terminal, or add the login again in rush's Settings → Providers"
}

// Maker is Anthropic, whose models Claude Code runs.
func (Adapter) Maker() string { return "Anthropic" }

// KeyEnv is where Claude Code reads an Anthropic API key from.
func (Adapter) KeyEnv() string { return "ANTHROPIC_API_KEY" }

// Speaks is the one API Claude Code talks to, and Words what else it's
// typed as (#new cc@ollama).
func (Adapter) Speaks() string  { return "Anthropic's API" }
func (Adapter) Words() []string { return []string{"cc", "claudecode"} }

// ClaudeTranscripts: its transcripts are Claude Code's own.
func (Adapter) ClaudeTranscripts() {}

// Warm: Anthropic keeps Claude warm in the cloud.
func (Adapter) Warm() {}

// Program is claude, which Claude Code's own installer puts in
// ~/.claude/local.
func (Adapter) Program() (string, []string) { return "claude", []string{".claude/local"} }

// Published is @anthropic-ai/claude-code, which updates itself with
// claude update.
func (Adapter) Published() agent.Published {
	return agent.Published{NPM: "@anthropic-ai/claude-code", Update: []string{"update"}}
}

// features: Claude Code does everything rush does, bar sessions on
// Anthropic's servers.
var features = map[agent.Feature]agent.Support{
	agent.FeatureRun: agent.Yes, agent.FeaturePrompt: agent.Yes, agent.FeaturePort: agent.Yes, agent.FeatureResume: agent.Yes, agent.FeatureFork: agent.Yes,
	agent.FeatureRewind: agent.Yes, agent.FeatureInterrupt: agent.Yes, agent.FeatureGuide: agent.Yes.With("at its next step"), agent.FeatureModel: agent.Yes,
	agent.FeatureEffort: agent.Yes, agent.FeatureModes: agent.Yes, agent.FeaturePlan: agent.Yes,
	agent.FeatureImages: agent.Yes, agent.FeatureQuestions: agent.Yes, agent.FeatureSubagents: agent.Yes,
	agent.FeatureBackground: agent.Yes, agent.FeatureContext: agent.Yes, agent.FeatureCompact: agent.Yes,
	agent.FeatureCommands: agent.Yes, agent.FeatureSideQuestion: agent.Yes, agent.FeatureDirs: agent.Yes,
	agent.FeatureMCP: agent.Yes, agent.FeatureHooks: agent.Yes, agent.FeaturePlugins: agent.Yes,
	agent.FeatureStatusLine: agent.Yes, agent.FeatureScreen: agent.Yes, agent.FeatureHandoffIn: agent.Yes,
	agent.FeatureSettings: agent.Yes, agent.FeatureStats: agent.Yes,
	agent.FeatureLive: agent.Yes, agent.FeatureHistory: agent.Yes,
	agent.FeatureSwitch: agent.Yes, agent.FeatureSignIn: agent.Yes, agent.FeatureQuota: agent.Yes,
	agent.FeaturePricing: agent.Yes, agent.FeatureMemory: agent.Yes,
}

func (Adapter) Features() map[agent.Feature]agent.Support { return features }
func (Adapter) Level() agent.Level                        { return agent.LevelFull }

// Profiles is ~/.claude: every account is a sign-in swapped into it.
func (a Adapter) Profiles() []agent.Profile {
	return []agent.Profile{a.config().ActiveAccount().Profile()}
}

func (a Adapter) config() state.Config {
	if a.Config != nil {
		return a.Config()
	}
	return state.Load().Config
}

// runAs is the folder a session started for ~/.claude runs in: the home
// of the login in use, ready, or ~/.claude when it can't be readied.
func (a Adapter) runAs() claude.Account {
	cfg := a.config()
	acct := claude.RunAccount(cfg)
	if acct.IsDefault() {
		return acct
	}
	if err := claude.LinkHome(acct, claude.Active(cfg)); err != nil || !claude.HasHome(acct) {
		return claude.Active(cfg)
	}
	return acct
}

// Home is ~/.claude, where Claude Code keeps everything when
// CLAUDE_CONFIG_DIR doesn't say otherwise.
func (Adapter) Home() agent.Profile { return claude.DefaultAccount().Profile() }

// BuiltinDef is Claude Code's own agent among the definitions.
func (Adapter) BuiltinDef() string { return claude.DefaultAgent }

// Profile is a Claude config folder as rush's own.
func Profile(a claude.Account) agent.Profile {
	return a.Profile()
}

// Account is a profile as the Claude config folder it is.
func Account(p agent.Profile) claude.Account {
	return claude.AccountOf(p)
}

// Cost is what model's tokens cost at list prices.
func (Adapter) Cost(model string, u usage.TokenUsage) (float64, bool) {
	if _, ok := claude.PriceFor(model); !ok {
		return 0, false
	}
	return claude.Cost(model, u, false), true
}

// Commands are the slash commands and skills a session in cwd can run; a
// profile without a folder is ~/.claude.
func (Adapter) Commands(p agent.Profile, cwd string) []agent.Command {
	dir := p.Dir
	if dir == "" {
		dir = claude.DefaultAccount().ConfigDir
	}
	return claude.Commands(dir, cwd)
}

var (
	_ agent.Adapter           = Adapter{}
	_ agent.Driver            = Adapter{}
	_ agent.Pricer            = Adapter{}
	_ agent.Commander         = Adapter{}
	_ agent.ClaudeTranscripts = Adapter{}
	_ agent.SkillRooter       = Adapter{}
	_ agent.Answerer          = (*conn)(nil)
	_ agent.Responder         = (*conn)(nil)
	_ agent.Asker             = (*conn)(nil)
	_ agent.ContextReader     = (*conn)(nil)
	_ agent.TaskStopper       = (*conn)(nil)
	_ agent.Backgrounder      = (*conn)(nil)
	_ agent.Staler            = (*conn)(nil)
	_ agent.QuotaKeeper       = (*conn)(nil)
	_ agent.Ender             = (*conn)(nil)
	_ agent.PIDer             = (*conn)(nil)
	_ agent.Tapper            = (*conn)(nil)
	_ agent.Describer         = Adapter{}
	_ agent.SettingsFiler     = Adapter{}
	_ agent.StatusLiner       = Adapter{}
	_ agent.SettingsPager     = Adapter{}
	_ agent.Definer           = Adapter{}
	_ agent.MemoryReader      = Adapter{}
	_ agent.MemoryKeeper      = Adapter{}
	_ agent.Brancher          = Adapter{}
	_ agent.Previewer         = Adapter{}
	_ agent.ModelNamer        = Adapter{}
	_ agent.StatsReader       = Adapter{}
	_ agent.DefaultModeler    = Adapter{}
	_ agent.LastQuotaReader   = Adapter{}
	_ agent.ContextWindower   = Adapter{}
	_ agent.Homer             = Adapter{}
	_ agent.BuiltinDefNamer   = Adapter{}
)

// Doing is a call in a few words, with words of its own for Claude Code's
// tools that have no kind (Skill, SendMessage, Monitor, ...).
func (Adapter) Doing(c *tool.Call) string { return claude.Doing(c.Name, c.Raw) }

// SpawnCommand is a run of Claude Code a shell can hand rush to host.
func (Adapter) SpawnCommand() (cmd, modelFlag string) { return `claude -p "<task>"`, "--model" }

// TakesSubagentMessages: a PostToolUse hook hands a running subagent what
// was sent it, at its next tool call.
func (Adapter) TakesSubagentMessages() {}
