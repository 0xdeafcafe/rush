package main

// shot is one screenshot: its name, the terminal's size, and the steps
// (see term.do) that reach its screen from rush's first.
type shot struct {
	name  string
	w, h  int
	steps []string
}

// open goes to a session the way you would: by its name, from ctrl+k.
func open(name string) []string {
	return []string{"C-k", "text=" + name, "Enter", "wait=1500ms"}
}

func then(a []string, b ...string) []string { return append(append([]string{}, a...), b...) }

// featuredSession is the session most shots show.
var featuredSession = open("Add Apple Pay to checkout")

// shots are the README's screenshots, each showing one thing rush does.
var shots = []shot{
	// The actual initial Agents list, before opening a session.
	{name: "agents", w: 180, h: 46},
	{name: "agents-before", w: 180, h: 46},
	{name: "feed", w: 120, h: 40},
	// The conversation: what was asked, what it did, what it said.
	{name: "session", w: 180, h: 50, steps: featuredSession},
	// A failed step opened: its script and the error.
	{name: "session-code", w: 180, h: 58, steps: then(featuredSession, "Up", "Up", "Up", "Right", "Down", "Right", "wait=500ms")},
	// Every file the session changed, with its diff.
	{name: "changes", w: 180, h: 52, steps: then(featuredSession, "]", "]", "wait=800ms", "Up", "Up", "Right", "wait=500ms")},
	// Subagents, each with what it was asked and what it's doing.
	{name: "subagents", w: 180, h: 46, steps: then(featuredSession, "]", "]", "]", "]", "wait=800ms", "Up", "Up", "wait=800ms")},
	// Searching every transcript from ctrl+k.
	{name: "command-bar-search", w: 180, h: 46, steps: then(featuredSession, "C-k", "text=merchant", "wait=4s")},
	// A command the agent handed you, run in place, caught mid-deploy.
	{name: "command-run", w: 180, h: 50, steps: then(open("Put the checkout on staging"), "Up", "Enter", "wait=17s")},
}
