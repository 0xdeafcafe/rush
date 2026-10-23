package prdrive

import (
	"context"
	"regexp"
	"strings"
	"sync"
	"time"
)

// A Reviewer is a review bot rush knows by its account and its marks, and
// how far its findings are taken: what blocks, what the agent should
// never spend a turn on.
type Reviewer struct {
	Name  string
	Login string // the bot account's login, without [bot]
	// Finding reads a root comment of the bot's: whether it's one of this
	// reviewer's findings, how severe, whether it blocks, and its id.
	Finding func(body string) (severity string, blocking, ok bool, id string)
	// Signature starts its reviews; "" when any review by its login is its.
	Signature string
	// Check is the name of the check whose run is its review: rerun to have
	// it read replies on the same commit.
	Check string
	// Ignored are finding ids the user never wants worked: their threads
	// are resolved without a reply and never wake the agent.
	Ignored []string
}

var (
	lwFinding = regexp.MustCompile(`\*\*\[(P[012]) · (blocking|non-blocking)\]\*\*`)
	lwID      = regexp.MustCompile(`<!--\s*id:([\w.:-]+)\s*-->`)
	crLevels  = []struct{ mark, sev string }{
		{"🔴 Critical", "critical"}, {"🟠 Major", "major"}, {"🟡 Minor", "minor"}, {"🔵 Trivial", "trivial"}, {"🧹 Nitpick", "nitpick"},
	}
)

// Reviewers are the review bots rush knows. A bot not here is untrusted.
var Reviewers = []Reviewer{{
	Name: "LangWatchReviewBot", Login: "github-actions", Signature: "**@LangWatchReviewBot**", Check: "review",
	Finding: func(body string) (string, bool, bool, string) {
		m := lwFinding.FindStringSubmatch(body)
		if m == nil {
			return "", false, false, ""
		}
		id := ""
		if i := lwID.FindStringSubmatch(body); i != nil {
			id = i[1]
		}
		return m[1], m[2] == "blocking", true, id
	},
	// It never approves, and reads replies only on its next run: Playbook.
	Ignored: []string{"no-linked-issue"},
}, {
	Name: "CodeRabbit", Login: "coderabbitai", Check: "CodeRabbit",
	Finding: func(body string) (string, bool, bool, string) {
		for _, l := range crLevels {
			if strings.Contains(body, l.mark) {
				return l.sev, l.sev == "critical" || l.sev == "major", true, ""
			}
		}
		return "comment", false, true, ""
	},
}, {
	Name: "Copilot", Login: "copilot-pull-request-reviewer",
	Finding: func(string) (string, bool, bool, string) { return "comment", false, true, "" },
}}

// reviewerOf is the known reviewer whose finding body is, by a bot account.
func reviewerOf(a Actor, body string) (*Reviewer, string, bool, string) {
	if !a.Bot {
		return nil, "", false, ""
	}
	for i := range Reviewers {
		r := &Reviewers[i]
		if r.Login != a.Login {
			continue
		}
		if sev, blocking, ok, id := r.Finding(body); ok {
			return r, sev, blocking, id
		}
	}
	return nil, "", false, ""
}

// knownBot is whether a is one of the known reviewers' accounts.
func knownBot(a Actor) bool {
	if !a.Bot {
		return false
	}
	for _, r := range Reviewers {
		if r.Login == a.Login {
			return true
		}
	}
	return false
}

// Trust decides who may give the agent instructions: a person who is a
// member of the repository's owner, or can write to it, asked of GitHub
// live and kept a short while; a known reviewer's bot, by its findings.
type Trust struct {
	Repo string
	// Ask is whether login is trusted, asked of GitHub. Tests set it.
	Ask func(ctx context.Context, repo, login string) bool

	mu    sync.Mutex
	cache map[string]trusted
}

type trusted struct {
	ok bool
	at time.Time
}

// trustFor is how long an answer is kept: a follower runs for days, and
// someone who leaves the org stops being trusted within it.
const trustTTL = 30 * time.Minute

// Person is whether a, a person, is trusted.
func (t *Trust) Person(ctx context.Context, a Actor) bool {
	if a.Bot || a.Login == "" || a.Login == "ghost" {
		return false
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	if c, ok := t.cache[a.Login]; ok && time.Since(c.at) < trustTTL {
		return c.ok
	}
	ask := t.Ask
	if ask == nil {
		ask = askGitHub
	}
	ok := ask(ctx, t.Repo, a.Login)
	if t.cache == nil {
		t.cache = map[string]trusted{}
	}
	t.cache[a.Login] = trusted{ok, time.Now()}
	return ok
}

// askGitHub: a member of the owning org (204), or a collaborator who can
// write. author_association alone isn't proof.
func askGitHub(ctx context.Context, repo, login string) bool {
	owner, _, _ := strings.Cut(repo, "/")
	if b, err := GH(ctx, "api", "-i", "orgs/"+owner+"/members/"+login); err == nil {
		if f := strings.Fields(string(b)); len(f) > 1 && f[1] == "204" {
			return true
		}
	}
	b, err := GH(ctx, "api", "repos/"+repo+"/collaborators/"+login+"/permission", "--jq", ".permission")
	if err != nil {
		return false
	}
	switch strings.TrimSpace(string(b)) {
	case "admin", "maintain", "write":
		return true
	}
	return false
}
