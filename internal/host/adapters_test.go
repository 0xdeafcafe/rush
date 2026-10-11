package host

// The host runs Claude Code through its adapter, as rush registers it.
import _ "github.com/0xdeafcafe/rush/internal/adapters/claude"

func init() { titler, feedSummer = nil, nil } // no model calls from tests
