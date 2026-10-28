package ui

import (
	"context"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/0xdeafcafe/rush/internal/fleet"
)

// misdirectState is what rush knows to tell a message typed into the wrong
// session: each repo's newest commit subjects, read off the UI every few
// minutes, and whether the send under way was told to go ahead regardless.
type misdirectState struct {
	subjects map[string][]string // by repo root, newest first
	at       time.Time
	loading  bool
	skip     bool
}

// subjectsMsg is each repo's newest commit subjects, read off the UI.
type subjectsMsg map[string][]string

const subjectsEvery = 2 * time.Minute

// refreshSubjects reads the newest commit subjects of every repo a session
// works in, every few minutes; git runs in the command, never in Update.
func (m *Model) refreshSubjects() tea.Cmd {
	if m.snap == nil || m.misdirect.loading || time.Since(m.misdirect.at) < subjectsEvery {
		return nil
	}
	roots := map[string]bool{}
	for _, a := range m.snap.Agents {
		if r := agentRoot(a); r != "" {
			roots[r] = true
		}
	}
	if len(roots) == 0 {
		return nil
	}
	m.misdirect.loading = true
	return func() tea.Msg {
		out := subjectsMsg{}
		for r := range roots {
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			b, err := exec.CommandContext(ctx, "git", "-C", r, "log", "-5", "--format=%s").Output()
			cancel()
			if err == nil {
				out[r] = strings.Split(strings.TrimSpace(string(b)), "\n")
			}
		}
		return out
	}
}

func (m *Model) onSubjects(msg subjectsMsg) {
	m.misdirect.subjects, m.misdirect.at, m.misdirect.loading = msg, time.Now(), false
}

func agentRoot(a *fleet.Agent) string {
	if a.Root != "" {
		return a.Root
	}
	return a.Cwd
}

func agentRepo(a *fleet.Agent) string {
	if a.Repo != "" {
		return filepath.Base(a.Repo)
	}
	if r := agentRoot(a); r != "" {
		return filepath.Base(r)
	}
	return ""
}

// askMisdirected asks before a message typed into one session goes there
// when another live session is clearly what it's about; enter forwards it,
// s sends it here anyway and esc leaves it in the box to edit.
func (m *Model) askMisdirected(c *hostConn, text string, send func() tea.Cmd) bool {
	if m.misdirect.skip || m.snap == nil || strings.Contains(text, "[Image #") {
		return false
	}
	here := m.agentByKey(c.key)
	if here == nil {
		return false
	}
	var others []*fleet.Agent
	for _, a := range m.snap.Agents {
		// Only a session still at work could be the one meant: a done or
		// stopped one is history, and there are dozens of those.
		if a.Key != here.Key && !a.Past && !a.Interactive && a.State != "done" && a.State != "stopped" {
			others = append(others, a)
		}
	}
	to, words := misdirected(text, here, others, m.misdirect.subjects)
	if to == nil {
		return false
	}
	m.confirm = &confirmation{
		question: "Looks meant for " + to.DisplayName + " (" + agentRepo(to) + ")",
		detail:   "it mentions " + strings.Join(words, ", ") + " · esc leaves it here to edit",
		yesText:  "send there",
		again:    "space", // space says yes too, beside enter
		box:      c,
		onYes: func() tea.Cmd {
			c.input, c.back, c.imgs = nil, 0, imageRefs{}
			c.undo = undoStack{}
			return m.replyTo(to, text, text)
		},
		more: []confirmChoice{{key: "s", text: "send here anyway", do: func() tea.Cmd {
			m.misdirect.skip = true
			defer func() { m.misdirect.skip = false }()
			return send()
		}}},
	}
	return true
}

// misdirected is the session among others that text is clearly about rather
// than here, and the words that say so; nil when nothing stands out.
func misdirected(text string, here *fleet.Agent, others []*fleet.Agent, subjects map[string][]string) (*fleet.Agent, []string) {
	words := msgWords(text)
	if len(words) == 0 {
		return nil, nil
	}
	hereScore, _ := matchScore(words, here, subjects)
	var best *fleet.Agent
	var bestWords []string
	top, second := 0, 0
	for _, a := range others {
		s, hit := matchScore(words, a, subjects)
		switch {
		case s > top:
			best, bestWords, top, second = a, hit, s, top
		case s > second:
			second = s
		}
	}
	// Clearly better: enough on its own, well above here, and alone at the top.
	if best == nil || top < 3 || top < 2*hereScore+3 || top == second {
		return nil, nil
	}
	// Another repo, and more than one word of its own or its name: a session
	// in the same repo, or a single word in a commit subject, says nothing.
	there, mine := agentRepo(best), agentRepo(here)
	if there == "" || there == mine || len(bestWords) < 2 && !slices.Contains(bestWords, strings.ToLower(there)) {
		return nil, nil
	}
	return best, bestWords
}

// matchScore weighs the words of a message a session's context also has:
// its repo by name most, its newest commit and its name next, the rest one.
func matchScore(words []string, a *fleet.Agent, subjects map[string][]string) (int, []string) {
	bag := map[string]int{}
	add := func(s string, w int) {
		for _, t := range subjectWords(s) {
			bag[t] = max(bag[t], w)
		}
	}
	add(a.Branch, 2)
	add(a.Name, 2)
	add(a.DisplayName, 2)
	add(a.Intent, 2)
	add(a.Detail, 1)
	for _, t := range a.TodoItems {
		add(t.Label, 1)
	}
	for i, s := range subjects[agentRoot(a)] {
		add(s, max(0, 3-i)) // the newest commit counts most
	}
	if r := strings.ToLower(agentRepo(a)); len(r) >= 3 {
		bag[r] = 6
	}
	score, hit := 0, []string(nil)
	for _, w := range words {
		if bag[w] > 0 {
			score += bag[w]
			hit = append(hit, w)
		}
	}
	return score, hit
}

// msgWords are a message's distinct words worth matching on.
func msgWords(text string) []string {
	seen := map[string]bool{}
	var out []string
	for _, t := range subjectWords(text) {
		if !seen[t] {
			seen[t] = true
			out = append(out, t)
		}
	}
	return out
}

// subjectWords are the lowercased words of s, three letters or more, without the
// words every session shares.
func subjectWords(s string) []string {
	var out []string
	for _, f := range strings.FieldsFunc(strings.ToLower(s), func(r rune) bool {
		return (r < 'a' || r > 'z') && (r < '0' || r > '9')
	}) {
		if len(f) >= 3 && !commonWords[f] {
			out = append(out, f)
		}
	}
	return out
}

// shortcut: a fixed list of generic words; upgrade to weighting by how many sessions share a word if it misfires.
var commonWords = map[string]bool{}

func init() {
	for w := range strings.FieldsSeq(`the and for you all but not are was can its it's this that with from have has had need needs make
		should would could will into what when then them there their they just also more some like please use using any
		one two now new get got let see run ran out off our your yes now why how who did does done keep going continue
		test tests lint build commit push merge branch review check error errors change changes update code file files
		work feat fix chore refactor docs main stuff thing things good fine okay same here thanks over still back
		wip ship add remove try again first last next sure`) {
		commonWords[w] = true
	}
}
