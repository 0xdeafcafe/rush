package agent

// Chooser is an adapter that says what a new session can be started with:
// the models, efforts and permission modes it takes (StartOptions' Model,
// Effort and Mode). Settings offers them for that agent; an adapter
// without one takes a typed model, and its own default for the rest.
type Chooser interface {
	Choices() Choices
}

// Choices are what a new session can start with. Each list is offered as
// it is, after the agent's own default, which is the empty ID.
type Choices struct {
	Models  []Choice
	Efforts []Choice
	Modes   []Choice
}

// Choice is one value and what it means: {"sonnet", "faster and cheaper,
// good for routine work"}.
type Choice struct {
	ID   string
	Note string
	// Context is a model's context window in tokens, when its lister
	// knows it; 0 when not.
	Context int64
}

// ChoicesOf are what agent k says a new session can start with.
func ChoicesOf(k Kind) (Choices, bool) {
	a, ok := Get(k)
	if !ok {
		return Choices{}, false
	}
	c, ok := a.(Chooser)
	if !ok {
		return Choices{}, false
	}
	return c.Choices(), true
}

// ModelLister is an adapter that reads the models its account offers now
// from profile p's home: Codex's, which come and go with the account. It
// reads the disk, so never on the UI.
type ModelLister interface {
	ListModels(p Profile) []Choice
}

// SkillRooter is an adapter whose skills another agent can read: the
// folders of <name>/SKILL.md skills it has in cwd. It reads the disk.
type SkillRooter interface {
	SkillRoots(p Profile, cwd string) []string
}

// RunnerSnapshot provides previously discovered runner metadata without launching
// a CLI or contacting a provider. Session startup must not wait on discovery of
// an unrelated harness. Settings remains responsible for refreshing the catalog.
type RunnerSnapshot interface {
	RunnerSnapshot() (models []Choice, ready bool)
}

// Trust is how much a permission mode lets an agent do without asking,
// whichever harness names it: plan and the asking modes are 0, approving
// edits 1, approving what a classifier or sandbox deems safe 2, never
// asking 3.
func Trust(mode string) int {
	switch mode {
	case "bypassPermissions", "full-access", "yolo", "auto-approve", "always-proceed":
		return 3
	case "auto":
		return 2
	case "acceptEdits", "autoEdit", "accept-edits":
		return 1
	}
	return 0
}

// TrustedAs is the mode of modes that trusts as much as mode does, or the
// most-trusting one short of it: a run started on another harness keeps
// its starter's trust and never gets more. "" when none trusts at all.
func TrustedAs(mode string, modes []Choice) string {
	want, best, at := Trust(mode), "", 0
	for _, m := range modes {
		if t := Trust(m.ID); t <= want && t > at {
			best, at = m.ID, t
		}
	}
	return best
}
