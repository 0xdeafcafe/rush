package efficiency

import (
	"bufio"
	"os"
	"path/filepath"
	"sort"
	"sync"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Event is something that changed how tokens are spent: a saver installed
// or removed, a setting changed, or a note you wrote. They're the markers
// on the Timeline, kept in events.jsonl, one per line.
type Event struct {
	At      time.Time `json:"at"`
	Kind    string    `json:"kind"` // install, remove, setting, note; "used" is worked out, never kept
	Saver   string    `json:"saver,omitempty"`
	Account string    `json:"account,omitempty"`
	// Source is how it's known: rush did it, rush noticed it, it was
	// read from an install date, or you wrote it.
	Source string `json:"source"`
	Detail string `json:"detail,omitempty"`
}

var eventsMu sync.Mutex

func eventsPath() string   { return filepath.Join(Dir(), "events.jsonl") }
func detectedPath() string { return filepath.Join(Dir(), "detected.json") }

// LoadEvents reads the log, oldest first.
func LoadEvents() []Event {
	eventsMu.Lock()
	defer eventsMu.Unlock()
	f, err := os.Open(eventsPath())
	if err != nil {
		return nil
	}
	defer f.Close()
	var out []Event
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 64<<10), 1<<20)
	for sc.Scan() {
		var e Event
		if jsonx.Unmarshal(sc.Bytes(), &e) == nil && !e.At.IsZero() {
			out = append(out, e)
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].At.Before(out[j].At) })
	return out
}

// AddEvent appends to the log.
func AddEvent(e Event) error {
	eventsMu.Lock()
	defer eventsMu.Unlock()
	if e.At.IsZero() {
		e.At = time.Now()
	}
	b, err := jsonx.Marshal(e)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(Dir(), 0o700); err != nil {
		return err
	}
	f, err := os.OpenFile(eventsPath(), os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	_, err = f.Write(append(b, '\n'))
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	return err
}

type seen struct {
	Status Status `json:"s"`
	Value  string `json:"v,omitempty"`
}

// Observe detects every saver in an account and logs what changed since it
// last looked. The first look at an account logs only what has a known
// install date. It returns what was found, by saver ID.
func Observe(env *Env) map[string]Found {
	found := map[string]Found{}
	for i := range Catalog {
		found[Catalog[i].ID] = env.Detect(&Catalog[i])
	}
	eventsMu.Lock()
	all := map[string]map[string]seen{}
	if b, err := os.ReadFile(detectedPath()); err == nil {
		_ = jsonx.Unmarshal(b, &all)
	}
	eventsMu.Unlock()
	acct := env.Profile.Dir
	before, looked := all[acct]
	now := map[string]seen{}
	for id, f := range found {
		now[id] = seen{f.Status, f.Value}
		sv := Find(id)
		was, ok := before[id]
		switch {
		case !looked:
			if f.Status != Off && !f.Since.IsZero() {
				_ = AddEvent(Event{At: f.Since, Kind: "install", Saver: id, Account: acct, Source: "install date"})
			}
		case !ok || was.Status == f.Status && was.Value == f.Value:
		case sv.Setting != nil:
			detail := sv.Setting.Key + " = " + f.Value
			if f.Status == Off {
				detail = sv.Setting.Key + " unset"
			}
			_ = AddEvent(Event{Kind: "setting", Saver: id, Account: acct, Source: "noticed", Detail: detail})
		case f.Status == Off:
			_ = AddEvent(Event{Kind: "remove", Saver: id, Account: acct, Source: "noticed"})
		case was.Status == Off || f.Status == On:
			_ = AddEvent(Event{Kind: "install", Saver: id, Account: acct, Source: "noticed", Detail: joinParts(f.Parts)})
		}
	}
	eventsMu.Lock()
	all[acct] = now
	if b, err := jsonx.Marshal(all); err == nil {
		_ = writeFile(detectedPath(), b)
	}
	eventsMu.Unlock()
	return found
}

// Remember records what rush itself just changed, so Observe doesn't log
// it a second time as noticed.
func Remember(env *Env, id string, f Found) {
	eventsMu.Lock()
	defer eventsMu.Unlock()
	all := map[string]map[string]seen{}
	if b, err := os.ReadFile(detectedPath()); err == nil {
		_ = jsonx.Unmarshal(b, &all)
	}
	if all[env.Profile.Dir] == nil {
		all[env.Profile.Dir] = map[string]seen{}
	}
	all[env.Profile.Dir][id] = seen{f.Status, f.Value}
	if b, err := jsonx.Marshal(all); err == nil {
		_ = writeFile(detectedPath(), b)
	}
}

func joinParts(p []string) string {
	s := ""
	for i, x := range p {
		if i > 0 {
			s += ", "
		}
		s += x
	}
	return s
}
