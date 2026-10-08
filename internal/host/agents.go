package host

import (
	"context"
	"encoding/json/jsontext"
	"strings"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/state"
)

// runner is an agent a session can hand work to from its shell, as rush's
// stand-ins host it: a provider in the harness it's set to run in.
type runner struct {
	k      agent.Kind
	prov   string
	name   string // its harness's, where Settings signs it in: "Codex"
	cmd    string // `codex exec "<task>"`, RUSH_AGENT= before one riding another's harness
	flag   string // how a model is picked: "-m", or "VAR=" set before the command
	models []agent.Choice
	ready  bool // signed in, or has its key
}

// runners are the agents a session can hand work to: each provider (with
// its models) in the harness it's set to run in, installed, and one a
// stand-in hosts. One that can't run yet is still one, not ready, so asked
// for by name it's known and the user told to sign it in.
func runners() []runner {
	all, cfg := Installed(), state.Load().Config
	out := make([]runner, 0, len(all))
	for _, a := range all {
		k, base, prov := a.Kind(), a, agent.ProviderOf(a.Kind())
		if agent.KeyOnly(k) && !cfg.HasAPIKey(prov) {
			continue // no key to pay with
		}
		want, _ := agent.KindFor(prov, agent.Kind(cfg.RunsIn[prov]))
		key := want
		if agent.Split(prov) {
			key, _ = agent.KindFor(prov, agent.Kind(cfg.RunsIn[agent.KeyOf(prov)]))
		}
		if k != want && k != key {
			continue // each provider in the harness it's set to run in, its key in its own
		}
		if r, ok := a.(agent.Rider); ok {
			base, _ = agent.Get(r.Rides())
		}
		sp, ok := base.(agent.Spawnable)
		if !ok {
			continue // no stand-in hosts it
		}
		r := runner{k: k, prov: prov, name: base.Name()}
		cached, hasSnapshot := a.(agent.RunnerSnapshot)
		if hasSnapshot {
			r.models, r.ready = cached.RunnerSnapshot()
		} else {
			r.ready = signedIn(base) && hasKey(a)
		}
		r.cmd, r.flag = sp.SpawnCommand()
		if k != base.Kind() {
			r.cmd = "RUSH_AGENT=" + string(k) + " " + r.cmd
		}
		ch, _ := agent.ChoicesOf(k)
		if !hasSnapshot {
			r.models = ch.Models
		}
		if ml, ok := base.(agent.ModelLister); ok && !hasSnapshot && len(r.models) == 0 {
			if ps := agent.ProfilesOf(base); len(ps) > 0 {
				r.models = ml.ListModels(ps[0])
			}
		}
		if len(r.models) == 0 {
			for _, id := range localModels(prov) {
				r.models = append(r.models, agent.Choice{ID: id})
			}
		}
		out = append(out, r)
	}
	return out
}

// agentsPrompt tells a session which agents it can start from its shell,
// for a harness that takes no MCP tools (Pi): the rest have spawn_agent.
// "" when there's none to tell of.
func agentsPrompt() string {
	var lines []string
	for _, r := range runners() {
		out := ""
		if !r.ready {
			out = "; availability unverified, so don't run it: check or refresh its account in rush, Settings, " + r.name
		}
		var ids []string
		for _, c := range r.models {
			id := c.ID
			if c.Note != "" { // what it's for, so "a cheap one" finds one
				id += " (" + strings.ToLower(c.Note[:1]) + strings.TrimSuffix(c.Note[1:], ".") + ")"
			}
			ids = append(ids, id)
		}
		what := "<model>"
		switch {
		case len(ids) > 0:
			what = "one of " + strings.Join(ids, ", ")
		case r.prov == "ollama":
			what = "<a model from `ollama list`>"
		}
		pick := ", " + r.flag + " " + what
		if strings.HasSuffix(r.flag, "=") { // a variable, set before the command
			pick = ", " + r.flag + "<model> before it"
			if what != "<model>" {
				pick += ", " + what
			}
		}
		lines = append(lines, "- "+r.prov+": "+agent.ProviderLabel(r.prov)+"'s models in "+agent.HarnessLabel(r.k)+", `"+r.cmd+"`"+pick+out)
	}
	if len(lines) == 0 {
		return ""
	}
	return "You can hand work to other agents by running them from your shell; rush hosts each one, signed in, and shows it to the user as your subagent, and its output comes back as the program's own:\n" +
		strings.Join(lines, "\n") +
		"\nWhen the user asks for one by name (\"an ollama lane\"), run that line as given; a model named on its own, or by part of its name, is the line whose list has it, run with that model. Pick one by what the work needs: a cheaper or local model for routine work, another provider for a second opinion. Treat them as you treat your own subagent types: if one fails to start, use another and tell the user; never debug its sign-in or setup."
}

// toolsNote tells a session that has rush's tools how to reach other agents.
const toolsNote = "An agent the user names (\"have <name> do it\", \"use <name> for that\", \"a codex lane\") is one of the agents rush's spawn_agent tool lists: start it with that tool, in the background when you have other work meanwhile, not from your shell."

// pick is an agent a session can start: a runner on one of its models,
// by the name the session calls it.
type pick struct {
	name, desc string
	r          runner
	model      string // "" for its own default
}

// picks are the agents a session can start, each named once.
func picks() []pick {
	var out []pick
	taken := map[string]bool{}
	for _, r := range runners() {
		models := r.models
		if len(models) == 0 {
			models = []agent.Choice{{}} // its own default
		}
		for _, c := range models {
			name := defName(r, c.ID, taken)
			taken[name] = true
			desc := strings.TrimSuffix(or(c.Note, "A capable general agent"), ".") + "."
			out = append(out, pick{name: name, desc: strings.ToUpper(desc[:1]) + desc[1:], r: r, model: c.ID})
		}
	}
	return out
}

// agentDefs are the subagent types (--agents) a Claude Code session of kind
// session runs itself: its own models, by name. Every other agent is
// spawn_agent's.
func agentDefs(session agent.Kind) map[string]jsontext.Value {
	defs := map[string]jsontext.Value{}
	for _, p := range picks() {
		if p.r.k != session || p.r.prov != string(session) || !plainModel(p.model) || !p.r.ready {
			continue
		}
		if b, err := jsonx.Marshal(map[string]any{"description": p.desc, "model": p.model, "prompt": workPrompt}); err == nil {
			defs[p.name] = b
		}
	}
	return defs
}

// agentsNote tells a session with agentDefs how to reach them, and the rest.
const agentsNote = "An agent the user names (\"have <name> do it\", \"use <name> for that\") is one of your subagent types, started with the Agent tool, or else one of the agents rush's spawn_agent tool lists, started with that tool; never by message or from your shell."

// workPrompt is a subagent's that does the work itself.
const workPrompt = "Do the task you're given, fully and carefully, then report what you did and what you found: short, with the details the one who asked needs to carry on."

// defName is what an agent on model is called: the model's own short name
// (astra for gpt-6-astra) when that's free, else its whole id, else the
// agent's with it; the agent's own for its default.
func defName(r runner, model string, taken map[string]bool) string {
	clean := func(s string) string {
		s = strings.Trim(strings.Map(func(c rune) rune {
			if c >= 'a' && c <= 'z' || c >= '0' && c <= '9' {
				return c
			}
			return '-'
		}, strings.ToLower(s)), "-")
		for strings.Contains(s, "--") {
			s = strings.ReplaceAll(s, "--", "-")
		}
		return s
	}
	var tries []string
	if model == "" {
		tries = []string{clean(string(r.k))}
	} else {
		short := model[strings.LastIndex(model, "-")+1:]
		tries = []string{clean(model), clean(string(r.k) + "-" + model)}
		if plainModel(short) && len(short) >= 3 {
			tries = append([]string{short}, tries...)
		}
	}
	for _, n := range tries {
		if !taken[n] && n != "" {
			return n
		}
	}
	return tries[len(tries)-1]
}

// plainModel is whether id is letters alone: a name, not a version.
func plainModel(id string) bool {
	return id != "" && strings.IndexFunc(id, func(c rune) bool { return c < 'a' || c > 'z' }) < 0
}

// signedIn is whether a's first profile can run now, rush signing it in
// with the sign-in it keeps when it's out.
func signedIn(a agent.Adapter) bool {
	ps := agent.ProfilesOf(a)
	return len(ps) == 0 || SignInIfOut(string(a.Kind()), ps[0]) == nil
}

// hasKey is whether a finds the key its own program keeps, when it runs
// on one (agent.KeyChecker).
func hasKey(a agent.Adapter) bool {
	kc, ok := a.(agent.KeyChecker)
	ps := agent.ProfilesOf(a)
	return !ok || len(ps) == 0 || kc.CheckKey(ps[0]) == nil
}

// localModels are the models provider prov has on this machine, as its
// own agent's Summarizer lists them (Ollama's), else none.
func localModels(prov string) []string {
	a, _ := agent.Get(agent.Kind(prov))
	sm, ok := a.(agent.Summarizer)
	if !ok {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	ms, _ := sm.SummaryModels(ctx)
	return ms
}
