package advisor

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/efficiency"
	"github.com/0xdeafcafe/rush/internal/netwatch"
)

// What each pass may spend, and how long it may take. The scout runs on
// the agent's quick model, the review on its careful one.
const (
	scoutBudget = 0.30
	scoutTime   = 5 * time.Minute

	reviewBudget = 1.50
	reviewTime   = 10 * time.Minute
)

// Result is what one pass found and what it cost.
type Result struct {
	At       time.Time
	Findings []Finding
	Reviewed []time.Time // when each Opus review ran
	Spent    float64
	Err      error
}

// Pass runs the advisor once, as acct: Haiku proposes, then Opus checks
// the candidates worth money, new and left from earlier passes, while the
// day's reviews last. reviews is how many the cap still allows.
func Pass(ctx context.Context, acct agent.Profile, in Input, reviews int) Result {
	res := Result{At: time.Now()}
	briefs := filepath.Join(Dir(), "briefs")
	if in.Briefs == nil {
		var paths []string
		for _, s := range in.Top {
			paths = append(paths, s.Path)
		}
		in.Briefs = Briefs(briefs, paths)
	}
	digest := Digest(in)
	var scoutModel, reviewModel string
	if q, ok := agent.As[agent.Querier](acct.Kind); ok {
		scoutModel, reviewModel = q.QueryModels()
	}
	// Haiku reads the briefs alone; Opus may check them against the
	// transcripts.
	scoutDirs, reviewDirs := []string{briefs}, []string{briefs, efficiency.TranscriptsDir(acct)}

	var scout struct {
		Findings []proposal `json:"findings"`
	}
	cost, err := ask(ctx, acct, call{
		model: scoutModel, budget: scoutBudget, timeout: scoutTime, dirs: scoutDirs,
		system: scoutSystem, prompt: digest, schema: scoutSchema,
	}, &scout)
	res.Spent += cost
	if err != nil {
		res.Err = fmt.Errorf("haiku: %w", err)
		return res
	}

	var cands []Finding
	for _, p := range scout.Findings {
		f := p.finding(Candidate, res.At)
		if f.Title == "" || slices.Contains(in.Settled, f.ID) {
			continue
		}
		cands = append(cands, f)
	}
	for _, f := range in.Pending {
		if i := slices.IndexFunc(cands, func(c Finding) bool { return c.ID == f.ID }); i >= 0 {
			cands[i].Tries = f.Tries // proposed again: its failed reviews still count
		} else {
			cands = append(cands, f)
		}
	}
	roots := []string{acct.Dir}
	for _, s := range in.Top {
		roots = append(roots, s.Project)
	}
	// The most at stake is checked first.
	slices.SortStableFunc(cands, func(a, b Finding) int {
		switch {
		case a.Weekly > b.Weekly:
			return -1
		case a.Weekly < b.Weekly:
			return 1
		}
		return 0
	})
	for i := range cands {
		c := &cands[i]
		c.Open = checkOpen(c.Open, roots)
		if reviews == 0 || c.Weekly < ReviewAt || c.Tries >= Tries {
			continue
		}
		if in.Reserve != nil {
			if err := in.Reserve(); err != nil {
				res.Err = err
				break
			}
		}
		reviews--
		res.Reviewed = append(res.Reviewed, time.Now())
		var v verdict
		cost, err := ask(ctx, acct, call{
			model: reviewModel, budget: reviewBudget, timeout: reviewTime, dirs: reviewDirs,
			system: reviewSystem, prompt: reviewPrompt(digest, *c), schema: reviewSchema,
		}, &v)
		res.Spent += cost
		if err != nil {
			res.Err = fmt.Errorf("opus: %w", err)
			c.Tries++ // it stays a candidate, until it has failed too often
			continue
		}
		if !v.Confirmed {
			c.Status, c.Note = Rejected, v.Note
			continue
		}
		// Kept under the candidate's ID, so Haiku proposing it again
		// finds it already checked.
		id := c.ID
		*c = v.proposal.finding(Confirmed, res.At)
		c.ID, c.Note, c.Open = id, v.Note, checkOpen(v.Open, roots)
	}
	res.Findings = cands
	return res
}

// checkOpen is a file a finding may send you to: one that exists, under
// the account's folder or a folder its sessions worked in. A model wrote
// it, so anything else is dropped.
func checkOpen(p string, roots []string) string {
	if p == "" || !filepath.IsAbs(p) {
		return ""
	}
	p = filepath.Clean(p)
	if st, err := os.Stat(p); err != nil || !st.Mode().IsRegular() {
		return ""
	}
	for _, r := range roots {
		if r != "" && strings.HasPrefix(p, filepath.Clean(r)+string(filepath.Separator)) {
			return p
		}
	}
	return ""
}

// proposal is a finding as a model writes it.
type proposal struct {
	Title    string   `json:"title"`
	Detail   string   `json:"detail"`
	Evidence []string `json:"evidence"`
	Weekly   float64  `json:"weeklyCost"`
	Fix      string   `json:"fix"`
	Open     string   `json:"open"`
}

func (p proposal) finding(status string, at time.Time) Finding {
	f := Finding{ID: idOf(p.Title), Title: strings.TrimSpace(p.Title), Detail: strings.TrimSpace(p.Detail),
		Evidence: p.Evidence, Weekly: max(0, p.Weekly), Open: p.Open, Status: status, At: at}
	if efficiency.Find(p.Fix) != nil {
		f.Fix = p.Fix
	}
	return f
}

type verdict struct {
	Confirmed bool   `json:"confirmed"`
	Note      string `json:"note"`
	proposal
}

// call is one `claude -p`.
type call struct {
	model   string
	budget  float64
	timeout time.Duration
	dirs    []string
	system  string
	prompt  string
	schema  string
}

// Job is what the advisor's calls are called where rush shows what waits
// on the network.
const Job = "Advisor"

// ask runs c and decodes its structured answer into out. It reports what
// the call cost even when it fails. While the network is down it waits
// rather than run.
func ask(ctx context.Context, p agent.Profile, c call, out any) (float64, error) {
	if !netwatch.Run(Job) {
		return 0, netwatch.ErrOffline
	}
	cost, err := run(ctx, p, c, out)
	netwatch.Done(Job, err)
	return cost, err
}

func run(ctx context.Context, p agent.Profile, c call, out any) (float64, error) {
	ctx, cancel := context.WithTimeout(ctx, c.timeout)
	defer cancel()
	if err := os.MkdirAll(Dir(), 0o700); err != nil {
		return 0, err
	}
	q, ok := agent.As[agent.Querier](p.Kind)
	if !ok {
		return 0, fmt.Errorf("the advisor can't run on %s", p.Kind)
	}
	args, env := q.QueryCommand(p, agent.Query{Model: c.model, System: c.system, Prompt: c.prompt, Schema: c.schema, Dirs: c.dirs, Budget: c.budget})
	// The program is where the profile's agent was found installed.
	prog := agent.Path(p.Kind)
	if prog == "" {
		return 0, fmt.Errorf("the advisor runs %s, which isn't installed", p.Kind)
	}
	cmd := exec.CommandContext(ctx, prog, args...)
	cmd.Dir = Dir()
	env = append(env, "RUSH_ADVISOR=1")
	cmd.Env = env
	// Its own process group, so stopping it takes whatever it started too.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error { return syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL) }
	cmd.Stdin = strings.NewReader(c.prompt)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Start(); err != nil {
		return 0, err
	}
	running.add(cmd)
	runErr := cmd.Wait()
	running.remove(cmd)

	ans, ok := q.QueryAnswer(stdout.Bytes())
	if !ok {
		// Stopped or broken halfway: what it spent isn't known, so count
		// what it was allowed to.
		if runErr != nil {
			return c.budget, fmt.Errorf("%w: %s", runErr, firstLine(stderr.String()))
		}
		return c.budget, errors.New("its answer couldn't be read")
	}
	switch {
	case ans.Err != nil:
		return ans.Cost, ans.Err
	case len(ans.Out) == 0 || string(ans.Out) == "null":
		return ans.Cost, errors.New("no answer")
	}
	return ans.Cost, jsonx.Unmarshal(ans.Out, out)
}

// running are the passes' claude processes, for Stop.
var running procs

type procs struct {
	mu sync.Mutex
	m  map[*exec.Cmd]bool
}

func (p *procs) add(c *exec.Cmd) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.m == nil {
		p.m = map[*exec.Cmd]bool{}
	}
	p.m[c] = true
}

func (p *procs) remove(c *exec.Cmd) {
	p.mu.Lock()
	defer p.mu.Unlock()
	delete(p.m, c)
}

// Stop ends any pass still running, as rush quits: a claude left behind
// would spend on an answer nobody reads.
func Stop() {
	running.mu.Lock()
	defer running.mu.Unlock()
	for c := range running.m {
		_ = syscall.Kill(-c.Process.Pid, syscall.SIGKILL)
	}
}

func firstLine(s string) string {
	s, _, _ = strings.Cut(strings.TrimSpace(s), "\n")
	if len(s) > 200 {
		s = s[:200] + "…"
	}
	return s
}

func reviewPrompt(digest string, c Finding) string {
	b, _ := jsonx.MarshalIndent(proposal{c.Title, c.Detail, c.Evidence, c.Weekly, c.Fix, c.Open})
	return "## Candidate\n" + string(b) + "\n\n" + digest
}

const scoutSystem = `You are rush's advisor. rush is a terminal dashboard that runs the user's coding agents (Claude Code and others). You get a digest of the last 7 days of their agents' figures, which rush has already worked out.

Find up to 5 specific changes to how this user works with their agents that would cut tokens and cost, or make the agents faster. Look at habits and setup: CLAUDE.md and memory, settings, savers, model and effort choice, subagent use, sessions left to grow instead of starting fresh, files read again and again, noisy shell output, and prompts that make an agent explore more than it needs.

Rules:
- Every finding rests on evidence: figures from the digest, or lines in a session's brief. Put brief paths, and the lines you saw, in evidence.
- Each costly session has a brief: one short line for each prompt you gave (with the context size then), each tool call (with "→ size" of what it sent back) and each compaction. Briefs are small, so read the ones that matter. You can't read the transcripts themselves.
- Read only what a finding needs: two or three briefs at most. Each is about 16k tokens.
- Give no generic advice. If the figures don't show a problem, return fewer findings, or none.
- Don't repeat anything under "Already said".
- title: one short line, what to change. detail: one or two sentences, why, with the figure behind it.
- weeklyCost: the dollars a week this change would plausibly save, worked out from the digest's figures. That's the share it would cut, never the whole figure it touches. Be conservative, and use 0 if you can't tell.
- fix: a saver id from the digest's list when one addresses it, otherwise "". open: the absolute path of a file the user should edit, when that's the change, otherwise "".`

const reviewSystem = `You check findings for rush's advisor. A cheaper model read a digest of the user's coding-agent figures and proposed the candidate below. Your job is to decide whether it's true and worth the user's time.

Check its evidence against the sessions' briefs (one short line per prompt, tool call and compaction) and the digest's figures. When a brief isn't enough, grep the raw JSONL transcript. Use Grep with output_mode "count" or "files_with_matches", or a head_limit, because one transcript line can be hundreds of kilobytes. Never read a transcript whole. Confirm it only if the evidence holds, the change would really help this user, and the cost estimate is sound (correct it if not). Reject anything generic, already handled by a saver that's on, or unsupported.

When you confirm it, rewrite title and detail to be exact and actionable, in plain words: title is one short line, and detail is one or two sentences with the figure behind it. Keep or correct fix and open. note: one line on what you checked, or why you rejected it.`

const findingProps = `"title":{"type":"string"},"detail":{"type":"string"},"evidence":{"type":"array","items":{"type":"string"}},"weeklyCost":{"type":"number"},"fix":{"type":"string"},"open":{"type":"string"}`

var (
	scoutSchema  = `{"type":"object","properties":{"findings":{"type":"array","maxItems":5,"items":{"type":"object","properties":{` + findingProps + `},"required":["title","detail","evidence","weeklyCost","fix","open"]}}},"required":["findings"]}`
	reviewSchema = `{"type":"object","properties":{"confirmed":{"type":"boolean"},"note":{"type":"string"},` + findingProps + `},"required":["confirmed","note","title","detail","evidence","weeklyCost","fix","open"]}`
)
