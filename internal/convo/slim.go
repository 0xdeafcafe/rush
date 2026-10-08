package convo

import (
	"sort"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/tool"
)

// Unused is something in the context the session has never used, what it
// costs every request, and the tool rules its agent goes without it by.
type Unused struct {
	What   string // "MCP server", "subagent", "skill"
	Name   string
	Tokens int
	Rules  []string
}

// Unused is what the session's last context reading holds that it hasn't
// called once: MCP servers, subagents and skills, the costliest first.
// rush's own server stays: it's how rush draws. Nil before a reading.
// ponytail: a thing unused so far may be wanted later; the sheet asks, and
// dropping it only lasts until it's put back.
func (s *Session) Unused() []Unused {
	u := s.Usage
	if u == nil {
		return nil
	}
	servers, agents, skills := s.used()
	var out []Unused
	cost := map[string]int{}
	var names []string
	for _, t := range u.MCPTools {
		if _, seen := cost[t.Server]; !seen {
			names, cost[t.Server] = append(names, t.Server), 0
		}
		if t.IsLoaded { // one loaded on demand costs nothing until it is
			cost[t.Server] += t.Tokens
		}
	}
	for _, n := range names {
		if id := mcpID(n); id != "rush" && !servers[id] {
			out = append(out, Unused{What: "MCP server", Name: n, Tokens: cost[n], Rules: []string{"mcp__" + id}})
		}
	}
	for _, a := range u.Agents {
		if !agents[a.Type] {
			out = append(out, Unused{What: "subagent", Name: a.Type, Tokens: a.Tokens, Rules: []string{"Task(" + a.Type + ")", "Agent(" + a.Type + ")"}})
		}
	}
	for _, sk := range u.Skills.Each {
		if !skills[sk.Name] {
			out = append(out, Unused{What: "skill", Name: sk.Name, Tokens: sk.Tokens, Rules: []string{"Skill(" + sk.Name + ")"}})
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Tokens > out[j].Tokens })
	return out
}

// used are the MCP servers (by id), subagents and skills the session has
// called.
func (s *Session) used() (servers, agents, skills map[string]bool) {
	servers, agents, skills = map[string]bool{}, map[string]bool{}, map[string]bool{}
	for name := range s.Tools {
		if rest, ok := strings.CutPrefix(name, "mcp__"); ok {
			id, _, _ := strings.Cut(rest, "__")
			servers[id] = true
		}
	}
	for _, st := range s.byID {
		switch {
		case st.kind() == tool.Subagent:
			agents[agentName(st)] = true
		case st.Tool == "Skill":
			var in struct {
				Skill   string `json:"skill"`
				Command string `json:"command"`
			}
			if jsonx.Unmarshal(st.Input, &in) == nil {
				skills[firstNonEmpty(in.Skill, strings.TrimPrefix(in.Command, "/"))] = true
			}
		}
	}
	return servers, agents, skills
}

// mcpID is a server's name as its tools are named: mcp__<id>__<tool>,
// anything but letters, digits, _ and - made _.
func mcpID(name string) string {
	return strings.Map(func(r rune) rune {
		if r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '_' || r == '-' {
			return r
		}
		return '_'
	}, name)
}
