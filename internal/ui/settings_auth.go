package ui

import (
	tea "charm.land/bubbletea/v2"
	"github.com/0xdeafcafe/rush/internal/agent"
	"github.com/0xdeafcafe/rush/internal/health"
	"os/exec"
)

// Native sign-in does not imply support for storing or swapping accounts.
func (m *Model) nativeAuthSection(k agent.Kind) (section, bool) {
	ad, ok := agent.Get(k)
	if !ok {
		return section{}, false
	}
	if _, ok := ad.(agent.Authenticator); !ok {
		return section{}, false
	}
	row := setting{label: "Sign in", what: "Open " + agent.HarnessLabel(k) + "'s own sign-in/setup in the terminal. Finish sign-in, then exit that program to return to rush.", keys: []string{"enter", "sign in"}}
	row.key = func(s string) (tea.Cmd, bool) {
		if s != "enter" {
			return nil, false
		}
		return m.nativeSignIn(k), true
	}
	rows := m.nativeAccountRows(k)
	rows = append(rows, row)
	return section{title: "Account", rows: rows}, true
}

// nativeSignIn is shared by account actions and model/harness settings.
func (m *Model) nativeSignIn(k agent.Kind) tea.Cmd {
	ad, ok := agent.Get(k)
	if !ok {
		return nil
	}
	auth, ok := ad.(agent.Authenticator)
	if !ok {
		return nil
	}
	p, found := m.profileOf(ad)
	if !found {
		p = agent.Profile{Kind: k}
	}
	return inTerminal(func() (*exec.Cmd, func(error) tea.Msg, error) {
		cmd, err := auth.SignInCommand(p)
		return cmd, func(err error) tea.Msg {
			return applyMsg(func(m *Model) tea.Cmd {
				if err != nil {
					m.flash("Sign-in failed: "+err.Error(), true)
					return nil
				}
				go health.Reset(string(agent.Migrated(string(k))), "")
				m.flash("Sign-in finished · checking credentials", false)
				return tea.Batch(m.findSignIns(), m.loadModels(string(k)))
			})
		}, err
	})
}

func (m *Model) nativeAccountRows(k agent.Kind) []setting {
	info, read := m.accts.summary[string(k)]
	status := "not checked"
	if _, ok := m.accts.now[string(k)]; ok {
		status = "credentials available"
	}
	if why := m.accts.why[string(k)]; why != "" {
		status = why
	}
	if read {
		switch {
		case info.Status != "" && info.Status != "unknown":
			status = info.Status
		case info.SignedIn:
			status = "credentials available"
		default:
			if _, available := m.accts.now[string(k)]; available {
				status = "credentials available"
			} else {
				status = "unknown"
			}
		}
	}
	rows := []setting{{label: "Status", value: status, what: info.Note}}
	add := func(label, value string) {
		if value != "" {
			rows = append(rows, setting{label: label, value: value})
		}
	}
	add("Account", info.Name)
	add("Email", info.Email)
	add("Organization", info.Org)
	add("Plan", info.Plan)
	add("Sign-in method", info.Method)
	add("Endpoint", info.Endpoint)
	rows = append(rows, setting{label: "Refresh account and models", what: "Read account information and the current model catalog without changing this session.", keys: []string{"enter", "refresh"}, key: func(s string) (tea.Cmd, bool) {
		if s != "enter" {
			return nil, false
		}
		return tea.Batch(m.findSignIns(), m.loadModels(string(k))), true
	}})
	return rows
}
