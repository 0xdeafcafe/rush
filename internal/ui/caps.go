package ui

import (
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// commandNeeds are rush's session commands only some agents can do, and
// the feature each needs.
var commandNeeds = map[string]agent.Feature{
	"fork": agent.FeatureFork, "rewind": agent.FeatureRewind, "effort": agent.FeatureEffort, "plan": agent.FeaturePlan,
	"tasks": agent.FeatureBackground, "subtask": agent.FeatureSubagents,
	"btw": agent.FeatureSideQuestion, "catchup": agent.FeatureSideQuestion, "cd": agent.FeatureDirs,
	"model": agent.FeatureModel, "compact": agent.FeatureCompact,
}

// screenNeeds are the screens a session opens and the features each needs.
// A screen rush draws works from rush's own records or the agent's
// adapter; one it doesn't (mcp) is the agent's own, so it needs its
// screen too. status and config are every agent's.
var screenNeeds = map[string][]agent.Feature{
	"context": {agent.FeatureContext}, "usage": {agent.FeatureQuota}, "stats": {agent.FeatureStats},
	"skills": {agent.FeatureCommands}, "plugin": {agent.FeaturePlugins}, "hooks": {agent.FeatureHooks},
	"permissions": {agent.FeatureSettings}, "memory": {agent.FeatureMemory}, "statusline": {agent.FeatureStatusLine},
	"mcp": {agent.FeatureMCP, agent.FeatureScreen},
}

// sessionAgent is the agent a session runs.
func sessionAgent(c *hostConn) agent.Kind {
	if k := c.sess.Info.Kind; k != "" {
		return agent.Kind(k) // its host's word
	}
	return c.kind
}

// agentCanRun is whether an agent of kind can do the command rush would
// run for name: what canRun checks for a session, and what the command bar
// checks with no session open.
func agentCanRun(kind agent.Kind, name string) bool {
	need, gated := commandNeeds[name]
	return !gated || agent.Supports(kind, need)
}

// canRun is whether the session's agent can do the command rush would
// run for name, or open the screen it names.
func canRun(c *hostConn, name string) bool {
	if screen, ok := claudeScreen(name); ok && !canScreen(c, screen) {
		return false
	}
	return agentCanRun(sessionAgent(c), name)
}

// canScreen is whether the session's agent can open screen.
func canScreen(c *hostConn, screen string) bool {
	for _, f := range screenNeeds[screen] {
		if !agent.Supports(sessionAgent(c), f) {
			return false
		}
	}
	return true
}

// ownScreens is whether the session's agent has screens of its own, which
// rush shows or hands the terminal to.
func ownScreens(c *hostConn) bool { return agent.Supports(sessionAgent(c), agent.FeatureScreen) }

// sessionCommands are rush's commands this session's agent can do.
func sessionCommands(c *hostConn) []event.Command {
	var out []event.Command
	for _, cmd := range rushCommands {
		if canRun(c, cmd.Name) {
			out = append(out, cmd)
		}
	}
	return out
}
