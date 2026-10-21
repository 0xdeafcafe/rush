package prdrive

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Most a reply may say, and how often we may speak in one thread: past
// that, the point goes to the user, not a third reply.
const (
	maxReply   = 700
	maxReplies = 2
)

// Reply posts body in thread of pull request n, unless it would start or
// carry on an argument: see why in the error.
func Reply(ctx context.Context, repo string, n int, thread, body string, trust *Trust) error {
	raw, err := Fetch(ctx, repo, n)
	if err != nil {
		return err
	}
	me, err := Me(ctx)
	if err != nil {
		return err
	}
	us := []string{me, raw.Author}
	i := slices.IndexFunc(raw.Threads, func(t Thread) bool { return t.ID == thread })
	if i < 0 {
		return fmt.Errorf("thread %s isn't on #%d", thread, n)
	}
	if err := mayReply(ctx, raw.Threads[i], strings.TrimSpace(body), us, trust); err != nil {
		return err
	}
	// A JSON string is a GraphQL string.
	text, _ := jsonx.Marshal(strings.TrimSpace(body))
	q := fmt.Sprintf(`mutation{addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:%q,body:%s}){comment{url}}}`, thread, text)
	return gql(ctx, q, &struct{}{})
}

func mayReply(ctx context.Context, t Thread, body string, us []string, trust *Trust) error {
	if len(t.Comments) == 0 {
		return errors.New("the thread is empty")
	}
	root := t.Comments[0]
	if r, _, _, _ := reviewerOf(root.Author, root.Body); r == nil && !slices.Contains(us, root.Author.Login) && !trust.Person(ctx, root.Author) {
		return errors.New("refused: an untrusted author opened this thread; that's the user's to answer")
	}
	if r, _, _, id := reviewerOf(root.Author, root.Body); r != nil && slices.Contains(r.Ignored, id) {
		return errors.New("refused: the user never wants '" + id + "' answered; resolve it")
	}
	if t.Resolved {
		return errors.New("refused: the thread is resolved")
	}
	ours, lastOurs := 0, false
	for _, c := range t.Comments {
		mine := slices.Contains(us, c.Author.Login)
		if mine {
			ours++
		}
		if mine || knownBot(c.Author) || trust.Person(ctx, c.Author) {
			lastOurs = mine
		}
	}
	switch {
	case lastOurs:
		return errors.New("refused: the last word here is already ours; resolve it, or wait for an answer")
	case ours >= maxReplies:
		return errors.New("refused: we've replied here twice; put the point to the user instead")
	case body == "":
		return errors.New("refused: the reply is empty")
	case len([]rune(body)) > maxReply:
		return fmt.Errorf("refused: %d characters; a reply is a sentence or two of evidence (at most %d)", len([]rune(body)), maxReply)
	case strings.Contains(body, "—"):
		return errors.New("refused: no em dashes")
	}
	return nil
}

// Resolve resolves review threads.
func Resolve(ctx context.Context, threads ...string) error {
	for _, t := range threads {
		if err := gql(ctx, fmt.Sprintf(`mutation{resolveReviewThread(input:{threadId:%q}){thread{isResolved}}}`, t), &struct{}{}); err != nil {
			return fmt.Errorf("%s: %w", t, err)
		}
	}
	return nil
}
