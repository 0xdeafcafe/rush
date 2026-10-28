package convo

import (
	"context"
	"sync"
	"sync/atomic"
	"time"
)

// Links rush knows the kind of are drawn with what it knows of them after
// them: a pull request where it stands, an issue whether it's closed. Each
// kind is a LinkKind, registered once (RegisterLink); this file asks them
// and keeps their answers, drawing nothing of its own.
//
// Nothing here is asked while a frame is drawn: the first drawing of a
// link starts its lookup and draws it bare, and once the answer is in,
// what was drawn is drawn again with it.

// A LinkKind is a kind of address rush knows something of. T is what its
// lookup says of one.
type LinkKind[T any] struct {
	// Name names the kind, for LinkInfo's callers: "github-pr".
	Name string
	// Key is the address as the kind knows it, so a link to a tab or an
	// anchor of the same thing shares one answer; "" when u isn't one.
	Key func(u string) string
	// Look asks what there is to know of the address at key. It runs off
	// the UI, a few at once.
	Look func(ctx context.Context, key string) (T, error)
	// Badge is what's drawn after a link, a few coloured words; "" draws
	// nothing. A change in it is what draws the conversation again.
	Badge func(v *T) string
	// Fresh is how long an answer stands before the next drawing asks
	// again; 0 is for good. An answer that failed is asked again after
	// linkFailedFor.
	Fresh func(v *T) time.Duration
}

// linkFailedFor is how long a failed lookup stands: GitHub signed out or
// rate limited isn't asked again on every frame.
const linkFailedFor = 10 * time.Minute

// linkKind is a LinkKind with its T put away, so kinds of every T share a
// list.
type linkKind interface {
	name() string
	key(u string) string
	look(ctx context.Context, key string) (any, error)
	badge(v any) string
	fresh(v any) time.Duration
}

func (k LinkKind[T]) name() string        { return k.Name }
func (k LinkKind[T]) key(u string) string { return k.Key(u) }
func (k LinkKind[T]) look(ctx context.Context, key string) (any, error) {
	return k.Look(ctx, key)
}
func (k LinkKind[T]) badge(v any) string {
	t, ok := v.(T)
	if !ok {
		return "" // not answered yet
	}
	return k.Badge(&t)
}
func (k LinkKind[T]) fresh(v any) time.Duration {
	t, ok := v.(T)
	if k.Fresh == nil || !ok {
		return 0
	}
	return k.Fresh(&t)
}

// linkAnswer is what a kind's lookup last said of one address.
type linkAnswer struct {
	kind   linkKind
	v      any
	known  bool // it has answered once
	failed bool // its last answer was a failure
	at     time.Time
	asking bool
}

var (
	linkKinds struct {
		sync.RWMutex
		list []linkKind
	}
	linkAnswers = struct {
		sync.Mutex
		by map[string]*linkAnswer // by kind name and key
	}{by: map[string]*linkAnswer{}}
	// linksLive is whether lookups run at all: off, as in tests, every
	// link is drawn bare and nothing is asked.
	linksLive atomic.Bool
	// Only a few lookups at once: a page of links asks about each the
	// first time it's drawn.
	linkSlots = make(chan struct{}, 3)
)

// RegisterLink adds a kind of link. A kind registered before it that also
// knows an address wins it.
func RegisterLink[T any](k LinkKind[T]) {
	linkKinds.Lock()
	linkKinds.list = append(linkKinds.list, k)
	linkKinds.Unlock()
}

// LookLinks turns link lookups on or off: the UI turns them on.
func LookLinks(on bool) {
	if linksLive.Swap(on) != on {
		lookupsGen.Add(1)
	}
}

// linkKindOf is the kind that knows u, and u's key for it.
func linkKindOf(u string) (linkKind, string) {
	linkKinds.RLock()
	defer linkKinds.RUnlock()
	for _, k := range linkKinds.list {
		if key := k.key(u); key != "" {
			return k, key
		}
	}
	return nil, ""
}

// LinkInfo is what the kind that knows u last said of it: the kind's
// name, u's key for it, and the answer, asking again in the background
// when that answer is old. ok is false for an address no kind knows, and
// until its lookup has answered. Callers type v by the kind's name.
func LinkInfo(u string) (kind, key string, v any, ok bool) {
	if !linksLive.Load() {
		return "", "", nil, false
	}
	k, key := linkKindOf(u)
	if k == nil {
		return "", "", nil, false
	}
	id := k.name() + " " + key
	linkAnswers.Lock()
	defer linkAnswers.Unlock()
	a := linkAnswers.by[id]
	if a == nil {
		a = &linkAnswer{kind: k}
		linkAnswers.by[id] = a
	}
	if !a.asking && a.due() {
		a.asking = true
		go a.ask(key)
	}
	// Unless its answer is for good, what drew it is drawn again when any
	// lookup lands, and asks again then if it's due.
	if a.asking || a.failed || a.kind.fresh(a.v) > 0 {
		lookupWaits.Add(1)
	}
	return k.name(), key, a.v, a.known
}

// LinkInfoOf is LinkInfo's answer as a T, for a caller that knows which
// kind knows u.
func LinkInfoOf[T any](kind, u string) (T, bool) {
	name, _, v, ok := LinkInfo(u)
	t, isT := v.(T)
	return t, ok && name == kind && isT
}

// due is whether the answer is old enough to ask again.
func (a *linkAnswer) due() bool {
	switch {
	case a.at.IsZero():
		return true
	case a.failed:
		return time.Since(a.at) >= linkFailedFor
	}
	d := a.kind.fresh(a.v)
	return d > 0 && time.Since(a.at) >= d
}

func (a *linkAnswer) ask(key string) {
	linkSlots <- struct{}{}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	v, err := a.kind.look(ctx, key)
	cancel()
	<-linkSlots
	linkAnswers.Lock()
	changed := err == nil && (!a.known || a.kind.badge(a.v) != a.kind.badge(v))
	a.asking, a.at, a.failed = false, time.Now(), err != nil
	if err == nil {
		a.v, a.known = v, true
	}
	linkAnswers.Unlock()
	if changed {
		lookupsGen.Add(1) // drawn again with it
	}
}

// ForgetLink has the next drawing of a link to u ask again, as after
// something was done to what it points at from rush.
func ForgetLink(u string) {
	k, key := linkKindOf(u)
	if k == nil {
		return
	}
	linkAnswers.Lock()
	if a := linkAnswers.by[k.name()+" "+key]; a != nil {
		a.at = time.Time{}
	}
	linkAnswers.Unlock()
	lookupsGen.Add(1)
}

// linkAfter is what's drawn after a link to u: what its kind knows of it,
// linked the same so a click on it opens it too, or nothing when no kind
// knows u or its lookup hasn't answered yet.
func linkAfter(u string) string {
	_, _, v, ok := LinkInfo(u)
	if !ok {
		return ""
	}
	k, _ := linkKindOf(u)
	b := k.badge(v)
	if b == "" {
		return ""
	}
	return " \x1b]8;;" + u + "\x1b\\" + b + "\x1b]8;;\x1b\\"
}
