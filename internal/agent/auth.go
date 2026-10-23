package agent

import "os/exec"

// Authenticator offers the harness's own terminal login/setup flow without
// implying that Rush can store, switch or delete multiple accounts for it.
// The caller runs the command with terminal access and refreshes CheckKey.
type Authenticator interface {
	SignInCommand(p Profile) (*exec.Cmd, error)
}

// LoginHinter says how to sign the harness in again from a terminal, for a
// run whose login stopped working: "run `claude auth login` in a terminal".
type LoginHinter interface {
	LoginHint() string
}

// AccountSummary is safe account information for display. It never contains
// credentials. Unknown identity and plan fields stay empty.
type AccountSummary struct {
	SignedIn                                       bool
	Status                                         string // explicit evidence: credentials available, unknown, or another native state
	Method, Name, Email, Org, Plan, Endpoint, Note string
}

// AccountSummaryReader reads native account information off the UI goroutine.
// It does not imply account switching or credential-vault support.
type AccountSummaryReader interface {
	AccountSummary(p Profile) AccountSummary
}
