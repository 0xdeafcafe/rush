// Package prdrive drives a pull request to mergeable for an agent: State is
// everything between it and merge, in the order to do it; Follow keeps
// asking GitHub (cheaply, through watch's shared cache) and wakes the
// agent's session only with news it can act on. Who may give the agent
// instructions is decided here, live, never by the agent.
package prdrive

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os/exec"
	"regexp"
	"strconv"
	"strings"
	"time"

	"encoding/json/jsontext"

	"github.com/0xdeafcafe/photon/jsonx"
)

// An Actor is who wrote something: a GitHub login, and whether the account
// is a bot (an app), which a person can't be however they name themselves.
type Actor struct {
	Login string
	Bot   bool
}

// A Comment is one comment in a review thread, or on the pull request.
type Comment struct {
	ID     int64
	Author Actor
	Body   string
	At     time.Time
	URL    string
}

// A Thread is a review thread: comments anchored to a line.
type Thread struct {
	ID       string
	Resolved bool
	Outdated bool
	Path     string
	Line     int
	Comments []Comment
}

// A Review is a submitted review.
type Review struct {
	Author Actor
	State  string // APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED
	At     time.Time
	Commit string
	Body   string
}

// A Check is one check on the head commit, the latest of its name.
type Check struct {
	Name     string
	Workflow string
	State    string // passed, failed, cancelled, pending or skipped
	Started  time.Time
	Run      int64
	Job      int64
	URL      string
	Required bool
}

// Raw is a pull request as GitHub says it is now.
type Raw struct {
	Repo, Title, URL, State, Author string
	Number                          int
	Draft                           bool
	HeadRef, Head, Base, HeadRepo   string
	HeadAt                          time.Time // when the head commit was pushed
	Mergeable                       string    // MERGEABLE, CONFLICTING, UNKNOWN
	Body                            string
	Reviews                         []Review
	Threads                         []Thread
	Comments                        []Comment // on the pull request itself
	Checks                          []Check
	Alerts                          []string        // open code-scanning alerts
	Required                        map[string]bool // the checks the base requires, by name
}

// GH runs gh with args and gives its output. Tests set it.
var GH = func(ctx context.Context, args ...string) ([]byte, error) {
	var out, errb bytes.Buffer
	for attempt := range 3 {
		out.Reset()
		errb.Reset()
		cmd := exec.CommandContext(ctx, "gh", args...)
		cmd.Stdout, cmd.Stderr = &out, &errb
		err := cmd.Run()
		if err == nil {
			return out.Bytes(), nil
		}
		msg := strings.TrimSpace(errb.String())
		if !transient.MatchString(msg) || attempt == 2 {
			if errors.Is(err, exec.ErrNotFound) {
				return nil, errors.New("gh isn't installed")
			}
			if i := strings.IndexByte(msg, '\n'); i > 0 {
				msg = msg[:i]
			}
			return out.Bytes(), fmt.Errorf("gh %s: %s", strings.Join(args[:min(2, len(args))], " "), or(msg, err.Error()))
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(time.Duration(2+attempt*4) * time.Second):
		}
	}
	return nil, errors.New("unreachable")
}

var transient = regexp.MustCompile(`HTTP 50[234]|timeout|EOF|connection reset`)

// gql runs a GraphQL query, its values inlined, and unmarshals data into v.
func gql(ctx context.Context, query string, v any) error {
	b, err := GH(ctx, "api", "graphql", "-f", "query="+strings.Join(strings.Fields(query), " "))
	if err != nil {
		return err
	}
	var r struct {
		Data   jsontext.Value `json:"data"`
		Errors []struct {
			Message string `json:"message"`
		} `json:"errors"`
	}
	if err := jsonx.Unmarshal(b, &r); err != nil {
		return err
	}
	if len(r.Errors) > 0 {
		return errors.New("GitHub says: " + r.Errors[0].Message)
	}
	return jsonx.Unmarshal(r.Data, v)
}

type gqlActor struct {
	Typename string `json:"__typename"`
	Login    string `json:"login"`
}

func (a *gqlActor) actor() Actor {
	if a == nil {
		return Actor{Login: "ghost"}
	}
	return Actor{Login: a.Login, Bot: a.Typename == "Bot"}
}

// Fetch is pull request n of repo as GitHub says now.
func Fetch(ctx context.Context, repo string, n int) (Raw, error) {
	owner, name, ok := strings.Cut(repo, "/")
	if !ok {
		return Raw{}, fmt.Errorf("%q isn't owner/name", repo)
	}
	base := fmt.Sprintf(`repository(owner:%q,name:%q){pullRequest(number:%d){`, owner, name, n)
	var top struct {
		Repository struct {
			PullRequest *struct {
				Title, URL, State, HeadRefName, HeadRefOid, BaseRefName, Mergeable, Body string
				IsDraft                                                                  bool
				Author                                                                   *gqlActor
				HeadRepository                                                           *struct{ NameWithOwner string }
				Reviews                                                                  struct {
					Nodes []struct {
						Author      *gqlActor
						State, Body string
						SubmittedAt time.Time
						Commit      *struct{ Oid string }
					}
				}
				Comments struct {
					Nodes []struct {
						DatabaseID int64 `json:"databaseId"`
						Author     *gqlActor
						Body, URL  string
						CreatedAt  time.Time
					}
				}
				Commits struct {
					Nodes []struct {
						Commit struct{ PushedDate, CommittedDate time.Time }
					}
				}
			}
		}
	}
	err := gql(ctx, "query{"+base+`
		title url state isDraft author{__typename login} headRefName headRefOid baseRefName
		headRepository{nameWithOwner} mergeable body
		reviews(last:100){nodes{author{__typename login} state body submittedAt commit{oid}}}
		comments(last:100){nodes{databaseId author{__typename login} body url createdAt}}
		commits(last:1){nodes{commit{pushedDate committedDate}}}
	}}}`, &top)
	if err != nil {
		return Raw{}, err
	}
	p := top.Repository.PullRequest
	if p == nil {
		return Raw{}, fmt.Errorf("%s has no pull request #%d", repo, n)
	}
	r := Raw{Repo: repo, Number: n, Title: p.Title, URL: p.URL, State: p.State, Draft: p.IsDraft,
		Author: p.Author.actor().Login, HeadRef: p.HeadRefName, Head: p.HeadRefOid, Base: p.BaseRefName,
		Mergeable: p.Mergeable, Body: p.Body}
	if p.HeadRepository != nil {
		r.HeadRepo = p.HeadRepository.NameWithOwner
	}
	if c := p.Commits.Nodes; len(c) > 0 {
		r.HeadAt = c[0].Commit.PushedDate
		if r.HeadAt.IsZero() {
			r.HeadAt = c[0].Commit.CommittedDate
		}
	}
	for _, v := range p.Reviews.Nodes {
		rv := Review{Author: v.Author.actor(), State: v.State, At: v.SubmittedAt, Body: v.Body}
		if v.Commit != nil {
			rv.Commit = v.Commit.Oid
		}
		r.Reviews = append(r.Reviews, rv)
	}
	for _, c := range p.Comments.Nodes {
		r.Comments = append(r.Comments, Comment{ID: c.DatabaseID, Author: c.Author.actor(), Body: c.Body, At: c.CreatedAt, URL: c.URL})
	}
	if r.Threads, err = fetchThreads(ctx, base); err != nil {
		return r, err
	}
	if r.Checks, err = fetchChecks(ctx, base); err != nil {
		return r, err
	}
	r.Required = Required(ctx, repo, r.Base)
	for i := range r.Checks {
		r.Checks[i].Required = r.Required[r.Checks[i].Name]
	}
	r.Alerts = alerts(ctx, repo, n)
	return r, nil
}

func fetchThreads(ctx context.Context, base string) ([]Thread, error) {
	var out []Thread
	cursor := ""
	for {
		after := ""
		if cursor != "" {
			after = fmt.Sprintf(",after:%q", cursor)
		}
		var page struct {
			Repository struct {
				PullRequest struct {
					ReviewThreads struct {
						PageInfo struct {
							HasNextPage bool
							EndCursor   string
						}
						Nodes []struct {
							ID         string
							IsResolved bool
							IsOutdated bool
							Path       string
							Line       int
							Comments   struct {
								Nodes []struct {
									DatabaseID int64 `json:"databaseId"`
									Author     *gqlActor
									Body, URL  string
									CreatedAt  time.Time
								}
							}
						}
					}
				}
			}
		}
		err := gql(ctx, "query{"+base+"reviewThreads(first:100"+after+`){
			pageInfo{hasNextPage endCursor}
			nodes{id isResolved isOutdated path line
				comments(first:100){nodes{databaseId author{__typename login} body url createdAt}}}
		}}}}`, &page)
		if err != nil {
			return out, err
		}
		rt := page.Repository.PullRequest.ReviewThreads
		for _, t := range rt.Nodes {
			th := Thread{ID: t.ID, Resolved: t.IsResolved, Outdated: t.IsOutdated, Path: t.Path, Line: t.Line}
			for _, c := range t.Comments.Nodes {
				th.Comments = append(th.Comments, Comment{ID: c.DatabaseID, Author: c.Author.actor(), Body: c.Body, At: c.CreatedAt, URL: c.URL})
			}
			out = append(out, th)
		}
		if !rt.PageInfo.HasNextPage {
			return out, nil
		}
		cursor = rt.PageInfo.EndCursor
	}
}

var jobID = regexp.MustCompile(`/job/(\d+)`)

func fetchChecks(ctx context.Context, base string) ([]Check, error) {
	latest := map[string]Check{}
	var order []string
	cursor := ""
	for {
		after := ""
		if cursor != "" {
			after = fmt.Sprintf(",after:%q", cursor)
		}
		var page struct {
			Repository struct {
				PullRequest struct {
					Commits struct {
						Nodes []struct {
							Commit struct {
								StatusCheckRollup *struct {
									Contexts struct {
										PageInfo struct {
											HasNextPage bool
											EndCursor   string
										}
										Nodes []struct {
											Typename                  string `json:"__typename"`
											Name, Status, Conclusion  string
											DetailsURL                string `json:"detailsUrl"`
											StartedAt                 time.Time
											Context, State, TargetURL string
											CreatedAt                 time.Time
											CheckSuite                *struct {
												WorkflowRun *struct {
													DatabaseID int64 `json:"databaseId"`
													Workflow   struct{ Name string }
												}
											}
										}
									}
								}
							}
						}
					}
				}
			}
		}
		err := gql(ctx, "query{"+base+"commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100"+after+`){
			pageInfo{hasNextPage endCursor}
			nodes{__typename
				... on CheckRun{name status conclusion detailsUrl startedAt
					checkSuite{workflowRun{databaseId workflow{name}}}}
				... on StatusContext{context state targetUrl createdAt}}
		}}}}}}}}`, &page)
		if err != nil {
			return nil, err
		}
		nodes := page.Repository.PullRequest.Commits.Nodes
		if len(nodes) == 0 || nodes[0].Commit.StatusCheckRollup == nil {
			break
		}
		ctxs := nodes[0].Commit.StatusCheckRollup.Contexts
		for _, c := range ctxs.Nodes {
			var ch Check
			if c.Typename == "CheckRun" {
				ch = Check{Name: c.Name, State: runState(c.Status, c.Conclusion), Started: c.StartedAt, URL: c.DetailsURL}
				if c.CheckSuite != nil && c.CheckSuite.WorkflowRun != nil {
					ch.Workflow, ch.Run = c.CheckSuite.WorkflowRun.Workflow.Name, c.CheckSuite.WorkflowRun.DatabaseID
				}
				if m := jobID.FindStringSubmatch(c.DetailsURL); m != nil {
					ch.Job, _ = strconv.ParseInt(m[1], 10, 64)
				}
			} else {
				st := map[string]string{"SUCCESS": "passed", "PENDING": "pending", "EXPECTED": "pending"}[c.State]
				ch = Check{Name: c.Context, State: or(st, "failed"), Started: c.CreatedAt, URL: c.TargetURL}
			}
			key := ch.Workflow + "\x00" + ch.Name
			old, seen := latest[key]
			if !seen {
				order = append(order, key)
			}
			if !seen || !ch.Started.Before(old.Started) {
				latest[key] = ch
			}
		}
		if !ctxs.PageInfo.HasNextPage {
			break
		}
		cursor = ctxs.PageInfo.EndCursor
	}
	out := make([]Check, 0, len(order))
	for _, k := range order {
		out = append(out, latest[k])
	}
	return out, nil
}

func runState(status, conclusion string) string {
	if status != "COMPLETED" {
		return "pending"
	}
	switch conclusion {
	case "SUCCESS", "NEUTRAL":
		return "passed"
	case "SKIPPED", "STALE":
		return "skipped"
	case "CANCELLED":
		return "cancelled"
	}
	return "failed"
}

// Required are the checks base's rules and protection require, by name.
// Tests set it.
var Required = func(ctx context.Context, repo, base string) map[string]bool {
	out := map[string]bool{}
	if b, err := GH(ctx, "api", "repos/"+repo+"/rules/branches/"+base); err == nil {
		var rules []struct {
			Type       string `json:"type"`
			Parameters struct {
				Checks []struct {
					Context string `json:"context"`
				} `json:"required_status_checks"`
			} `json:"parameters"`
		}
		if jsonx.Unmarshal(b, &rules) == nil {
			for _, r := range rules {
				if r.Type == "required_status_checks" {
					for _, c := range r.Parameters.Checks {
						out[c.Context] = true
					}
				}
			}
		}
	}
	if b, err := GH(ctx, "api", "repos/"+repo+"/branches/"+base+"/protection"); err == nil {
		var p struct {
			Checks struct {
				Contexts []string `json:"contexts"`
			} `json:"required_status_checks"`
		}
		if jsonx.Unmarshal(b, &p) == nil {
			for _, c := range p.Checks.Contexts {
				out[c] = true
			}
		}
	}
	return out
}

func alerts(ctx context.Context, repo string, n int) []string {
	b, err := GH(ctx, "api", fmt.Sprintf("repos/%s/code-scanning/alerts?ref=refs/pull/%d/merge&state=open&per_page=50", repo, n))
	if err != nil {
		return nil
	}
	var list []struct {
		Rule struct {
			ID string `json:"id"`
		} `json:"rule"`
		Instance struct {
			Location struct {
				Path string `json:"path"`
				Line int    `json:"start_line"`
			} `json:"location"`
		} `json:"most_recent_instance"`
	}
	if jsonx.Unmarshal(b, &list) != nil {
		return nil
	}
	var out []string
	for _, a := range list {
		out = append(out, fmt.Sprintf("%s %s:%d", a.Rule.ID, a.Instance.Location.Path, a.Instance.Location.Line))
	}
	return out
}

// Me is the login gh is signed in as. Tests set it.
var Me = func(ctx context.Context) (string, error) {
	b, err := GH(ctx, "api", "user", "--jq", ".login")
	return strings.TrimSpace(string(b)), err
}

func or(s ...string) string {
	for _, v := range s {
		if v != "" {
			return v
		}
	}
	return ""
}
