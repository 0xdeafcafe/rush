package ui

import "testing"

// claudeCodeNames are Claude Code's own commands (2.1.284) that rush runs
// its own way. / is the agent's: a command rush adds takes #.
var claudeCodeNames = map[string]bool{
	"clear": true, "fork": true, "rewind": true, "model": true, "effort": true, "plan": true,
	"diff": true, "tasks": true, "copy": true, "rename": true, "cd": true, "stop": true,
	"background": true, "resume": true, "help": true, "btw": true, "export": true, "subtask": true,
}

func TestSlashIsOnlyClaudeCodes(t *testing.T) {
	for _, c := range rushCommands {
		if !claudeCodeNames[c.Name] {
			t.Errorf("/%s isn't one of Claude Code's commands: rush's own take #, so it belongs in fleetCommands", c.Name)
		}
	}
}

// #agent completes its setup, and the old /agent still runs.
func TestAgentMovedToHash(t *testing.T) {
	m, _ := benchModel(120, 40)
	if cmds := m.hashMatches([]rune("#agent "), 0); len(cmds) == 0 {
		t.Fatal("#agent offers no setups")
	}
	if _, ok := m.setupCommand(nil, "/agent", "/agent"); !ok {
		t.Fatal("/agent typed the old way no longer runs")
	}
}
