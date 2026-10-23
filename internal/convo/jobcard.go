package convo

import (
	"sync"
	"time"
)

// A JobCard is how a plugin has a task it knows drawn in place of the
// shell row: what it's waiting on, how far it's got, in its own words.
// The plugin says what each piece is (a Tone); rush picks the colours.
type JobCard struct {
	Mark string // its mark, before the kind
	Tone Tone   // the mark's
	Spin bool   // waiting: the mark turns, rush's spinner, in Tone
	Kind string // in a word, where the shell row says "shell"
	Body []Seg  // after the kind
	// Right ends the row, before the task's own state; Detail is a line
	// each under the row when the task is picked.
	Right  []Seg
	Detail [][]Seg
}

// A Seg is a run of a card's text in one tone.
type Seg struct {
	Text string
	Tone Tone
}

// Tone is what a piece of a card says: how rush colours it.
type Tone int

const (
	ToneSub   Tone = iota // the ordinary, secondary
	ToneText              // what to read first
	ToneDim               // what's there for reference
	ToneFaint             // punctuation
	ToneGood              // passed
	ToneBad               // failed
	ToneWait              // still going
	ToneInfo              // what it's waiting for
)

// A JobCarder is a plugin's card for task j, if j is one of its own.
type JobCarder func(j *Job, now time.Time) (JobCard, bool)

var (
	carders  []JobCarder
	cardersM sync.RWMutex
)

// RegisterJobCard adds a plugin's cards. A bundled plugin calls it from
// its init.
func RegisterJobCard(f JobCarder) {
	cardersM.Lock()
	defer cardersM.Unlock()
	carders = append(carders, f)
}

// JobCardOf is a plugin's card for running task j: the first that knows it.
func JobCardOf(j *Job, now time.Time) (JobCard, bool) {
	if j == nil || !j.Running() {
		return JobCard{}, false
	}
	cardersM.RLock()
	defer cardersM.RUnlock()
	for _, f := range carders {
		if c, ok := f(j, now); ok {
			return c, true
		}
	}
	return JobCard{}, false
}
