package agent

import (
	"time"

	"github.com/0xdeafcafe/rush/internal/agent/usage"
)

// Spend is what a session's transcripts say it cost, and what else they
// say of it as they're priced.
type Spend struct {
	Cost  float64
	Usage usage.TokenUsage
	Model string
	First time.Time
	Last  time.Time
	PRs   []string
	Dirs  []string  // folders the agent worked in, subagents included
	Dir   string    // where its own transcript (not a subagent's) last worked
	DirAt time.Time // when Dir last changed
	Today float64
	Ready bool
	Halt  *Halt // the error its last turn ended on, if any
	// Progress is the last count it reported moving ("lint 11,065 →
	// 9,052"), and when; Context is its newest message's context, and
	// Compacts how often that was compacted.
	Progress   string
	ProgressAt time.Time
	Context    int64
	Compacts   int
}

// SpendTarget is one session's transcript, to price.
type SpendTarget struct {
	Key  string
	Path string
	// Live is an agent that may be writing: its subagents' transcripts are
	// checked every run. Others are only looked at when their own
	// transcript or subagents folder changes.
	Live bool
	// Past is a conversation nothing has open: its row is read again only
	// every so often, and so are its files.
	Past bool
}

// SpendScanner prices transcripts, reading each only as far as it grew,
// and keeps what it read in a cache of its own. Run and Flush may be
// called from any goroutine.
type SpendScanner interface {
	// Run prices every target whose files grew, by key.
	Run(targets []SpendTarget) map[string]Spend
	// Flush saves the cache, unless a Run is under way.
	Flush()
}

// SpendReader is an agent whose transcripts rush prices itself.
type SpendReader interface {
	SpendScanner() SpendScanner
}
